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

#[cfg(test)]
tokio::task_local! {
    /// The participant boundary a test commit stops at, as a process crash would
    /// (MEDULLA-DECISION-ENGINE-R113.2.4). Compiled only under `cfg(test)`.
    pub(in crate::decide) static CRASH_AT: CrashPoint;
}

/// A participant boundary of a topology commit.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::decide) enum CrashPoint {
    /// After the owners reserved and the leases were acquired, before the vote.
    Prepare,
    /// After the durable COMMIT, before the record is written.
    Decision,
    /// After the record and evidence are written, before the owners resolve.
    Apply,
}

/// Stop here when the running test asked to crash at `point`: no cleanup runs.
#[cfg(test)]
fn crash_if(point: CrashPoint) -> Result<(), String> {
    if CRASH_AT.try_with(|at| *at == point).unwrap_or(false) {
        return Err(format!(
            "{}: injected crash at {point:?}",
            medulla_decision_types::decision::DecisionErrorCode::DecisionCoordinatorUnavailable
                .as_str()
        ));
    }
    Ok(())
}

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
        topology,
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
    // MEDULLA-DECISION-ENGINE-R113.2.1 and R113.2.5: the library and the capacity ledger have
    // separate owners. A capacity-backed topology decision (a solved plan with positive lease
    // demand) is published by the shared-owner path below, which needs a capacity authority:
    // without one it is refused before any write. Inputs alone are advisory; abstentions and
    // zero-demand plans acquire no capacity and keep the ordinary commit path.
    let capacity_backed = !capacity::demands(&record).is_empty();
    if !capacity_backed && topology.is_some() {
        return Err(format!(
            "{}: topology evidence was sent for a record that leases no capacity",
            DecisionErrorCode::DerivationRejected.as_str()
        ));
    }
    if capacity_backed {
        let capable = state
            .read()
            .await
            .persistence
            .as_ref()
            .is_some_and(|backend| backend.supports_native_capacity_leases());
        if !capable {
            return Err(format!(
                "{}: topology capacity acquisition requires shared transaction authority",
                DecisionErrorCode::DerivationRejected.as_str()
            ));
        }
        if topology.is_none() {
            return Err(format!(
                "{}: a capacity-backed topology commit names its capacity graph and evidence graph",
                DecisionErrorCode::DerivationRejected.as_str()
            ));
        }
        if let Some(asked) = &topology {
            capacity::check_ledger_access(state, verified, &asked.capacity_graph).await?;
        }
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
    // The evidence graph's context is bound to the verified request like the record's: tenant,
    // principal, nonce and policy come from the authenticated request, never from the body.
    let mut topology = topology;
    if let Some(asked) = topology.as_mut() {
        let mut bound =
            medulla_persistence_records::agent_library::context::bind_agent_library_context(
                &store,
                req_id,
                verified,
                asked.evidence.context.clone(),
                "agent-graph:publish",
                true,
            )?;
        bound.idempotency_key = format!("{}:evidence", bound.idempotency_key);
        asked.evidence.context = bound;
    }
    // MEDULLA-DECISION-ENGINE-R036.103/.104/.105: the commit is a transaction over the RBAC image
    // and the Agent Library heads. The coordinator writes its protocol-start marker and the
    // request-bound certificate first; both owners reserve; the vote is taken; the decision is
    // made durable with its certificate; only then is the record written; and both owners are
    // resolved from the durable decision. A repeat of a stored record needs none of this.
    let stored = store.current_component(verified.tenant(), &record.record_id)?;
    let replaying = stored.is_some();
    if let (Some(stored), true) = (&stored, capacity_backed) {
        // MEDULLA-DECISION-ENGINE-R113.2.4: a repeat of a decided capacity-backed commit is
        // refused with its original outcome. A crash after the record write may have left the
        // owners unresolved: the durable COMMIT of this transaction finishes them first.
        let txn_id = certified::Coordinator::transaction_id(&record, verified);
        if certified::Coordinator::decided_commit(state, verified, &record).await? {
            admission::Admitted::resume(state, verified, &txn_id)
                .await
                .resolve_decided(state, &store, OwnerDecision::Commit)
                .await?;
        }
        return Err(format!(
            "{}: record {} was decided COMMIT at version {}{}",
            DecisionErrorCode::CommitAlreadyDecided.as_str(),
            stored.component_id,
            stored.version,
            topology
                .as_ref()
                .map(|asked| format!(" with evidence graph {}", asked.evidence.graph.graph_id))
                .unwrap_or_default()
        ));
    }
    let mut premise = None;
    let (admitted, coordinator) = if replaying {
        (admission::Admitted::none(verified.tenant()), None)
    } else {
        let coordinator =
            certified::Coordinator::begin(state, verified, &record, &expected_catalog_digest)
                .await?;
        premise = Some(coordinator.premise());
        if coordinator.interrupted {
            // An earlier attempt stopped before the decision and its deadline passed: presumed
            // abort, recorded, then the leases and both reservations are released from it.
            let decision = coordinator.recover().await?;
            if let (Some(asked), true) = (topology.as_ref(), capacity_backed) {
                if let Ok(capacity::Prepared::Held(held)) = capacity::LeaseHold::acquire(
                    state,
                    verified,
                    &record,
                    &asked.capacity_graph,
                    coordinator.txn_id(),
                )
                .await
                {
                    held.resolve_decided(decision).await?;
                }
            }
            admission::Admitted::resume(state, verified, coordinator.txn_id())
                .await
                .resolve_decided(state, &store, decision)
                .await?;
            return Err(format!(
                "{}: transaction {} was decided ABORT",
                DecisionErrorCode::CertificateDecidedAbort.as_str(),
                coordinator.txn_id()
            ));
        }
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
    let mut leases: Option<capacity::LeaseHold> = None;
    if let (Some(coordinator), Some(topology), true) =
        (&coordinator, topology.as_ref(), capacity_backed)
    {
        // The lease participant's checked PREPARE: all cells or none. A denial or refusal
        // decides ABORT, which releases the owners' reservations; nothing is written.
        let prepared = capacity::LeaseHold::acquire(
            state,
            verified,
            &record,
            &topology.capacity_graph,
            coordinator.txn_id(),
        )
        .await;
        match prepared {
            Ok(capacity::Prepared::Held(hold)) => leases = Some(hold),
            Ok(capacity::Prepared::Denied(message)) => {
                let _ = coordinator.decide(false).await;
                let _ = admitted
                    .resolve_decided(state, &store, OwnerDecision::Abort)
                    .await;
                return Err(message);
            }
            Err(error) => {
                let _ = coordinator.decide(false).await;
                let _ = admitted
                    .resolve_decided(state, &store, OwnerDecision::Abort)
                    .await;
                return Err(error);
            }
        }
    }
    #[cfg(test)]
    crash_if(CrashPoint::Prepare)?;
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
                    if let Some(held) = leases.take() {
                        let _ = held.resolve_decided(decision).await;
                    }
                    let _ = admitted.resolve_decided(state, &store, decision).await;
                    return Err(format!(
                        "{}: the expected catalog is not the record's",
                        DecisionErrorCode::StaleCatalog.as_str()
                    ));
                }
                #[cfg(test)]
                crash_if(CrashPoint::Decision)?;
            }
            Err(error) => {
                // CERTIFICATE_EXPIRED (the coordinator recorded ABORT) or a storage refusal:
                // both owners resolve from the durable decision, the code answers first.
                if let Ok(decision) = coordinator.recover().await {
                    if let Some(held) = leases.take() {
                        let _ = held.resolve_decided(decision).await;
                    }
                    let _ = admitted.resolve_decided(state, &store, decision).await;
                }
                return Err(error);
            }
        }
    }
    let evidence_graph = topology
        .as_ref()
        .map(|asked| asked.evidence.graph.graph_id.clone());
    let lease_premises: Vec<String> = leases
        .as_ref()
        .map(|held| {
            held.lease_ids()
                .into_iter()
                .map(|id| format!("lease:{id}"))
                .collect()
        })
        .unwrap_or_default();
    let written = super::telemetry::stage(
        super::telemetry::Stage::Commit,
        "assembly",
        verified.tenant(),
        || match topology {
            // The record and its evidence graph are one Agent Library transaction, after the
            // durable COMMIT; the lease is the other participant, already held.
            Some(topology) if capacity_backed => {
                let txn_id = certified::Coordinator::transaction_id(&record, verified);
                store
                    .commit_topology_decision(
                        context,
                        &record,
                        &expected_catalog_digest,
                        topology.evidence,
                        &txn_id,
                    )
                    .map(|parts| (parts.record, Some(parts.evidence_revision)))
            }
            _ => store
                .commit_decision_record(context, &record, &expected_catalog_digest)
                .map(|written| (written, None)),
        },
    );
    #[cfg(test)]
    crash_if(CrashPoint::Apply)?;
    if let Some(coordinator) = &coordinator {
        // Both owners resolve from the durable decision, never from the local write result.
        let decision = coordinator.recover().await?;
        if let Some(held) = leases.take() {
            held.resolve_decided(decision).await?;
        }
        admitted.resolve_decided(state, &store, decision).await?;
    }
    let (written, evidence_revision) = written?;
    if let Some(premise) = premise {
        tracing::info!(%premise, "decision-commit coordinator decision");
    }
    super::assemble::audit_line("decision-commit", &record, verified);
    Ok(medulla_decision_types::decision::DecisionCommitResult {
        schema_version: medulla_decision_types::decision::DECISION_COMMIT_RESULT_SCHEMA_VERSION,
        record_id: record.record_id,
        component: written.result,
        replayed: written.replayed,
        topology: evidence_graph
            .zip(evidence_revision)
            .map(
                |(graph, revision)| medulla_decision_types::decision::TopologyCommitOutcome {
                    evidence_graph_id: graph,
                    evidence_revision: revision,
                    premises: lease_premises,
                },
            ),
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
pub(in crate::decide) mod capacity;
#[cfg(feature = "decide")]
pub(in crate::decide) mod certified;
#[cfg(all(test, feature = "decide", feature = "redb"))]
pub(in crate::decide) mod tests;
