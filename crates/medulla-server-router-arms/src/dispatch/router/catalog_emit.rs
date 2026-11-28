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
pub async fn commit_asset(
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
    let triples =
        medulla_catalog::emit::emit_asset(record, &links).map_err(|error| error.to_string())?;
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

/// How many catalog emits failed after their record committed, read from the registered
/// `medulla_catalog_emit_failures_total` series.
#[cfg(all(test, feature = "rdf"))]
pub(super) fn emit_failures() -> u64 {
    medulla_kernel::metrics::process().catalog_emit_failures()
}

/// Answer `response` unchanged. A committed write is never reported as a failure
/// (MEDULLA-TYPED-PACKS-R108.3): a catalog emit that failed after its record committed is
/// logged and counted on the registered `medulla_catalog_emit_failures_total` series, and the
/// replayed request emits the nodes again.
#[cfg(feature = "rdf")]
pub fn after_emit(response: Response, _req_id: u64, emitted: Result<(), String>) -> Response {
    if let Err(refusal) = emitted {
        medulla_kernel::metrics::process().catalog_emit_failed();
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
    use medulla_catalog_source_types::stewardship::StewardRole;
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
    let Ok(probe) =
        medulla_catalog::emit::emit_asset(*record, &medulla_catalog::emit::AssetLinks::default())
    else {
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

/// One run a terminal commit records: the run id, the work item that ran it and, once read
/// back from the work item row, the workflow definition it executed (MEDULLA-TYPED-PACKS-R108.6).
#[cfg(feature = "rdf")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTraceAsset {
    pub run_id: String,
    pub tenant: String,
    pub work_item_id: String,
    pub definition: Option<medulla_work_types::orchestration::workflows::WorkflowIdentity>,
}

/// The runs a terminal commit records: one per `RunTrace` receipt node of its outcome
/// extension, in node order. `None` for any method that is not a terminal commit whose
/// extension holds a run trace. The definition each run executed is not read from the trace the
/// caller sent: the commit stamps it from the work item row, and [`commit_runs`] reads it back
/// from that row.
#[cfg(feature = "rdf")]
pub fn run_traces_of(method: &Method) -> Option<Vec<RunTraceAsset>> {
    let Method::CommitWorkItemResult {
        tenant,
        work_item_id,
        outcome_extension: Some(extension),
        ..
    } = method
    else {
        return None;
    };
    let runs = run_traces(&extension.receipt_nodes, tenant, work_item_id);
    (!runs.is_empty()).then_some(runs)
}

/// One run per `RunTrace` receipt node.
#[cfg(feature = "rdf")]
fn run_traces(
    nodes: &[medulla_result_types::outcome_bundle::ReceiptNode],
    tenant: &str,
    work_item_id: &str,
) -> Vec<RunTraceAsset> {
    use medulla_result_types::outcome_bundle::ReceiptNodeKind;
    nodes
        .iter()
        .filter(|node| node.kind == ReceiptNodeKind::RunTrace)
        .map(|node| RunTraceAsset {
            run_id: node.run_id.clone(),
            tenant: tenant.to_string(),
            work_item_id: work_item_id.to_string(),
            definition: None,
        })
        .collect()
}

/// The definition a work item row names: its recorded workflow, when the identity is whole and
/// valid. Any other row (no workflow, a malformed digest) still records the run, with no link.
#[cfg(feature = "rdf")]
fn definition_of(
    view: Option<&medulla_work_types::work_item_read::WorkItemView>,
) -> Option<medulla_work_types::orchestration::workflows::WorkflowIdentity> {
    view.and_then(|view| view.workflow.clone())
        .filter(|identity| identity.validate().is_ok())
}

/// Read one work item row back through the served `GetWorkItem` read, under the committing
/// caller's identity. A row that cannot be read names no definition.
#[cfg(feature = "rdf")]
async fn read_work_item(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    run: &RunTraceAsset,
) -> Option<medulla_work_types::work_item_read::WorkItemView> {
    use medulla_protocol_types::protocol::ResultPayload;
    let response = dispatch_graph_op(
        state,
        graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        Method::GetWorkItem {
            tenant: run.tenant.clone(),
            work_item_id: run.work_item_id.clone(),
        },
    )
    .await;
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice::<
            Option<medulla_work_types::work_item_read::WorkItemView>,
        >(&bytes)
        .ok()
        .flatten(),
        _ => None,
    }
}

/// Catalog every run of a committed terminal result. Each run links to the definition its work
/// item row recorded at submit. The first refusal stops the rest; a replay of the same commit
/// emits them all again (the triples are idempotent).
#[cfg(feature = "rdf")]
pub async fn commit_runs(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    runs: &[RunTraceAsset],
) -> Result<(), String> {
    for run in runs {
        let view = read_work_item(state, req, verified_context, graph, run).await;
        let definition = definition_of(view.as_ref());
        commit_asset(
            state,
            req,
            verified_context,
            graph,
            medulla_catalog::emit::AssetRecord::WorkflowRun(
                medulla_catalog::emit::WorkflowRunRecord {
                    run_id: &run.run_id,
                    definition: definition.as_ref(),
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
    use medulla_work_types::orchestration::workflows::WorkflowIdentity;
    use medulla_work_types::work_item_read::WorkItemView;

    fn trace_node(run_id: &str) -> ReceiptNode {
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
            // A caller's own claim of a workflow is never the link.
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "workflow_id": "forged",
                "workflow_version": "9",
                "workflow_content_digest": format!("sha256:{}", "f".repeat(64)),
            }))
            .unwrap(),
        }
    }

    fn view(workflow: Option<WorkflowIdentity>) -> WorkItemView {
        let row = serde_json::json!({
            "node_type": "WorkItem", "tenant": "t", "status": "succeeded", "kind": "run",
            "payload_ref": "p",
        });
        let mut view = WorkItemView::from_tenant_row("wi", row.as_object().unwrap(), "t")
            .unwrap()
            .unwrap();
        view.workflow = workflow;
        view
    }

    // spec: MEDULLA-TYPED-PACKS-R108.6
    #[test]
    fn a_run_names_its_definition_only_from_a_whole_valid_row_identity() {
        let runs = run_traces(&[trace_node("run-1")], "t", "wi");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "run-1");
        assert_eq!(
            (runs[0].tenant.as_str(), runs[0].work_item_id.as_str()),
            ("t", "wi")
        );
        // The trace the caller sent names a definition; the run does not take it.
        assert!(runs[0].definition.is_none());

        let valid = WorkflowIdentity {
            workflow_id: "summarize".into(),
            version: "1.2.0".into(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
        };
        assert_eq!(
            definition_of(Some(&view(Some(valid.clone())))),
            Some(valid.clone())
        );
        assert!(definition_of(Some(&view(None))).is_none());
        assert!(definition_of(None).is_none());
        let badly_digested = WorkflowIdentity {
            content_digest: "not-a-digest".into(),
            ..valid
        };
        assert!(definition_of(Some(&view(Some(badly_digested)))).is_none());
        assert!(run_traces_of(&Method::GraphSchemaList).is_none());
    }
}

#[cfg(all(test, feature = "rdf"))]
mod tests {
    use super::*;

    // spec: MEDULLA-TYPED-PACKS-R108.3
    #[cfg(feature = "metrics")]
    #[test]
    fn a_failed_emit_after_a_committed_record_is_counted_and_never_fails_the_response() {
        let before = emit_failures();
        let ok = Response::ok(
            7,
            medulla_protocol_types::protocol::ResultPayload::Raw(Vec::new()),
        );
        let answered = after_emit(ok, 7, Err("CATALOG_EMIT_FAILED: boom".to_string()));
        assert!(answered.error.is_none());
        assert_eq!(emit_failures(), before + 1);
        let again = after_emit(answered, 7, Ok(()));
        assert!(again.error.is_none());
        assert_eq!(emit_failures(), before + 1);
    }
}
