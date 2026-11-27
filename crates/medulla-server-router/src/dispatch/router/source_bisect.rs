//! `Method::BisectStep` (MEDULLA-SCM-R009): the next changeset of a branch chain that needs a
//! gate verdict, or the culprit. The chain window comes from the branch history index, the
//! verdicts from the sealed `GateVerdict` nodes of the repository graph. Reads only.

use super::*;

/// The wire field of `Method::BisectStep`.
pub struct BisectStepFields {
    pub request: Box<medulla_source_types::source_bisect::BisectStepRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::super::read_path_history::RegistryHistory;
    use super::*;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_bisect::{read_window, step};
    use medulla_mutation_types::branch::BranchRef;
    use medulla_server_auth::access::path_grant;
    use medulla_server_graph::dispatch::shared::{
        authorized_graph_read, ensure_graph_resident, timed_read,
    };
    use medulla_source_types::gate_verdict::{
        gate_verdict_node_id, GateVerdictRecord, GATE_VERDICT_RECORD_PROPERTY,
    };
    use medulla_source_types::source_bisect::BisectStepReceipt;
    use medulla_source_types::source_path_grant::PathAccess;
    use medulla_work_types::step_verdict::VerificationVerdict;
    use serde_json::Value;
    use std::collections::BTreeMap;

    /// Serve `Method::BisectStep`.
    pub async fn handle_bisect_step(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: BisectStepFields,
    ) -> Response {
        let request = fields.request;
        let reference = BranchRef {
            tenant: verified.tenant().to_string(),
            graph: request.repo.clone(),
            name: request.branch.clone(),
        };
        let target = match ops::resolve_target(state, verified, reference).await {
            Ok(target) => target,
            Err(error) => return ops::target_error_response(req.id, error),
        };
        let (tenant, principal) = (
            verified.tenant().to_string(),
            verified.agent_id().to_string(),
        );
        let window_request = request.clone();
        let window = target
            .read_blocking(move |registry, branch| {
                // A bisect reads the whole repository's history: it needs read on the root.
                let grants = registry.path_grants(&tenant, &window_request.repo, &principal)?;
                if !path_grant::allows(&grants, "", PathAccess::Read) {
                    return Ok(Err(format!(
                        "{}: no read grant covers the repository",
                        PathAccess::Read.denied().code()
                    )));
                }
                Ok(
                    read_window(&RegistryHistory { registry, branch }, &window_request)?
                        .map_err(|refusal| refusal.to_string()),
                )
            })
            .await;
        let window = match window {
            Ok(Ok(Ok(window))) => window,
            Ok(Ok(Err(message))) => return Response::err(req.id, message),
            Ok(Err(error)) | Err(error) => return Response::err(req.id, error),
        };
        ensure_graph_resident(state, &request.repo).await;
        let read = {
            let current = timed_read(state).await;
            authorized_graph_read(
                &current,
                &request.repo,
                verified,
                &format!("GRAPH_NOT_FOUND: {}", request.repo),
            )
        };
        let (core, authority) = match read {
            Ok(read) => read,
            Err(error) => return Response::err(req.id, error),
        };
        let mut verdicts: BTreeMap<String, VerificationVerdict> = BTreeMap::new();
        for link in &window.links {
            let Some(blob) =
                core.get_node_properties(&gate_verdict_node_id(&link.changeset, &request.gate_id))
            else {
                continue;
            };
            if !authority.can_see_node(&blob, false) {
                continue;
            }
            let Ok(node) = medulla_types::msgpack::decode_property_value(&blob) else {
                continue;
            };
            let record: Option<GateVerdictRecord> = node
                .get(GATE_VERDICT_RECORD_PROPERTY)
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str(text).ok());
            if let Some(record) = record {
                verdicts.insert(link.changeset.clone(), record.verdict);
            }
        }
        match step(&window, &verdicts) {
            Ok(receipt) => typed_response_of(req.id, receipt),
            Err(refusal) => Response::err(req.id, refusal.to_string()),
        }
    }

    fn typed_response_of(req_id: u64, receipt: BisectStepReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::BisectStep>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_bisect_step;

/// `BisectStep` needs the `redb` branch registry and the `blob` ingest handlers.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_bisect_step(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: BisectStepFields,
) -> Response {
    Response::err(req.id, "BisectStep requires the `redb` and `blob` features")
}
