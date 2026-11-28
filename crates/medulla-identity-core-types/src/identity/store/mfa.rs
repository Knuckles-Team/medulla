//! Second factors: TOTP (RFC 6238, replay-guarded) and recovery codes.
//! The boundary unsealed the secret and matched the code; the store sees
//! the matched time step bound to the exact sealed secret verified.

use super::super::audit::IdentityEvent;
use super::super::model::TotpRecord;
use super::super::ops::MfaOp;
use super::super::stamp::IdentityStamp;
use super::super::views::{AuthenticateOutcome, AuthenticateResult, IdentityReply};
use super::super::IdentityRefusal;
use super::sessions::PendingSession;
use super::throttle::account_key;
use super::{ApplyContext, IdentityStore};

impl IdentityStore {
    pub(super) fn apply_mfa(
        &mut self,
        op: &MfaOp,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        match op {
            MfaOp::RegisterWebauthn { .. }
            | MfaOp::WebauthnCredentials { .. }
            | MfaOp::VerifyWebauthn { .. }
            | MfaOp::RemoveWebauthn { .. } => self.apply_webauthn(op, stamp, ctx),
            MfaOp::Enroll { request } => self.enroll_totp(&request.tenant, stamp, ctx.now_ms),
            MfaOp::Confirm { request } => Ok(IdentityReply::Authenticate(self.confirm_totp(
                &request.tenant,
                stamp,
                ctx.now_ms,
            )?)),
            MfaOp::Verify { request } => Ok(IdentityReply::Authenticate(self.verify_totp(
                &request.tenant,
                stamp,
                ctx.now_ms,
            )?)),
            MfaOp::Disable { request } => Ok(IdentityReply::Authenticate(self.disable_totp(
                &request.tenant,
                stamp,
                ctx.now_ms,
            )?)),
            MfaOp::SetRecoveryCodes { .. } => self.set_recovery_codes(stamp, ctx.now_ms),
            MfaOp::ConsumeRecoveryCode { .. } => Ok(IdentityReply::Authenticate(
                self.consume_recovery(stamp, ctx.now_ms)?,
            )),
            MfaOp::IssueChallenge { request } => self.issue_mfa_challenge(request, stamp, ctx),
            MfaOp::ConsumeChallenge { request } => self.consume_mfa_challenge(request, stamp, ctx),
        }
    }

    /// Store a sealed secret, unconfirmed. A confirmed factor is never
    /// silently replaced.
    fn enroll_totp(
        &mut self,
        tenant: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let principal = self.mfa_subject(tenant, stamp, now_ms, PendingSession::AllowEnrollment)?;
        let sealed = stamp
            .sealed_secret
            .clone()
            .ok_or(IdentityRefusal::Unstamped)?;
        if self.totp_confirmed(&principal) {
            return Err(IdentityRefusal::Collision);
        }
        self.totp.insert(
            principal.clone(),
            TotpRecord {
                sealed_secret: sealed,
                confirmed_at_ms: None,
                last_step: 0,
            },
        );
        self.audit_event(stamp, now_ms, IdentityEvent::MfaEnrolled, Some(&principal));
        Ok(IdentityReply::Done { changed: true })
    }

    /// Accept `step` for `principal` once: later than every accepted step.
    fn accept_step(
        &mut self,
        principal: &str,
        stamp: &IdentityStamp,
        step: u64,
    ) -> Result<(), IdentityRefusal> {
        let record = self
            .totp
            .get_mut(principal)
            .ok_or(IdentityRefusal::NotFound)?;
        if stamp.sealed_secret.as_deref() != Some(record.sealed_secret.as_str()) {
            return Err(IdentityRefusal::Unstamped);
        }
        if step <= record.last_step {
            return Err(IdentityRefusal::MfaCodeReplayed);
        }
        record.last_step = step;
        Ok(())
    }

    /// The principal of the session in `stamp`, provided the session was opened under `tenant`.
    /// An absent, dead or foreign-tenant session answers the same refusal, so the answer is no
    /// oracle. A session with no recorded tenant is bound to the deployment tenant the boundary
    /// stamped.
    fn mfa_subject(
        &self,
        tenant: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
        pending: PendingSession,
    ) -> Result<String, IdentityRefusal> {
        self.mfa_tenant_guard(tenant, stamp)?;
        self.session_subject(stamp, now_ms, pending)
    }

    fn mfa_tenant_guard(&self, tenant: &str, stamp: &IdentityStamp) -> Result<(), IdentityRefusal> {
        let session = self
            .sessions
            .get(stamp.token_hash(0)?)
            .ok_or(IdentityRefusal::NotAuthorized)?;
        let bound = session
            .session_tenant
            .as_deref()
            .or(stamp.session_tenant.as_deref());
        if bound == Some(tenant) {
            Ok(())
        } else {
            Err(IdentityRefusal::NotAuthorized)
        }
    }

    /// Check the stamped code of `principal`: the already-built throttled outcome while the
    /// principal's backoff runs, a counted failure for a wrong code, and otherwise the accepted
    /// step (a replayed one is refused and changes nothing). `Ok(None)` means the code was
    /// accepted.
    fn check_totp_code(
        &mut self,
        principal: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<Option<AuthenticateResult>, IdentityRefusal> {
        if let Some(throttled) = self.second_factor_throttled(principal, now_ms) {
            return Ok(Some(throttled));
        }
        let Some(step) = stamp.totp_step else {
            return Ok(Some(self.second_factor_failed(stamp, principal, now_ms)));
        };
        self.accept_step(principal, stamp, step)?;
        Ok(None)
    }

    fn confirm_totp(
        &mut self,
        tenant: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<AuthenticateResult, IdentityRefusal> {
        let principal = self.mfa_subject(tenant, stamp, now_ms, PendingSession::AllowEnrollment)?;
        if self.totp_confirmed(&principal) {
            return Err(IdentityRefusal::Collision);
        }
        if !self.totp.contains_key(&principal) {
            return Err(IdentityRefusal::MfaEnrollmentRequired);
        }
        if let Some(refused) = self.check_totp_code(&principal, stamp, now_ms)? {
            return Ok(refused);
        }
        if let Some(record) = self.totp.get_mut(&principal) {
            record.confirmed_at_ms.get_or_insert(now_ms);
        }
        self.clear_throttle(&account_key(&principal));
        self.audit_event(stamp, now_ms, IdentityEvent::MfaVerified, Some(&principal));
        Ok(self.second_factor_ok(principal))
    }

    /// Remove the confirmed factor of the session's principal. The session must be complete and
    /// the code a fresh step (one later than every accepted step).
    fn disable_totp(
        &mut self,
        tenant: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<AuthenticateResult, IdentityRefusal> {
        let principal = self.mfa_subject(tenant, stamp, now_ms, PendingSession::Refuse)?;
        if !self.totp_confirmed(&principal) {
            return Err(IdentityRefusal::MfaEnrollmentRequired);
        }
        if let Some(refused) = self.check_totp_code(&principal, stamp, now_ms)? {
            return Ok(refused);
        }
        self.totp.remove(&principal);
        self.clear_throttle(&account_key(&principal));
        self.audit_event(stamp, now_ms, IdentityEvent::MfaRemoved, Some(&principal));
        Ok(self.second_factor_ok(principal))
    }

    /// The principal of a live session still owing its second factor.
    pub(super) fn pending_session_principal(
        &self,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<(String, String), IdentityRefusal> {
        let hash = stamp.token_hash(0)?;
        let session = self
            .sessions
            .get(hash)
            .filter(|session| self.session_admissible(session, now_ms) && session.mfa_pending)
            .ok_or(IdentityRefusal::NotFound)?;
        Ok((hash.to_string(), session.principal_id.clone()))
    }

    /// The common entry of every second-factor verification (TOTP,
    /// WebAuthn, recovery): resolve the pending session's `(hash, principal)`, answer
    /// the already-built throttled outcome if the account backoff is
    /// running, or else hand both to `verify` for the factor's own check.
    pub(super) fn with_second_factor_gate(
        &mut self,
        stamp: &IdentityStamp,
        now_ms: u64,
        verify: impl FnOnce(&mut Self, String, String) -> Result<AuthenticateResult, IdentityRefusal>,
    ) -> Result<AuthenticateResult, IdentityRefusal> {
        let (hash, principal) = self.pending_session_principal(stamp, now_ms)?;
        if let Some(throttled) = self.second_factor_throttled(&principal, now_ms) {
            return Ok(throttled);
        }
        verify(self, hash, principal)
    }

    /// Mark a pending session complete by `method`.
    pub(super) fn complete_session(&mut self, hash: &str, method: &str, now_ms: u64) {
        if let Some(session) = self.sessions.get_mut(hash) {
            session.mfa_pending = false;
            session.mfa_at_ms = Some(now_ms);
            session.auth_methods.push(method.to_string());
        }
    }

    /// A wrong second factor answers `bad` (not a refusal), so the failure
    /// count it adds is kept.
    pub(super) fn second_factor_failed(
        &mut self,
        stamp: &IdentityStamp,
        principal: &str,
        now_ms: u64,
    ) -> AuthenticateResult {
        self.record_failure(account_key(principal), now_ms);
        self.audit_event(stamp, now_ms, IdentityEvent::MfaFailed, Some(principal));
        AuthenticateResult::bad()
    }

    /// `Throttled` while `principal`'s account backoff runs.
    pub(super) fn second_factor_throttled(
        &self,
        principal: &str,
        now_ms: u64,
    ) -> Option<AuthenticateResult> {
        let until = self.throttled_until(&account_key(principal), now_ms)?;
        Some(AuthenticateResult {
            outcome: AuthenticateOutcome::Throttled,
            principal_id: None,
            retry_after_ms: Some(until - now_ms),
        })
    }

    pub(super) fn second_factor_ok(&self, principal: String) -> AuthenticateResult {
        AuthenticateResult {
            outcome: AuthenticateOutcome::Ok,
            principal_id: Some(principal),
            retry_after_ms: None,
        }
    }

    fn verify_totp(
        &mut self,
        tenant: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<AuthenticateResult, IdentityRefusal> {
        self.mfa_tenant_guard(tenant, stamp)?;
        self.with_second_factor_gate(stamp, now_ms, |store, hash, principal| {
            if !store.totp_confirmed(&principal) {
                return Err(IdentityRefusal::MfaEnrollmentRequired);
            }
            let Some(step) = stamp.totp_step else {
                return Ok(store.second_factor_failed(stamp, &principal, now_ms));
            };
            store.accept_step(&principal, stamp, step)?;
            store.complete_session(&hash, "totp", now_ms);
            store.clear_throttle(&account_key(&principal));
            store.audit_event(stamp, now_ms, IdentityEvent::MfaVerified, Some(&principal));
            Ok(store.second_factor_ok(principal))
        })
    }
}
