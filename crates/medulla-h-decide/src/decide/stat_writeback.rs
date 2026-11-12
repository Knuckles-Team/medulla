//! `DecisionLog.writeback`, dispatched (MEDULLA-DECISION-ENGINE-R615.2.1,
//! R615.2.2): the write-back preflight and the creation and ticket proposals.
//!
//! Both operations compute from the request alone: nothing is stored, so the
//! op classifies as a read. The rules live in
//! `medulla_decision_stats::decision::writeback`.

use std::collections::BTreeSet;

use medulla_decision_stats::decision::writeback::{
    creation_proposals, preflight, ticket_proposals, InventoryItem, Preflight, RiskFinding,
    WritebackError, WritebackProposal,
};
use medulla_decision_types::decision::statistical::writeback::{
    PreflightVerdict, ProposalKind, ProposalRow, WritebackOp, WritebackOutcome,
};
use medulla_types::contract::{ResourceId, TenantId};

fn invalid(error: impl std::fmt::Display) -> String {
    format!("PARAMETER_INVALID: {error}")
}

fn refused(error: WritebackError) -> String {
    error.to_string()
}

fn resource(value: &str) -> Result<ResourceId, String> {
    ResourceId::new(value).map_err(|detail| invalid(format!("{value:?}: {detail}")))
}

/// Serve one write-back computation for `tenant`.
pub(super) fn serve(tenant: &str, op: WritebackOp) -> Result<WritebackOutcome, String> {
    let min_tier = medulla_server_api::knob_config::knobs().writeback_queue_min_tier;
    run(tenant, op, min_tier)
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
    }
}
