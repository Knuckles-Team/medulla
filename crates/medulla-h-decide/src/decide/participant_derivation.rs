//! `CompleteDerivation` is checked against the claim store (MEDULLA-DECISION-ENGINE-R651.12).
//!
//! The graph is the authority: a worker that finishes a derive task reports how many claims it
//! wrote, and the engine counts the claim records that carry the task's derivation key and cite a
//! turn of its range. A report that differs from that count is `DERIVATION_COUNT_MISMATCH` and
//! nothing is checkpointed. The checkpoint itself is the graph write of
//! `Method::Participant { CompleteDerivation }`, which the router continues to after this check.

use super::SharedState;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// Refuse a `CompleteDerivation` whose `claims_written` is not the number of claims the claim
/// store holds for the payload's key and range.
#[cfg(feature = "statechart")]
pub async fn check_derivation_count(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    payload_ref: &str,
    claims_written: u32,
) -> Result<(), String> {
    use medulla_h_graph_gateway::graph_ops::participant_derive::derive_input;
    use {
        medulla_decision_stats_records::decision::participant_derive::conclusion_claim_prefix,
        medulla_decision_stats_records::decision::participant_derive::DeriveKey,
    };
    let tenant = verified.tenant();
    let core = super::claims::participants_of(state, graph)
        .await
        .ok_or_else(|| format!("DERIVE_PAYLOAD_NOT_FOUND: no derive payload {payload_ref}"))?;
    let input = derive_input(&core, tenant, payload_ref)?;
    if claims_written > input.max_conclusions {
        return Err(format!(
            "DERIVE_CONCLUSION_LIMIT: {claims_written} conclusions exceed memory.derive_max_conclusions"
        ));
    }
    let key = DeriveKey {
        scope_id: input.scope_id.clone(),
        session_id: input.session_id.clone(),
        subject: input.subject.clone(),
        observer: input.observer.clone(),
    };
    let turn_ids: std::collections::BTreeSet<&str> = input
        .turns
        .iter()
        .map(|turn| turn.turn_id.as_str())
        .collect();
    let persist_dir = { state.read().await.persist_dir.clone() };
    let held = super::claims::served::ClaimStore::open(
        persist_dir.as_deref(),
        tenant,
        &verified.principal_persistence_id(),
    )?
    .derived_claim_count(&conclusion_claim_prefix(&key), &turn_ids)?;
    if held != claims_written {
        return Err(format!(
            "DERIVATION_COUNT_MISMATCH: the worker reported {claims_written} claims and the claim store holds {held} for this range"
        ));
    }
    Ok(())
}

/// Without the claim store no derived claim can exist, so no count can be confirmed.
#[cfg(not(feature = "statechart"))]
pub async fn check_derivation_count(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    payload_ref: &str,
    claims_written: u32,
) -> Result<(), String> {
    let _ = (state, graph, verified, payload_ref, claims_written);
    Err("CompleteDerivation requires the `statechart` feature".to_string())
}
