//! Wire logins in the identity store (MEDULLA-ENGINE-PARITY-R007.4).
//!
//! The record maps one SQL-wire login to one engine principal. It is written only through
//! `IdentityOp::WireLogin`, inside the same identity write that appends its audit entry, and is
//! never deleted: a revoke moves the `wire_login` machine to `Revoked`.

use super::super::audit::AuditRecord;
use super::super::ops::WireLoginOp;
use super::super::requests_admin::{WireLoginGrant, WireLoginRevoke};
use super::super::scope::ScopeClassifier;
use super::super::stamp::IdentityStamp;
use super::super::text::{bounded, MAX_NAME_BYTES};
use super::super::validate_principal_id;
use super::super::views::IdentityReply;
use super::super::wire_login::{
    verifier_is_well_formed, WireLoginEvent, WireLoginGranted, WireLoginKind, WireLoginRecord,
    WireLoginRevoked, MAX_WIRE_LOGIN_SCOPES,
};
use super::super::{IdentityEvent, IdentityRefusal};
use super::{ApplyContext, IdentityStore};
use std::collections::BTreeSet;

/// Most wire logins (revoked ones included) one store holds.
pub const MAX_WIRE_LOGINS: usize = 10_000;

impl IdentityStore {
    /// The stored record of `login`, active or revoked.
    pub fn wire_login(&self, login: &str) -> Option<&WireLoginRecord> {
        self.wire_logins.get(login)
    }

    /// Store `record` unchanged, bypassing the op path (the fixture seam of the in-process pgwire
    /// listener tests, which have no initialized identity store).
    #[cfg(any(test, feature = "test-support"))]
    pub fn seed_wire_login(&mut self, record: WireLoginRecord) {
        self.wire_logins.insert(record.login.clone(), record);
    }

    pub(super) fn apply_wire_login(
        &mut self,
        op: &WireLoginOp,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        match op {
            WireLoginOp::Grant { request } => {
                self.grant_wire_login(request, stamp, ctx.now_ms, ctx.classifier)
            }
            WireLoginOp::Revoke { request } => self.revoke_wire_login(request, stamp, ctx.now_ms),
        }
    }

    fn grant_wire_login(
        &mut self,
        request: &WireLoginGrant,
        stamp: &IdentityStamp,
        now_ms: u64,
        classifier: &dyn ScopeClassifier,
    ) -> Result<IdentityReply, IdentityRefusal> {
        bounded(&request.login, MAX_NAME_BYTES)?;
        bounded(&request.principal, MAX_NAME_BYTES)?;
        // A login never maps to a reserved engine service agent id.
        validate_principal_id(&request.principal)?;
        let scopes = checked_scopes(&request.scopes, classifier)?;
        if !verifier_is_well_formed(request.kind, &request.verifier) {
            return Err(IdentityRefusal::InvalidRequest);
        }
        // The record's tenant is the verified actor's, never a request field.
        let tenant = stamp
            .session_tenant
            .as_deref()
            .ok_or(IdentityRefusal::NotAuthorized)?;
        if let Some(existing) = self.wire_logins.get(&request.login) {
            let same = existing.principal == request.principal
                && existing.kind == request.kind
                && existing.verifier == request.verifier
                && existing.scopes == scopes;
            return if same {
                Ok(IdentityReply::WireLoginGranted(existing.into()))
            } else {
                Err(IdentityRefusal::WireLoginExists)
            };
        }
        if self.wire_logins.len() >= MAX_WIRE_LOGINS {
            return Err(IdentityRefusal::Full);
        }
        let record = WireLoginRecord::new(
            &request.login,
            request.kind,
            &request.verifier,
            &request.principal,
            tenant,
            now_ms,
        )
        .with_scopes(scopes);
        let granted = WireLoginGranted::from(&record);
        let record_key = (request.login.clone(), request.kind);
        self.wire_logins.insert(request.login.clone(), record);
        self.audit_wire_login(stamp, now_ms, IdentityEvent::WireLoginGranted, &record_key);
        Ok(IdentityReply::WireLoginGranted(granted))
    }

    /// Append the audit entry of a wire login change: the actor, the login and its kind.
    fn audit_wire_login(
        &mut self,
        stamp: &IdentityStamp,
        now_ms: u64,
        event: IdentityEvent,
        (login, kind): &(String, WireLoginKind),
    ) {
        self.audit.append(AuditRecord {
            at_ms: now_ms,
            actor: stamp.actor.principal_id.clone(),
            event,
            target: Some(login.clone()),
            ip_prefix: None,
            detail: kind.as_str().to_string(),
            tenant: stamp.session_tenant.clone(),
            authorization_digest: None,
        });
    }

    fn revoke_wire_login(
        &mut self,
        request: &WireLoginRevoke,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let record = self
            .wire_logins
            .get_mut(&request.login)
            .ok_or(IdentityRefusal::WireLoginUnknown)?;
        if let Some(revoked_ms) = record.revoked_ms {
            return Ok(IdentityReply::WireLoginRevoked(WireLoginRevoked {
                login: request.login.clone(),
                revoked_ms,
            }));
        }
        record
            .apply(WireLoginEvent::Revoke { now_ms })
            .map_err(|_| IdentityRefusal::IllegalTransition)?;
        let record_key = (request.login.clone(), record.kind);
        self.audit_wire_login(stamp, now_ms, IdentityEvent::WireLoginRevoked, &record_key);
        Ok(IdentityReply::WireLoginRevoked(WireLoginRevoked {
            login: request.login.clone(),
            revoked_ms: now_ms,
        }))
    }
}

/// The scopes of a grant, sorted and de-duplicated: non-empty, bounded in count and size, every
/// one registered, and none of a class a bearer login cannot carry (administration and approvals).
fn checked_scopes(
    requested: &[String],
    classifier: &dyn ScopeClassifier,
) -> Result<Vec<String>, IdentityRefusal> {
    if requested.is_empty() || requested.len() > MAX_WIRE_LOGIN_SCOPES {
        return Err(IdentityRefusal::InvalidRequest);
    }
    let mut scopes = BTreeSet::new();
    for scope in requested {
        bounded(scope, MAX_NAME_BYTES)?;
        let class = classifier
            .class_of(scope)
            .ok_or(IdentityRefusal::UnknownScope)?;
        if !class.allows_api_key() {
            return Err(IdentityRefusal::ClassViolation);
        }
        scopes.insert(scope.clone());
    }
    Ok(scopes.into_iter().collect())
}
