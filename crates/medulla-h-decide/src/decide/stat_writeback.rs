//! `DecisionLog.writeback`, dispatched (MEDULLA-DECISION-ENGINE-R615.2.1,
//! R615.2.2, R615.3): the write-back preflight, the creation and ticket
//! proposals, and the served approval queue.
//!
//! Preflight and propose compute from the request alone and classify as reads.
//! Queue, approve and release store one [`StoredQueue`] per queue id in the
//! tenant's decision artifacts; its lifecycle is the machine `proposal_queue`
//! (`queue_transition` is the only writer of its state). The rules live in
//! `medulla_decision_stats::decision::writeback`.

use std::collections::BTreeSet;

use medulla_decision_stats::decision::writeback::{
    creation_proposals, preflight, queue_transition, ticket_proposals, InventoryItem, Preflight,
    ProposalQueue, QueueEvent, QueueState, RiskFinding, WritebackError, WritebackProposal,
    WRITEBACK_BELOW_QUEUE_TIER, WRITEBACK_PROPOSAL_UNKNOWN, WRITEBACK_QUEUE_EXISTS,
    WRITEBACK_QUEUE_TOO_LARGE, WRITEBACK_QUEUE_UNKNOWN,
};
use medulla_decision_types::decision::statistical::writeback::{
    GovernedChangeSet, PreflightVerdict, ProposalInput, ProposalKind, ProposalRow,
    WritebackApproval, WritebackOp, WritebackOutcome,
};
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::{ResourceId, TenantId};
use serde::{Deserialize, Serialize};

use super::stat_executor::ExecutionContext;
use super::stat_log::LogReader;

/// One stored approval queue: the queue, its approvals in approval order and
/// its lifecycle state, persisted as one record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredQueue {
    queue: ProposalQueue,
    approvals: Vec<WritebackApproval>,
    state: QueueState,
}

fn queue_key(queue_id: &str) -> String {
    format!("writeback_queue:{queue_id}")
}

fn read_queue(
    ctx: &ExecutionContext,
    tenant: &str,
    queue_id: &str,
) -> Result<(Vec<u8>, StoredQueue), String> {
    let bytes = ctx
        .store
        .decision_artifact(tenant, &queue_key(queue_id))?
        .ok_or_else(|| {
            refused(WritebackError::refusal(
                WRITEBACK_QUEUE_UNKNOWN,
                format!("tenant holds no approval queue {queue_id}"),
            ))
        })?;
    let stored = decode_artifact(&bytes, "write-back queue")?;
    Ok((bytes, stored))
}

fn input_of(proposal: &WritebackProposal) -> ProposalInput {
    ProposalInput {
        id: proposal.id.as_str().to_string(),
        external_id: proposal.external_id.as_str().to_string(),
        system_of_record: proposal.system_of_record.as_str().to_string(),
        risk_tier: proposal.risk_tier,
        as_of_ms: proposal.as_of_ms,
    }
}

fn fully_approved(stored: &StoredQueue) -> bool {
    stored.queue.proposals.iter().all(|p| {
        stored
            .approvals
            .iter()
            .any(|a| a.proposal_id == p.id.as_str())
    })
}

fn change_set(stored: &StoredQueue) -> GovernedChangeSet {
    GovernedChangeSet {
        change_set_id: format!("changeset:{}", stored.queue.id.as_str()),
        proposals: stored.queue.proposals.iter().map(input_of).collect(),
        approvals: stored.approvals.clone(),
    }
}

fn queue(
    ctx: &ExecutionContext,
    tenant: &str,
    queue_id: &str,
    proposals: Vec<medulla_decision_types::decision::statistical::writeback::QueuedProposalInput>,
    min_tier: u64,
    max_proposals: u64,
) -> Result<WritebackOutcome, String> {
    if proposals.is_empty() || proposals.len() as u64 > max_proposals {
        return Err(refused(WritebackError::refusal(
            WRITEBACK_QUEUE_TOO_LARGE,
            format!(
                "a queue holds 1 to {max_proposals} proposals, got {}",
                proposals.len()
            ),
        )));
    }
    let mut checked = Vec::with_capacity(proposals.len());
    for queued in proposals {
        let p = queued.proposal;
        let proposal = WritebackProposal::new(
            p.id,
            tenant,
            p.external_id,
            p.system_of_record,
            p.risk_tier,
            p.as_of_ms,
        )
        .map_err(refused)?;
        match preflight(&proposal, &queued.source_values, min_tier) {
            Preflight::Refuse(error) => return Err(refused(error)),
            Preflight::Proceed => {
                return Err(refused(WritebackError::refusal(
                    WRITEBACK_BELOW_QUEUE_TIER,
                    format!(
                        "proposal {} is below the approval tier and proceeds without a queue",
                        proposal.id.as_str()
                    ),
                )))
            }
            Preflight::Queue => checked.push(proposal),
        }
    }
    let proposal_ids = checked.iter().map(|p| p.id.as_str().to_string()).collect();
    let queue = ProposalQueue::new(queue_id, tenant, checked).map_err(refused)?;
    if ctx
        .store
        .decision_artifact(tenant, &queue_key(queue_id))?
        .is_some()
    {
        return Err(refused(WritebackError::refusal(
            WRITEBACK_QUEUE_EXISTS,
            format!("approval queue {queue_id} is already stored"),
        )));
    }
    let stored = StoredQueue {
        queue,
        approvals: Vec::new(),
        state: QueueState::Pending,
    };
    ctx.store.swap_decision_artifact(
        tenant,
        &queue_key(queue_id),
        None,
        encode_artifact(&stored)?,
    )?;
    Ok(WritebackOutcome::Queued {
        queue_id: queue_id.to_string(),
        proposal_ids,
    })
}

fn approve(
    ctx: &ExecutionContext,
    reader: &LogReader,
    tenant: &str,
    queue_id: &str,
    proposal_id: &str,
) -> Result<WritebackOutcome, String> {
    let (bytes, mut stored) = read_queue(ctx, tenant, queue_id)?;
    stored.state = queue_transition(stored.state, QueueEvent::Approve).map_err(refused)?;
    if !stored
        .queue
        .proposals
        .iter()
        .any(|p| p.id.as_str() == proposal_id)
    {
        return Err(refused(WritebackError::refusal(
            WRITEBACK_PROPOSAL_UNKNOWN,
            format!("queue {queue_id} holds no proposal {proposal_id}"),
        )));
    }
    // Approving twice keeps the first approval: the approver and instant stand.
    let approval = match stored
        .approvals
        .iter()
        .find(|a| a.proposal_id == proposal_id)
    {
        Some(existing) => existing.clone(),
        None => {
            let approval = WritebackApproval {
                proposal_id: proposal_id.to_string(),
                approver: reader.principal.clone(),
                approved_at_ms: i64::try_from(ctx.now_ms).unwrap_or(i64::MAX),
            };
            stored.approvals.push(approval.clone());
            ctx.store.swap_decision_artifact(
                tenant,
                &queue_key(queue_id),
                Some(bytes),
                encode_artifact(&stored)?,
            )?;
            approval
        }
    };
    Ok(WritebackOutcome::Approved { approval })
}

fn release(
    ctx: &ExecutionContext,
    tenant: &str,
    queue_id: &str,
) -> Result<WritebackOutcome, String> {
    let (bytes, mut stored) = read_queue(ctx, tenant, queue_id)?;
    let before = stored.state;
    let event = QueueEvent::Release {
        fully_approved: fully_approved(&stored),
    };
    stored.state = queue_transition(before, event).map_err(refused)?;
    if stored.state != before {
        ctx.store.swap_decision_artifact(
            tenant,
            &queue_key(queue_id),
            Some(bytes),
            encode_artifact(&stored)?,
        )?;
    }
    Ok(WritebackOutcome::Released {
        change_set: change_set(&stored),
    })
}

fn invalid(error: impl std::fmt::Display) -> String {
    format!("PARAMETER_INVALID: {error}")
}

fn refused(error: WritebackError) -> String {
    error.to_string()
}

fn resource(value: &str) -> Result<ResourceId, String> {
    ResourceId::new(value).map_err(|detail| invalid(format!("{value:?}: {detail}")))
}

/// Serve one write-back operation for `tenant`.
pub(super) fn serve(
    ctx: &ExecutionContext,
    reader: &LogReader,
    tenant: &str,
    op: WritebackOp,
) -> Result<WritebackOutcome, String> {
    let knobs = medulla_server_api::knob_config::knobs();
    match op {
        WritebackOp::Queue {
            queue_id,
            proposals,
        } => queue(
            ctx,
            tenant,
            &queue_id,
            proposals,
            knobs.writeback_queue_min_tier,
            knobs.writeback_queue_max_proposals,
        ),
        WritebackOp::Approve {
            queue_id,
            proposal_id,
        } => approve(ctx, reader, tenant, &queue_id, &proposal_id),
        WritebackOp::Release { queue_id } => release(ctx, tenant, &queue_id),
        computed => run(tenant, computed, knobs.writeback_queue_min_tier),
    }
}

fn run(tenant: &str, op: WritebackOp, min_tier: u64) -> Result<WritebackOutcome, String> {
    let tenant_id = TenantId::new(tenant).map_err(invalid)?;
    match op {
        WritebackOp::Preflight {
            proposal,
            source_values,
        } => {
            let proposal = WritebackProposal::new(
                proposal.id,
                tenant,
                proposal.external_id,
                proposal.system_of_record,
                proposal.risk_tier,
                proposal.as_of_ms,
            )
            .map_err(refused)?;
            let verdict = match preflight(&proposal, &source_values, min_tier) {
                Preflight::Refuse(error) => PreflightVerdict::Refuse {
                    code: error.code.to_string(),
                    detail: error.detail,
                },
                Preflight::Queue => PreflightVerdict::Queue,
                Preflight::Proceed => PreflightVerdict::Proceed,
            };
            Ok(WritebackOutcome::Preflight { verdict })
        }
        WritebackOp::Propose {
            inventory,
            enabled_systems,
            findings,
        } => {
            let systems = enabled_systems
                .iter()
                .map(|s| resource(s))
                .collect::<Result<Vec<_>, _>>()?;
            let items = inventory
                .iter()
                .map(|item| {
                    Ok(InventoryItem {
                        tenant: tenant_id.clone(),
                        external_id: resource(&item.external_id)?,
                        present_in: item
                            .present_in
                            .iter()
                            .map(|s| resource(s))
                            .collect::<Result<BTreeSet<_>, String>>()?,
                        as_of_ms: item.as_of_ms,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let risks = findings
                .iter()
                .map(|f| {
                    Ok(RiskFinding {
                        tenant: tenant_id.clone(),
                        finding_id: resource(&f.finding_id)?,
                        external_id: resource(&f.external_id)?,
                        system_of_record: resource(&f.system_of_record)?,
                        risk_tier: f.risk_tier,
                        as_of_ms: f.as_of_ms,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let mut proposals: Vec<ProposalRow> = creation_proposals(&items, &systems)
                .map_err(refused)?
                .into_iter()
                .map(|c| ProposalRow {
                    kind: ProposalKind::InventoryCreation,
                    id: c.id.as_str().to_string(),
                    external_id: c.external_id.as_str().to_string(),
                    system_of_record: c.system_of_record.as_str().to_string(),
                    risk_tier: None,
                    as_of_ms: c.as_of_ms,
                })
                .collect();
            proposals.extend(
                ticket_proposals(&risks)
                    .map_err(refused)?
                    .into_iter()
                    .map(|t| ProposalRow {
                        kind: ProposalKind::FindingTicket,
                        id: t.id.as_str().to_string(),
                        external_id: t.external_id.as_str().to_string(),
                        system_of_record: t.system_of_record.as_str().to_string(),
                        risk_tier: Some(t.risk_tier),
                        as_of_ms: t.as_of_ms,
                    }),
            );
            Ok(WritebackOutcome::Proposals { proposals })
        }
        stored => Err(invalid(format!("{stored:?} is not a computed operation"))),
    }
}
