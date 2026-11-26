//! Delegation records for background work (MEDULLA-IDENTITY-R701).

use super::super::delegation::{
    DelegationKind, DelegationRecord, DelegationRecordWrite, DelegationStanding, DelegationState,
    MAX_DELEGATION_OPERATIONS,
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

/// An agent-run record names its run and a bounded, non-empty operation set, no tool scope, and
/// expires in the future of the engine clock.
fn agent_run_shape(record: &DelegationRecord, now_ms: u64) -> Result<(), IdentityRefusal> {
    let run_id = record.run_id.as_deref().ok_or(IdentityRefusal::InvalidRequest)?;
    bounded(run_id, MAX_NAME_BYTES)?;
    if !record.tool.is_empty()
        || record.operations.is_empty()
        || record.operations.len() > MAX_DELEGATION_OPERATIONS
        || record.expires_at <= now_ms
    {
        return Err(IdentityRefusal::InvalidRequest);
    }
    for operation in &record.operations {
        bounded(operation, MAX_NAME_BYTES)?;
    }
    Ok(())
}

/// Who a request acts as under an agent-run delegation (MEDULLA-IDENTITY-R744).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedActor {
    /// The principal the run acts for.
    pub principal: String,
    /// The roles the principal holds right now.
    pub roles: Vec<String>,
    /// The operations the delegation bounds the run to, sorted.
    pub operations: Vec<String>,
}

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
        if record.tenant.is_empty()
            || record.revoked_at.is_some()
            || !self.users.contains_key(&record.principal)
        {
            return Err(IdentityRefusal::InvalidRequest);
        }
        match record.kind {
            DelegationKind::ToolTask => {
                bounded(&record.tool, MAX_NAME_BYTES)?;
                if record.run_id.is_some() || !record.operations.is_empty() {
                    return Err(IdentityRefusal::InvalidRequest);
                }
            }
            DelegationKind::AgentRun => agent_run_shape(record, now_ms)?,
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
                .is_ok_and(|resolved| match record.kind {
                    DelegationKind::ToolTask => resolved.scopes.contains(&record.tool),
                    DelegationKind::AgentRun => record
                        .operations
                        .iter()
                        .all(|operation| resolved.scopes.contains(operation)),
                });
        Some(DelegationState {
            record: record.clone(),
            standing,
            principal_active,
            holds_tool_scope,
        })
    }

    /// The actor an agent-run delegation lets a host act as, or `None` for every reason the
    /// delegation cannot be used: no such record in `tenant` (a record of another tenant reads
    /// as absent), a record of another kind, a revoked or expired delegation, an inactive
    /// principal, or an operation the principal no longer holds. The one answer for every
    /// refusal keeps the read from being an existence oracle (MEDULLA-IDENTITY-R744).
    pub fn delegated_actor(
        &self,
        delegation_id: &str,
        tenant: &str,
        now_ms: u64,
        classifier: &dyn ScopeClassifier,
    ) -> Option<DelegatedActor> {
        let state = self.delegation_state(delegation_id, tenant, now_ms, classifier)?;
        let usable = state.record.kind == DelegationKind::AgentRun
            && state.standing == DelegationStanding::Active
            && state.principal_active
            && state.holds_tool_scope;
        if !usable {
            return None;
        }
        let resolved = self.resolve(&state.record.principal, classifier).ok()?;
        Some(DelegatedActor {
            principal: state.record.principal,
            roles: resolved.roles.into_iter().collect(),
            operations: state.record.operations.into_iter().collect(),
        })
    }
}
