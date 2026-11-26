//! The credential ops of spec pass 14 (MEDULLA-IDENTITY-R730 to R735): administrator API
//! keys, session revoke by handle, the issuer key ring, SCIM client credentials,
//! administrator reset tokens and the MFA challenge store.
//!
//! Every clock read is the `now_ms` of the [`ApplyContext`], so a replica replaying the op
//! reaches the same state and a test drives a controlled clock. A secret arrives hashed in
//! the stamp; no reply carries a secret or a hash.

use super::super::audit::IdentityEvent;
use super::super::credentials::{
    AdminResetIssue, ApiKeyAdminIssue, ApiKeyView, IssuerKeyRecord, IssuerKeyRotate,
    IssuerKeyView, MfaChallengeConsume, MfaChallengeIssue, MfaChallengeKind, MfaChallengeRecord,
    MfaChallengeView, ScimClientCreate, ScimClientRecord, ScimClientUpdate, ScimClientView,
    SessionHandleRef, ISSUER_KEY_ALGORITHMS, MAX_ISSUER_KEYS, MAX_MFA_CHALLENGES,
    MAX_SCIM_CLIENTS, MFA_CHALLENGE_TTL_MS, SESSION_HANDLE_CHARS,
};
use super::super::model::{OneTimeToken, TokenPurpose};
use super::super::stamp::IdentityStamp;
use super::super::text::identifier;
use super::super::views::{session_handle, IdentityReply, ResetDelivery};
use super::super::{IdentityRefusal, MAX_API_KEY_LIFETIME_MS, MAX_ONE_TIME_TOKEN_LIFETIME_MS};
use super::sessions::PendingSession;
use super::{ApplyContext, IdentityStore};

/// The longest label a SCIM client carries.
const MAX_LABEL_CHARS: usize = 128;
/// The longest challenge a store holds.
const MAX_CHALLENGE_CHARS: usize = 1_024;

fn is_lower_hex(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn kind_name(kind: MfaChallengeKind) -> &'static str {
    match kind {
        MfaChallengeKind::Enrollment => "enrollment",
        MfaChallengeKind::Webauthn => "webauthn",
    }
}

fn scim_view(record: &ScimClientRecord) -> ScimClientView {
    ScimClientView {
        client_id: record.client_id.clone(),
        label: record.label.clone(),
        created_at_ms: record.created_at_ms,
        expires_at_ms: record.expires_at_ms,
    }
}

fn label_ok(label: &str) -> Result<(), IdentityRefusal> {
    if label.trim().is_empty()
        || label.chars().count() > MAX_LABEL_CHARS
        || label.chars().any(char::is_control)
    {
        return Err(IdentityRefusal::InvalidRequest);
    }
    Ok(())
}

impl IdentityStore {
    /// `list_api_keys`: one principal's keys in key-id order, without secrets. A principal
    /// with no keys, or none at all, answers the same empty list.
    pub(super) fn list_api_keys(&self, principal_id: &str) -> IdentityReply {
        IdentityReply::ApiKeys(
            self.api_keys
                .values()
                .filter(|key| key.principal_id == principal_id)
                .map(|key| ApiKeyView {
                    key_id: key.key_id.clone(),
                    principal_id: key.principal_id.clone(),
                    scopes: key.scopes.clone(),
                    created_at_ms: key.created_at_ms,
                    expires_at_ms: key.expires_at_ms,
                    last_used_at_ms: key.last_used_at_ms,
                    revoked_at_ms: key.revoked_at_ms,
                })
                .collect(),
        )
    }

    /// `issue_api_key_admin`: the same checks and the same record as `issue_api_key`, with the
    /// authority of a direct `identity:admin` actor and no session token.
    pub(super) fn issue_api_key_admin(
        &mut self,
        request: &ApiKeyAdminIssue,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let secret_hash = stamp.token_hash(0)?.to_string();
        self.insert_api_key(
            &request.principal_id,
            &request.key_id,
            &request.scopes,
            request.ttl_ms,
            secret_hash,
            stamp,
            ctx,
        )
    }

    /// `issue_admin_reset`: a one-time `AdminReset` token for an existing principal. A new
    /// token replaces the principal's outstanding ones. The reply names where the caller
    /// delivers the token through its connector; the engine delivers nothing itself. The
    /// token is spent by `redeem_one_time_token`, exactly once.
    pub(super) fn issue_admin_reset(
        &mut self,
        request: &AdminResetIssue,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        if !(1..=MAX_ONE_TIME_TOKEN_LIFETIME_MS).contains(&request.ttl_ms) {
            return Err(IdentityRefusal::InvalidRequest);
        }
        let email = self
            .users
            .get(&request.principal_id)
            .ok_or(IdentityRefusal::NotFound)?
            .email
            .clone();
        let hash = stamp.token_hash(0)?.to_string();
        self.prune_one_time(ctx.now_ms)?;
        self.one_time.retain(|_, token| {
            token.purpose != TokenPurpose::AdminReset
                || token.principal_id.as_deref() != Some(request.principal_id.as_str())
        });
        self.insert_one_time(OneTimeToken {
            token_hash: hash,
            purpose: TokenPurpose::AdminReset,
            principal_id: Some(request.principal_id.clone()),
            expires_at_ms: ctx.now_ms.saturating_add(request.ttl_ms),
            used_at_ms: None,
            created_by: stamp.actor.principal_id.clone(),
        })?;
        self.audit_event(
            stamp,
            ctx.now_ms,
            IdentityEvent::TokenIssued,
            Some(&request.principal_id),
        );
        Ok(IdentityReply::ResetDelivery(ResetDelivery { email }))
    }

    /// `revoke_session_by_handle`: revoke the session a listing shows under `handle`. A handle
    /// that names no session, or an already revoked one, answers `changed: false`.
    pub(super) fn revoke_session_by_handle(
        &mut self,
        request: &SessionHandleRef,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        if request.handle.len() != SESSION_HANDLE_CHARS || !is_lower_hex(&request.handle) {
            return Err(IdentityRefusal::InvalidRequest);
        }
        let hash = self
            .sessions
            .values()
            .find(|session| session_handle(&session.session_hash) == request.handle)
            .map(|session| session.session_hash.clone());
        let changed = hash
            .as_deref()
            .is_some_and(|hash| self.revoke_session(hash, now_ms, "admin_revoke"));
        if changed {
            self.audit_event(
                stamp,
                now_ms,
                IdentityEvent::SessionRevoked,
                Some(&request.handle),
            );
        }
        Ok(IdentityReply::Done { changed })
    }

    /// `read_issuer_keys`: every key still published at `now_ms`, the active one first.
    pub(super) fn read_issuer_keys(&self, now_ms: u64) -> IdentityReply {
        let mut keys: Vec<&IssuerKeyRecord> = self
            .issuer_keys
            .values()
            .filter(|key| key.published(now_ms))
            .collect();
        keys.sort_by(|a, b| {
            a.retired_at_ms
                .is_some()
                .cmp(&b.retired_at_ms.is_some())
                .then(b.created_at_ms.cmp(&a.created_at_ms))
                .then(a.kid.cmp(&b.kid))
        });
        IdentityReply::IssuerKeys(
            keys.into_iter()
                .map(|key| IssuerKeyView {
                    kid: key.kid.clone(),
                    algorithm: key.algorithm.clone(),
                    public_key_hex: key.public_key_hex.clone(),
                    created_at_ms: key.created_at_ms,
                    state: if key.retired_at_ms.is_some() {
                        "retired".to_string()
                    } else {
                        "active".to_string()
                    },
                    retired_at_ms: key.retired_at_ms,
                })
                .collect(),
        )
    }

    /// `rotate_issuer_key`: the new key becomes the active one and the previous active key is
    /// retired, still published for the grace period. A key past its grace is dropped here.
    pub(super) fn rotate_issuer_key(
        &mut self,
        request: &IssuerKeyRotate,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        identifier(&request.kid)?;
        let key_len = request.public_key_hex.len();
        if !ISSUER_KEY_ALGORITHMS.contains(&request.algorithm.as_str())
            || !is_lower_hex(&request.public_key_hex)
            || key_len % 2 != 0
            || !(64..=130).contains(&key_len)
        {
            return Err(IdentityRefusal::InvalidRequest);
        }
        if self.issuer_keys.contains_key(&request.kid) {
            return Err(IdentityRefusal::Collision);
        }
        self.issuer_keys.retain(|_, key| key.published(ctx.now_ms));
        if self.issuer_keys.len() >= MAX_ISSUER_KEYS {
            return Err(IdentityRefusal::Full);
        }
        for key in self.issuer_keys.values_mut() {
            key.retired_at_ms.get_or_insert(ctx.now_ms);
        }
        self.issuer_keys.insert(
            request.kid.clone(),
            IssuerKeyRecord {
                kid: request.kid.clone(),
                algorithm: request.algorithm.clone(),
                public_key_hex: request.public_key_hex.clone(),
                created_at_ms: ctx.now_ms,
                retired_at_ms: None,
            },
        );
        self.audit_event(
            stamp,
            ctx.now_ms,
            IdentityEvent::IssuerKeyRotated,
            Some(&request.kid),
        );
        Ok(self.read_issuer_keys(ctx.now_ms))
    }

    /// `create_scim_client`.
    pub(super) fn create_scim_client(
        &mut self,
        request: &ScimClientCreate,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        identifier(&request.client_id)?;
        label_ok(&request.label)?;
        if !(1..=MAX_API_KEY_LIFETIME_MS).contains(&request.ttl_ms) {
            return Err(IdentityRefusal::InvalidRequest);
        }
        if self.scim_clients.contains_key(&request.client_id) {
            return Err(IdentityRefusal::Collision);
        }
        if self.scim_clients.len() >= MAX_SCIM_CLIENTS {
            return Err(IdentityRefusal::Full);
        }
        let record = ScimClientRecord {
            client_id: request.client_id.clone(),
            label: request.label.clone(),
            secret_hash: stamp.token_hash(0)?.to_string(),
            created_at_ms: ctx.now_ms,
            expires_at_ms: ctx.now_ms.saturating_add(request.ttl_ms),
        };
        let view = scim_view(&record);
        self.scim_clients.insert(request.client_id.clone(), record);
        self.audit_event(
            stamp,
            ctx.now_ms,
            IdentityEvent::ScimClientChanged,
            Some(&request.client_id),
        );
        Ok(IdentityReply::ScimClient(view))
    }

    /// `list_scim_clients`: every client in id order, without secrets.
    pub(super) fn list_scim_clients(&self) -> IdentityReply {
        IdentityReply::ScimClients(self.scim_clients.values().map(scim_view).collect())
    }

    /// `get_scim_client`.
    pub(super) fn get_scim_client(&self, client_id: &str) -> Result<IdentityReply, IdentityRefusal> {
        self.scim_clients
            .get(client_id)
            .map(|record| IdentityReply::ScimClient(scim_view(record)))
            .ok_or(IdentityRefusal::NotFound)
    }

    /// `update_scim_client`: at least one of label, secret and lifetime changes.
    pub(super) fn update_scim_client(
        &mut self,
        request: &ScimClientUpdate,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        if request.label.is_none() && request.secret.is_none() && request.ttl_ms.is_none() {
            return Err(IdentityRefusal::InvalidRequest);
        }
        if let Some(label) = &request.label {
            label_ok(label)?;
        }
        if let Some(ttl) = request.ttl_ms {
            if !(1..=MAX_API_KEY_LIFETIME_MS).contains(&ttl) {
                return Err(IdentityRefusal::InvalidRequest);
            }
        }
        let secret_hash = request
            .secret
            .as_ref()
            .map(|_| stamp.token_hash(0).map(str::to_string))
            .transpose()?;
        let record = self
            .scim_clients
            .get_mut(&request.client_id)
            .ok_or(IdentityRefusal::NotFound)?;
        if let Some(label) = &request.label {
            record.label = label.clone();
        }
        if let Some(hash) = secret_hash {
            record.secret_hash = hash;
        }
        if let Some(ttl) = request.ttl_ms {
            record.expires_at_ms = ctx.now_ms.saturating_add(ttl);
        }
        let view = scim_view(record);
        self.audit_event(
            stamp,
            ctx.now_ms,
            IdentityEvent::ScimClientChanged,
            Some(&request.client_id),
        );
        Ok(IdentityReply::ScimClient(view))
    }

    /// `delete_scim_client`.
    pub(super) fn delete_scim_client(
        &mut self,
        client_id: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        self.scim_clients
            .remove(client_id)
            .ok_or(IdentityRefusal::NotFound)?;
        self.audit_event(stamp, now_ms, IdentityEvent::ScimClientChanged, Some(client_id));
        Ok(IdentityReply::Done { changed: true })
    }

    fn challenge_key(principal_id: &str, kind: MfaChallengeKind) -> String {
        format!("{principal_id}\0{}", kind_name(kind))
    }

    /// `issue_mfa_challenge`: store one challenge for the session's principal and kind with a
    /// five-minute lifetime from the injected clock. A new one replaces the outstanding one.
    pub(super) fn issue_mfa_challenge(
        &mut self,
        request: &MfaChallengeIssue,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let principal = self.session_subject(stamp, ctx.now_ms, PendingSession::AllowEnrollment)?;
        if request.challenge.is_empty()
            || request.challenge.chars().count() > MAX_CHALLENGE_CHARS
            || request.challenge.chars().any(char::is_control)
        {
            return Err(IdentityRefusal::InvalidRequest);
        }
        self.mfa_challenges
            .retain(|_, record| record.expires_at_ms > ctx.now_ms);
        let key = Self::challenge_key(&principal, request.kind);
        if !self.mfa_challenges.contains_key(&key) && self.mfa_challenges.len() >= MAX_MFA_CHALLENGES
        {
            return Err(IdentityRefusal::Full);
        }
        self.mfa_challenges.insert(
            key,
            MfaChallengeRecord {
                principal_id: principal,
                kind: request.kind,
                challenge: request.challenge.clone(),
                issued_at_ms: ctx.now_ms,
                expires_at_ms: ctx.now_ms.saturating_add(MFA_CHALLENGE_TTL_MS),
            },
        );
        Ok(IdentityReply::Done { changed: true })
    }

    /// `consume_mfa_challenge`: spend the stored challenge. A missing, spent or expired
    /// challenge answers the same `NotFound`; an expired one is removed.
    pub(super) fn consume_mfa_challenge(
        &mut self,
        request: &MfaChallengeConsume,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let principal = self.session_subject(stamp, ctx.now_ms, PendingSession::AllowEnrollment)?;
        let key = Self::challenge_key(&principal, request.kind);
        let record = self
            .mfa_challenges
            .remove(&key)
            .ok_or(IdentityRefusal::NotFound)?;
        if record.expires_at_ms <= ctx.now_ms {
            return Err(IdentityRefusal::NotFound);
        }
        Ok(IdentityReply::MfaChallenge(MfaChallengeView {
            kind: record.kind,
            challenge: record.challenge,
            expires_at_ms: record.expires_at_ms,
        }))
    }
}
