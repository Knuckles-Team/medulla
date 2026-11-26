//! Engine-held records reach the metadata catalog through the one emitter
//! (MEDULLA-TYPED-PACKS-R108.3).
//!
//! An Agent Library publish, a workflow registration and a graph creation each hold a
//! record the engine owns. After the record is durable, `emit_asset` turns it into the
//! catalog triples that describe it and the triples are committed as one served
//! `AddTriples` on the graph the request was issued on, so the catalog write guard of
//! `R108.4` and the ranking index of `R108.16` see them like any other asset. Connector
//! asset nodes stay with the arbora batch; no family keeps its own index.

use super::*;

/// Emit one record's catalog nodes into `graph`. A refusal text means the record is durable
/// but its catalog nodes are not; replaying the same request emits them again (the triples
/// are idempotent).
#[cfg(feature = "rdf")]
pub(super) async fn commit_asset(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    record: medulla_catalog::emit::AssetRecord<'_>,
) -> Result<(), String> {
    let evidence = asset_evidence(state, req, verified_context, graph, &record).await;
    let links = medulla_catalog::emit::AssetLinks {
        owner: evidence.owner.as_deref(),
        steward: evidence.steward.as_deref(),
        graph_revision: evidence.graph_revision,
        changed_at_ms: Some(evidence.changed_at_ms),
        ..medulla_catalog::emit::AssetLinks::default()
    };
    let triples = medulla_catalog::emit::emit_asset(record, &links)
        .map_err(|error| error.to_string())?;
    let response = dispatch_graph_op(
        state,
        graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        Method::AddTriples {
            turtle: String::new(),
            ntriples: medulla_catalog::emit::to_ntriples(&triples),
        },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(format!("CATALOG_EMIT_FAILED: {refusal}")),
        None => Ok(()),
    }
}

/// How many catalog emits failed after their record committed, since the process started.
static EMIT_FAILURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The number of catalog emits that failed after their record committed.
#[cfg(feature = "rdf")]
pub(super) fn emit_failures() -> u64 {
    EMIT_FAILURES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Answer `response` unchanged. A committed write is never reported as a failure
/// (MEDULLA-TYPED-PACKS-R108.3): a catalog emit that failed after its record committed is
/// logged and counted, and the replayed request emits the nodes again.
#[cfg(feature = "rdf")]
pub(super) fn after_emit(response: Response, _req_id: u64, emitted: Result<(), String>) -> Response {
    if let Err(refusal) = emitted {
        EMIT_FAILURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::error!(target: "medulla::catalog", %refusal, "catalog emit failed after the record committed");
    }
    response
}

/// What a node carries beyond its source record id: who owns and stewards it, the graph
/// revision it was catalogued from and when (MEDULLA-TYPED-PACKS-R108.5).
#[cfg(feature = "rdf")]
struct AssetEvidence {
    owner: Option<String>,
    steward: Option<String>,
    graph_revision: Option<u64>,
    changed_at_ms: u64,
}

/// The principal as an IRI: an IRI stays as it is, anything else is minted under the
/// principal namespace.
#[cfg(feature = "rdf")]
fn principal_iri(principal: &str) -> String {
    if principal.contains(':') && !principal.starts_with("principal:") {
        principal.to_string()
    } else {
        format!("urn:medulla:principal:{principal}")
    }
}

/// Read the evidence of one record. The node is a projection, so a stewardship or graph read
/// that fails leaves that field off the node (and the node flagged `owner_missing`); it never
/// fails the record's own write.
#[cfg(feature = "rdf")]
async fn asset_evidence(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    record: &medulla_catalog::emit::AssetRecord<'_>,
) -> AssetEvidence {
    use medulla_catalog_types::stewardship::StewardRole;
    use medulla_server_graph::dispatch::graph_client::GraphTarget;

    let changed_at_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let mut evidence = AssetEvidence {
        owner: None,
        steward: None,
        graph_revision: None,
        changed_at_ms,
    };
    medulla_server_graph::dispatch::shared::ensure_graph_resident(state, graph).await;
    {
        let current = medulla_server_graph::dispatch::shared::timed_read(state).await;
        if let Ok((core, _)) = medulla_server_graph::dispatch::shared::authorized_graph_read(
            &current,
            graph,
            verified_context,
            &format!("GRAPH_NOT_FOUND: {graph}"),
        ) {
            evidence.graph_revision = Some(core.version());
        }
    }
    let Ok(probe) = medulla_catalog::emit::emit_asset(
        record.clone(),
        &medulla_catalog::emit::AssetLinks::default(),
    ) else {
        return evidence;
    };
    let Some(subject) = probe.first().map(|triple| triple.subject.clone()) else {
        return evidence;
    };
    let target = GraphTarget {
        graph,
        req_id: req.id,
        caller: req.agent_id.as_deref(),
        verified: verified_context,
    };
    if let Ok(assignments) =
        medulla_server_graph::dispatch::stewardship::load_assignments(state, &target).await
    {
        let scopes = medulla_core::stewardship::TargetScopes {
            asset: Some(subject),
            concept_scheme: None,
            domain: None,
        };
        let resolved = medulla_core::stewardship::resolve(&assignments, &scopes);
        let first = |role: StewardRole| {
            resolved
                .iter()
                .find(|assignment| assignment.role == role)
                .map(|assignment| principal_iri(&assignment.principal))
        };
        evidence.owner = first(StewardRole::Owner);
        evidence.steward = first(StewardRole::Steward);
    }
    // A product names its owner when it is registered; the stewardship store answers first.
    if evidence.owner.is_none() {
        if let medulla_catalog::emit::AssetRecord::Product(product) = record {
            evidence.owner = Some(principal_iri(product.owner));
        }
    }
    evidence
}

/// One run a terminal commit records: the run id and the workflow definition its run trace
/// names, when it does (MEDULLA-TYPED-PACKS-R108.6).
#[cfg(feature = "rdf")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunTraceAsset {
    pub run_id: String,
    pub definition: Option<medulla_work_types::orchestration::workflows::WorkflowIdentity>,
}

/// The run-trace property names that carry the workflow definition identity a run executed.
#[cfg(feature = "rdf")]
const RUN_WORKFLOW_ID: &str = "workflow_id";
#[cfg(feature = "rdf")]
const RUN_WORKFLOW_VERSION: &str = "workflow_version";
#[cfg(feature = "rdf")]
const RUN_WORKFLOW_DIGEST: &str = "workflow_content_digest";

/// The runs a terminal commit records: one per `RunTrace` receipt node of its outcome
/// extension, in node order. `None` for any method that is not a terminal commit whose
/// extension holds a run trace.
#[cfg(feature = "rdf")]
pub(super) fn run_traces_of(method: &Method) -> Option<Vec<RunTraceAsset>> {
    let Method::CommitWorkItemResult {
        outcome_extension: Some(extension),
        ..
    } = method
    else {
        return None;
    };
    let runs = run_traces(&extension.receipt_nodes);
    (!runs.is_empty()).then_some(runs)
}

/// One run per `RunTrace` receipt node. A trace that names a valid workflow identity (all
/// three properties, a well-formed digest) links its run asset to that definition; any other
/// trace still records the run.
#[cfg(feature = "rdf")]
fn run_traces(nodes: &[medulla_result_types::outcome_bundle::ReceiptNode]) -> Vec<RunTraceAsset> {
    use medulla_result_types::outcome_bundle::ReceiptNodeKind;
    nodes
        .iter()
        .filter(|node| node.kind == ReceiptNodeKind::RunTrace)
        .map(|node| {
            let properties: std::collections::BTreeMap<String, serde_json::Value> =
                rmp_serde::from_slice(&node.properties_msgpack).unwrap_or_default();
            let text = |key: &str| {
                properties
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            let definition = match (
                text(RUN_WORKFLOW_ID),
                text(RUN_WORKFLOW_VERSION),
                text(RUN_WORKFLOW_DIGEST),
            ) {
                (Some(workflow_id), Some(version), Some(content_digest)) => {
                    Some(medulla_work_types::orchestration::workflows::WorkflowIdentity {
                        workflow_id,
                        version,
                        content_digest,
                    })
                    .filter(|identity| identity.validate().is_ok())
                }
                _ => None,
            };
            RunTraceAsset {
                run_id: node.run_id.clone(),
                definition,
            }
        })
        .collect()
}

/// Catalog every run of a committed terminal result. The first refusal stops the rest; a
/// replay of the same commit emits them all again (the triples are idempotent).
#[cfg(feature = "rdf")]
pub(super) async fn commit_runs(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    runs: &[RunTraceAsset],
) -> Result<(), String> {
    for run in runs {
        commit_asset(
            state,
            req,
            verified_context,
            graph,
            medulla_catalog::emit::AssetRecord::WorkflowRun(
                medulla_catalog::emit::WorkflowRunRecord {
                    run_id: &run.run_id,
                    definition: run.definition.as_ref(),
                },
            ),
        )
        .await?;
    }
    Ok(())
}

#[cfg(all(test, feature = "rdf"))]
mod run_trace_tests {
    use super::*;
    use medulla_result_types::outcome_bundle::{ReceiptNode, ReceiptNodeKind};

    fn trace_node(run_id: &str, properties: serde_json::Value) -> ReceiptNode {
        ReceiptNode {
            node_id: format!("trace:{run_id}"),
            kind: ReceiptNodeKind::RunTrace,
            delegation_id: "delegation".into(),
            work_item_id: "work-item".into(),
            run_id: run_id.into(),
            fence_token: 1,
            result_ref: None,
            outbox_id: "outbox".into(),
            payload_ref: "payload".into(),
            payload_digest: "digest".into(),
            properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R108.6
    #[test]
    fn a_run_trace_names_its_definition_only_with_a_whole_valid_identity() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let named = trace_node(
            "run-1",
            serde_json::json!({
                RUN_WORKFLOW_ID: "summarize",
                RUN_WORKFLOW_VERSION: "1.2.0",
                RUN_WORKFLOW_DIGEST: digest,
            }),
        );
        let partial = trace_node("run-2", serde_json::json!({ RUN_WORKFLOW_ID: "summarize" }));
        let badly_digested = trace_node(
            "run-3",
            serde_json::json!({
                RUN_WORKFLOW_ID: "summarize",
                RUN_WORKFLOW_VERSION: "1.2.0",
                RUN_WORKFLOW_DIGEST: "not-a-digest",
            }),
        );
        let found = run_traces(&[named, partial, badly_digested]);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].run_id, "run-1");
        assert_eq!(
            found[0].definition.as_ref().map(|d| d.workflow_id.as_str()),
            Some("summarize")
        );
        assert!(found[1].definition.is_none());
        assert!(found[2].definition.is_none());
        assert!(run_traces_of(&Method::GraphSchemaList).is_none());
    }
}

#[cfg(all(test, feature = "rdf"))]
mod tests {
    use super::*;

    // spec: MEDULLA-TYPED-PACKS-R108.3
    #[test]
    fn a_failed_emit_after_a_committed_record_is_counted_and_never_fails_the_response() {
        let before = emit_failures();
        let ok = Response::ok(7, medulla_protocol_types::protocol::ResultPayload::Raw(Vec::new()));
        let answered = after_emit(ok, 7, Err("CATALOG_EMIT_FAILED: boom".to_string()));
        assert!(answered.error.is_none());
        assert_eq!(emit_failures(), before + 1);
        let again = after_emit(answered, 7, Ok(()));
        assert!(again.error.is_none());
        assert_eq!(emit_failures(), before + 1);
    }
}
