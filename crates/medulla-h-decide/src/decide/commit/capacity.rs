//! MEDULLA-DECISION-ENGINE-R113.2.5: the capacity-lease participant of a topology `DecisionCommit`.
//!
//! The lease is the checked PREPARE of the commit's lease participant: it runs after the owners
//! reserved and before the vote, through the capacity ledger's own acquire (`AcquireCapacity`
//! through `PersistenceBackend::commit_capacity_lease`). One request carries every cell of the
//! plan, so the ledger grants all of them or none. The request is idempotent on the
//! coordinator transaction id, so a resumed commit replays the same leases instead of taking
//! them twice. A denial or any later ABORT releases what was taken, under the same key.

use std::sync::Arc;

use medulla_decision_types::decision::DecisionRecord;
use medulla_protocol_types::protocol::Method;
use medulla_raft_xshard::cross_shard_txn::certificate::OwnerDecision;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::EngineErrorCode;
use medulla_work_types::native_control::{
    CapacityAcquireRequest, CapacityAcquireResult, CapacityDecision, CapacityDemand,
    CapacityLeaseFence, CapacityLeaseMutationRequest, CapacityMutationResult,
    NativeControlSchemaVersion,
};

use super::super::SharedState;

/// Further decisions one denial allows (MEDULLA-DECISION-ENGINE-R113.1 `RetryAllowance`).
const DENIAL_RETRY_ALLOWANCE: u32 = 1;

/// The positive-amount lease demands of a solved topology record, in plan order.
pub(in crate::decide) fn demands(record: &DecisionRecord) -> Vec<CapacityDemand> {
    record
        .topology_plan()
        .map(|plan| {
            plan.lease
                .per_cell
                .iter()
                .filter(|cell| cell.amount > 0)
                .map(|cell| CapacityDemand {
                    cell_id: cell.cell_id.clone(),
                    resource_class: cell.class,
                    amount: cell.amount,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The leases one commit holds in the capacity ledger.
pub(in crate::decide) struct LeaseHold {
    backend: Arc<dyn PersistenceBackend>,
    ledger: String,
    tenant: String,
    owner_digest: String,
    txn_id: String,
    fences: Vec<CapacityLeaseFence>,
}

/// What the lease participant answered.
pub(in crate::decide) enum Prepared {
    /// Every cell was granted (or replayed from an earlier attempt of this transaction).
    Held(LeaseHold),
    /// The ledger refused: nothing is held.
    Denied(String),
}

impl LeaseHold {
    /// The lease ids held, in cell order.
    pub(in crate::decide) fn lease_ids(&self) -> Vec<String> {
        self.fences.iter().map(|f| f.lease_id.clone()).collect()
    }

    /// Acquire every demand in the ledger graph `capacity_graph`.
    pub(in crate::decide) async fn acquire(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        record: &DecisionRecord,
        capacity_graph: &str,
        txn_id: &str,
    ) -> Result<Prepared, String> {
        let backend = state.read().await.persistence.clone().ok_or_else(|| {
            format!(
                "{}: a topology commit needs the durable capacity ledger",
                EngineErrorCode::CapacityDenied.as_str()
            )
        })?;
        let plan_priority = record
            .topology_plan()
            .map(|plan| plan.lease.priority)
            .ok_or_else(|| "the record carries no topology plan".to_string())?;
        let ledger = medulla_kernel::persist::sanitize(capacity_graph);
        let owner_digest = verified.principal_persistence_id();
        let request = CapacityAcquireRequest {
            schema_version: NativeControlSchemaVersion::V1,
            tenant_ref: verified.tenant().to_string(),
            work_item_id: record.record_id.clone(),
            owner_digest: owner_digest.clone(),
            idempotency_key: txn_id.to_string(),
            priority: plan_priority,
            demands: demands(record),
            lease_id: None,
            ttl_ms: medulla_server_api::knob_config::knobs().topology_lease_ttl_ms,
            now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
            cost_budget_micros: None,
            token_budget: None,
        };
        let bytes = backend
            .commit_capacity_lease(&ledger, Method::AcquireCapacity { request })
            .await?;
        let result: CapacityAcquireResult = rmp_serde::from_slice(&bytes)
            .map_err(|error| format!("capacity acquire receipt is unreadable: {error}"))?;
        match result.decision {
            CapacityDecision::Accepted | CapacityDecision::Replayed => {
                let fences = result
                    .leases
                    .iter()
                    .map(|lease| CapacityLeaseFence {
                        lease_id: lease.lease_id.clone(),
                        lease_epoch: lease.lease_epoch,
                        fence_token: lease.fence_token,
                    })
                    .collect();
                Ok(Prepared::Held(Self {
                    backend,
                    ledger,
                    tenant: verified.tenant().to_string(),
                    owner_digest,
                    txn_id: txn_id.to_string(),
                    fences,
                }))
            }
            other => Ok(Prepared::Denied(format!(
                "{}: capacity was denied ({other:?}); one further decision is allowed (retry allowance {DENIAL_RETRY_ALLOWANCE})",
                EngineErrorCode::CapacityDenied.as_str()
            ))),
        }
    }

    /// Resolve the leases with the durable coordinator decision: COMMIT keeps them, ABORT
    /// releases them under the same transaction key.
    pub(in crate::decide) async fn resolve_decided(
        self,
        decision: OwnerDecision,
    ) -> Result<(), String> {
        if decision == OwnerDecision::Commit {
            return Ok(());
        }
        self.release().await
    }

    /// Release every lease this transaction holds.
    pub(in crate::decide) async fn release(self) -> Result<(), String> {
        let request = CapacityLeaseMutationRequest {
            schema_version: NativeControlSchemaVersion::V1,
            tenant_ref: self.tenant.clone(),
            owner_digest: self.owner_digest.clone(),
            leases: self.fences.clone(),
            now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
            ttl_ms: None,
            idempotency_key: Some(format!("release:{}", self.txn_id)),
        };
        let bytes = self
            .backend
            .commit_capacity_lease(&self.ledger, Method::ReleaseCapacity { request })
            .await?;
        let result: CapacityMutationResult = rmp_serde::from_slice(&bytes)
            .map_err(|error| format!("capacity release receipt is unreadable: {error}"))?;
        match result.decision {
            CapacityDecision::Released | CapacityDecision::Replayed => Ok(()),
            other => Err(format!("capacity release was refused: {other:?}")),
        }
    }
}

/// The ledger graph a topology commit draws from must be a graph the caller may write: a
/// capacity cell is a resource of its graph, so a graph of another tenant is refused as unknown.
pub(in crate::decide) async fn check_ledger_access(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    capacity_graph: &str,
) -> Result<(), String> {
    let s = state.read().await;
    let entry = s
        .registry
        .get(capacity_graph)
        .ok_or_else(|| format!("Graph '{capacity_graph}' not found"))?;
    medulla_server_auth::access::check_graph_access(
        &s.isolation,
        Some(verified.agent_id()),
        capacity_graph,
        entry.graph_type,
        entry.owner.as_deref(),
        medulla_isolation::isolation::AccessLevel::Write,
    )
}
