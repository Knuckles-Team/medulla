//! The dedicated source-query approval issuer writes the existing protected lease row
//! (MEDULLA-FEDERATED-QUERY-R085.2). External SQL never runs during approval; the grant
//! holds identity and digests only.
use super::*;
use medulla_work_types::source_query_approval::{
    ApproveSourceQueryRequest, SourceQueryApprovalIssued,
};

pub fn issue(
    graph: &str,
    request: &ApproveSourceQueryRequest,
    admitted_actor: Option<&str>,
    nodes: &mut NativeNodeRows<'_>,
    transitions: &mut NativeTransitionRows<'_>,
    crypto: DurableCrypto<'_>,
    committed_at_ms: u64,
) -> Result<SourceQueryApprovalIssued, String> {
    admitted_actor.ok_or("ACCESS_DENIED: source-query approval issuer unavailable")?;
    let (lease, approved) = request.lease_at(committed_at_ms)?;
    if load_row(nodes, graph, &lease.lease_id, crypto)?.is_some() {
        return Ok(SourceQueryApprovalIssued {
            outcome: ControlLeaseIssueOutcome::Collision,
            approved: None,
            changed_work_item_ids: Vec::new(),
        });
    }
    let mut row = lease.row();
    write_work_item_props(nodes, transitions, graph, &lease.lease_id, &mut row, crypto)?;
    Ok(SourceQueryApprovalIssued {
        outcome: ControlLeaseIssueOutcome::Issued,
        approved: Some(approved),
        changed_work_item_ids: vec![lease.lease_id],
    })
}

#[cfg(test)]
mod tests;
