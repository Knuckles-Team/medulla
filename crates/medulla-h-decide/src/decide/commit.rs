//! `Method::DecisionCommit`: make one assembly record durable.
//!
//! The record arriving on the wire is never trusted (DECIDE-LAYER-DESIGN
//! §4.2). In order: the version and size are checked; the engine re-derives
//! the record from its own stored inputs and compares bytes; the certificate
//! is verified independently; then, inside ONE Agent Library write, every
//! candidate's stored facts are compared with the revision they name and the
//! scope's catalog digest is compared-and-set. Only then is the record stored,
//! as a `DecisionRecord` component plus its verbatim body. A repeat of the
//! same record is an idempotent replay.

use super::SharedState;
use medulla_decision_types::decision::DecisionCommitRequest;
#[cfg(feature = "redb")]
#[cfg(feature = "decide")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// Commit one `DecisionRecord` as a component revision, idempotently.
#[cfg(feature = "decide")]
pub async fn handle_decision_commit(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: DecisionCommitRequest,
) -> Response {
    match commit(state, req_id, verified, request).await {
        Ok(result) => Response::ok(
            req_id,
            medulla_protocol_types::protocol::ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::DecisionCommit,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "decide")]
async fn commit(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: DecisionCommitRequest,
) -> Result<medulla_decision_types::decision::DecisionCommitResult, String> {
    use medulla_decision_types::{
        decision::DecisionErrorCode, decision::MAX_DECISION_RECORD_BYTES,
    };
    use medulla_raft_xshard::cross_shard_txn::certificate::OwnerDecision;

    let DecisionCommitRequest {
        context,
        record,
        expected_catalog_digest,
    } = request;
    if record.tenant_id != verified.tenant() {
        return Err(
            "ACCESS_DENIED: the record tenant must match the verified request tenant".into(),
        );
    }
    let bytes = medulla_compute::assemble::encoded_len(&record);
    if bytes > MAX_DECISION_RECORD_BYTES {
        return Err(format!(
            "{}: {bytes} bytes exceed {MAX_DECISION_RECORD_BYTES}",
            DecisionErrorCode::RecordTooLarge.as_str()
        ));
    }
    let checked = record.clone();
    tokio::task::spawn_blocking(move || medulla_compute::assemble::replay_check(&checked))
        .await
        .map_err(|error| format!("decision replay task failed: {error}"))?
        .map_err(|error| error.to_string())?;
    super::shape::check_decision_shape(&record)?;
    // MEDULLA-DECISION-ENGINE-R113.2.1: the library and capacity ledger have
    // separate owners. Do not publish a capacity-backed decision until a
    // shared transaction can also publish its leases and graph evidence.
    // Inputs alone are advisory; abstentions and zero-demand plans acquire
    // no capacity and must not be refused by this boundary guard.
    if record
        .topology_plan()
        .is_some_and(|plan| plan.lease.per_cell.iter().any(|cell| cell.amount > 0))
    {
        return Err(format!(
            "{}: topology capacity acquisition requires shared transaction authority",
            DecisionErrorCode::DerivationRejected.as_str()
        ));
    }
    let store = state.write().await.ensure_agent_library()?;
    let context = medulla_persistence_records::agent_library::context::bind_agent_library_context(
        &store,
        req_id,
        verified,
        context,
        "decision:commit",
        true,
    )?;
    // MEDULLA-DECISION-ENGINE-R036.103/.104/.105: the commit is a transaction over the RBAC image
    // and the Agent Library heads. The coordinator writes its protocol-start marker and the
    // request-bound certificate first; both owners reserve; the vote is taken; the decision is
    // made durable with its certificate; only then is the record written; and both owners are
    // resolved from the durable decision. A repeat of a stored record needs none of this.
    let replaying = store
        .current_component(verified.tenant(), &record.record_id)?
        .is_some();
    let mut premise = None;
    let (admitted, coordinator) = if replaying {
        (admission::Admitted::none(verified.tenant()), None)
    } else {
        let coordinator =
            certified::Coordinator::begin(state, verified, &record, &expected_catalog_digest)
                .await?;
        premise = Some(coordinator.premise());
        let admitted = if coordinator.resumed {
            admission::Admitted::resume(state, verified, coordinator.txn_id()).await
        } else {
            match admission::Admitted::reserve(
                state,
                &store,
                verified,
                &record,
                &context.policy_revision,
                coordinator.txn_id(),
            )
            .await
            {
                Ok(admitted) => admitted,
                Err(error) => {
                    // The reservation refused and released what it took: the decision is ABORT.
                    coordinator.decide(false).await?;
                    return Err(error);
                }
            }
        };
        (admitted, Some(coordinator))
    };
    if let Some(coordinator) = &coordinator {
        // The vote: the commit's own stale-catalog check, taken before the decision so a NO is
        // decided ABORT and both owners resolve from it.
        let vote = expected_catalog_digest == record.inputs.catalog_digest;
        let decided = if coordinator.resumed {
            Ok(OwnerDecision::Commit)
        } else {
            coordinator.decide(vote).await
        };
        match decided {
            Ok(decision) => {
                if !vote {
                    let _ = admitted.resolve_decided(state, &store, decision).await;
                    return Err(format!(
                        "{}: the expected catalog is not the record's",
                        DecisionErrorCode::StaleCatalog.as_str()
                    ));
                }
            }
            Err(error) => {
                // CERTIFICATE_EXPIRED (the coordinator recorded ABORT) or a storage refusal:
                // both owners resolve from the durable decision, the code answers first.
                if let Ok(decision) = coordinator.recover().await {
                    let _ = admitted.resolve_decided(state, &store, decision).await;
                }
                return Err(error);
            }
        }
    }
    let written = super::telemetry::stage(
        super::telemetry::Stage::Commit,
        "assembly",
        verified.tenant(),
        || store.commit_decision_record(context, &record, &expected_catalog_digest),
    );
    if let Some(coordinator) = &coordinator {
        // Both owners resolve from the durable decision, never from the local write result.
        let decision = coordinator.recover().await?;
        admitted.resolve_decided(state, &store, decision).await?;
    }
    let written = written?;
    if let Some(premise) = premise {
        tracing::info!(%premise, "decision-commit coordinator decision");
    }
    super::assemble::audit_line("decision-commit", &record, verified);
    Ok(medulla_decision_types::decision::DecisionCommitResult {
        schema_version: medulla_decision_types::decision::DECISION_COMMIT_RESULT_SCHEMA_VERSION,
        record_id: record.record_id,
        component: written.result,
        replayed: written.replayed,
    })
}

/// A build without the Decide layer still serves the method: it refuses by
/// name.
#[cfg(not(feature = "decide"))]
pub async fn handle_decision_commit(
    _state: &SharedState,
    req_id: u64,
    _verified: &VerifiedRequestContext,
    _request: DecisionCommitRequest,
) -> Response {
    Response::err(req_id, "DecisionCommit requires the `decide` feature")
}

#[cfg(feature = "decide")]
pub(in crate::decide) mod admission;
#[cfg(feature = "decide")]
pub(in crate::decide) mod certified;
#[cfg(all(test, feature = "decide", feature = "redb"))]
mod tests;
