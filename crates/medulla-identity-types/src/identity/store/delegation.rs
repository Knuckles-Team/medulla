//! Delegation records for background work (MEDULLA-IDENTITY-R701).

use super::super::delegation::{
    DelegationRecord, DelegationRecordWrite, DelegationStanding, DelegationState,
};
use super::super::ops::DelegationOp;
use super::super::requests_admin::ObjectRef;
use super::super::scope::ScopeClassifier;
use super::super::stamp::IdentityStamp;
use super::super::text::{bounded, identifier, MAX_NAME_BYTES};
use super::super::views::IdentityReply;
use super::super::{IdentityEvent, IdentityRefusal};
use super::{ApplyContext, IdentityStore};

/// Most delegation records retained; a write past it is refused.
pub const MAX_DELEGATIONS: usize = 100_000;

impl IdentityStore {
    pub(super) fn apply_delegation(
        &mut self,
        op: &DelegationOp,
        stamp: &IdentityStamp,
        ctx: &ApplyContext<'_>,
    ) -> Result<IdentityReply, IdentityRefusal> {
        match op {
            DelegationOp::Record { request } => self.record_delegation(request, stamp, ctx.now_ms),
            DelegationOp::Revoke { request } => self.revoke_delegation(request, stamp, ctx.now_ms),
        }
    }

    fn record_delegation(
        &mut self,
        request: &DelegationRecordWrite,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        identifier(&request.work_item_id)?;
        let record = &request.record;
        bounded(&record.tenant, MAX_NAME_BYTES)?;
        bounded(&record.tool, MAX_NAME_BYTES)?;
        if record.tenant.is_empty()
            || record.tool.is_empty()
            || record.revoked_at.is_some()
            || !self.users.contains_key(&record.principal)
        {
            return Err(IdentityRefusal::InvalidRequest);
        }
        if self.delegations.contains_key(&request.work_item_id) {
            return Err(IdentityRefusal::Collision);
        }
        if self.delegations.len() >= MAX_DELEGATIONS {
            return Err(IdentityRefusal::Full);
        }
        self.delegations
            .insert(request.work_item_id.clone(), record.clone());
        self.audit_event(
            stamp,
            now_ms,
            IdentityEvent::DelegationRecorded,
            Some(&request.work_item_id),
        );
        Ok(IdentityReply::Done { changed: true })
    }

    fn revoke_delegation(
        &mut self,
        request: &ObjectRef,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let record = self
            .delegations
            .get_mut(&request.id)
            .ok_or(IdentityRefusal::NotFound)?;
        let changed = record.revoked_at.is_none();
        if changed {
            record.revoked_at = Some(now_ms);
            self.audit_event(
                stamp,
                now_ms,
                IdentityEvent::DelegationRevoked,
                Some(&request.id),
            );
        }
        Ok(IdentityReply::Done { changed })
    }

    /// The current state of one WorkItem's delegation as seen by `tenant`. A
    /// record of another tenant is indistinguishable from an absent one.
    pub fn delegation_state(
        &self,
        work_item_id: &str,
        tenant: &str,
        now_ms: u64,
        classifier: &dyn ScopeClassifier,
    ) -> Option<DelegationState> {
        let record: &DelegationRecord = self
            .delegations
            .get(work_item_id)
            .filter(|record| record.tenant == tenant)?;
        let standing = if record.revoked_at.is_some() {
            DelegationStanding::Revoked
        } else if record.expires_at <= now_ms {
            DelegationStanding::Expired
        } else {
            DelegationStanding::Active
        };
        let principal_active = self.is_active(&record.principal);
        let holds_tool_scope = principal_active
            && self
                .resolve(&record.principal, classifier)
                .is_ok_and(|resolved| resolved.scopes.contains(&record.tool));
        Some(DelegationState {
            record: record.clone(),
            standing,
            principal_active,
            holds_tool_scope,
        })
    }
}
