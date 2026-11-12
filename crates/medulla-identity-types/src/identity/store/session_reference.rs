//! Inspection of the original durable reference, without credential issuance.

use super::{ApplyContext, IdentityStore};
use crate::identity::{
    IdentityRefusal, IdentityReply, IdentityStamp, SessionLiveness, SessionRecord,
    SessionRecordProjection, UserKind,
};

impl IdentityStore {
    pub(super) fn renew_session_reference(
        &mut self,
        reference: &str,
        tenant: &str,
        expected_scopes: &std::collections::BTreeSet<String>,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        self.inspect_session_reference(reference, tenant, stamp, ctx)?;
        let scopes = self
            .resolve(&stamp.actor.principal_id, ctx.classifier)?
            .scopes;
        if &scopes != expected_scopes {
            return Err(IdentityRefusal::PreconditionFailed);
        }
        let idle = self.idle_ms_for(&scopes, ctx);
        let session = self
            .sessions
            .values_mut()
            .find(|session| session.session_ref.as_deref() == Some(reference))
            .ok_or(IdentityRefusal::NotFound)?;
        let ceiling = session
            .original_scopes
            .as_ref()
            .ok_or(IdentityRefusal::PreconditionFailed)?;
        if !scopes.is_subset(ceiling) {
            return Err(IdentityRefusal::PreconditionFailed);
        }
        let expiry = ctx
            .now_ms
            .saturating_add(idle)
            .min(session.absolute_expires_at_ms);
        if expiry <= session.idle_expires_at_ms {
            return Err(IdentityRefusal::PreconditionFailed);
        }
        session.last_seen_at_ms = ctx.now_ms;
        session.idle_expires_at_ms = expiry;
        self.inspect_session_reference(reference, tenant, stamp, ctx)
    }

    fn unique_session_reference(&self, reference: &str) -> Result<&SessionRecord, IdentityRefusal> {
        let mut matches = self
            .sessions
            .values()
            .filter(|session| session.session_ref.as_deref() == Some(reference));
        let session = matches.next().ok_or(IdentityRefusal::NotFound)?;
        // An ambiguous stored reference must never select an arbitrary session.
        if matches.next().is_some() {
            return Err(IdentityRefusal::NotAuthorized);
        }
        Ok(session)
    }

    pub(super) fn inspect_session_reference(
        &self,
        reference: &str,
        tenant: &str,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        if stamp.actor.delegated
            || reference.is_empty()
            || stamp.session_tenant.as_deref() != Some(tenant)
        {
            return Err(IdentityRefusal::NotAuthorized);
        }
        let session = self.unique_session_reference(reference)?;
        if session.session_tenant.as_deref() != Some(tenant)
            || session.principal_id != stamp.actor.principal_id
            || self
                .users
                .get(&session.principal_id)
                .is_none_or(|user| user.kind != UserKind::Human)
        {
            return Err(IdentityRefusal::NotAuthorized);
        }
        if !self.session_admissible(session, ctx.now_ms)
            || session.mfa_pending
            || !self.is_active(&session.principal_id)
        {
            return Err(IdentityRefusal::NotFound);
        }
        Ok(IdentityReply::SessionRecord(SessionRecordProjection {
            session_ref: reference.to_string(),
            tenant: tenant.to_string(),
            principal_id: session.principal_id.clone(),
            absolute_expires_at_ms: session.absolute_expires_at_ms,
            idle_expires_at_ms: session.idle_expires_at_ms,
            mfa_at_ms: session.mfa_at_ms,
        }))
    }

    /// The stored state of one session reference, judged from stored expiry and
    /// revocation only. Reads nothing but the reference and the tenant: it
    /// never takes or returns a token and never touches last-seen or expiry.
    /// An API-key id or an external-identity link key answers
    /// `not_session_bound`; any other unknown reference, or one bound to a
    /// different tenant than the request and the caller's deployment tenant,
    /// refuses `SESSION_REFERENCE_UNKNOWN`.
    pub(super) fn session_liveness(
        &self,
        reference: &str,
        tenant: &str,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<SessionLiveness, IdentityRefusal> {
        if reference.is_empty()
            || stamp
                .session_tenant
                .as_deref()
                .is_some_and(|caller| caller != tenant)
        {
            return Err(IdentityRefusal::SessionReferenceUnknown);
        }
        let mut matches = self
            .sessions
            .values()
            .filter(|session| session.session_ref.as_deref() == Some(reference));
        let Some(session) = matches.next() else {
            if self.api_keys.contains_key(reference) || self.links.contains_key(reference) {
                return Ok(SessionLiveness::NotSessionBound);
            }
            return Err(IdentityRefusal::SessionReferenceUnknown);
        };
        if matches.next().is_some() || session.session_tenant.as_deref() != Some(tenant) {
            return Err(IdentityRefusal::SessionReferenceUnknown);
        }
        Ok(if session.revoked_at_ms.is_some() {
            SessionLiveness::Revoked
        } else if self.session_admissible(session, now_ms) {
            SessionLiveness::Live
        } else {
            SessionLiveness::Expired
        })
    }
}
