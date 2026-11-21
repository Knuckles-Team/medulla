//! Served process compile (MEDULLA-CQ-ORCH-R700.3, R700.4, R700.5).
//!
//! `ProcessCompile` is gateway-routed (`Route::ProcessCompile`). The handler reads the
//! caller's tenant-visible process view (the `hasStep` and `handsOffTo` edges and the nodes
//! they join), compiles it with `compile_process_view` and writes the next
//! `WorkflowPattern` node `workflow:<process_iri>:v<n>` and its `compiledFrom` edge to the
//! process node as the one `BatchUpdate` through the staged gateway. The write is a
//! compare-and-set on `expected_version`: a version that is not the stored one refuses
//! `WORKFLOW_VERSION_CONFLICT`, and the commit closure re-reads the stored version so two
//! racing compiles never both write version `n + 1`. A published version is never mutated.
//! A refusal writes nothing. The `compiledFrom` edge carries `processRevision`, `compilerId`
//! and `workflowDigest`; the answer returns the digest, the version and the process revision.

use super::*;
use medulla_core::process_compile::{
    compile_process_view, CompiledProcess, ProcessCompilationError, ViewEdge, ViewNode,
};
use medulla_core::process_exchange::bpmn_lower::bpmn_kind_annotations;
use medulla_protocol_types::result_contract::coordination::ProcessCompile;
use medulla_server_api::knob_config::knobs;
use medulla_types::process_import::{ProcessCompiled, WORKFLOW_VERSION_CONFLICT};

/// The id of the compiler recorded on every `compiledFrom` edge.
pub(super) const COMPILER_ID: &str = "medulla.process-compile/v1";

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The id of version `version` of the workflow compiled from `process_iri`.
pub(super) fn workflow_node_id(process_iri: &str, version: u32) -> String {
    format!("workflow:{process_iri}:v{version}")
}

/// The stored version of the workflow of `process_iri`: 0 when none is stored.
pub(super) fn stored_version(core: &GraphCore, process_iri: &str) -> u32 {
    let mut version = 0u32;
    while core.has_node(&workflow_node_id(process_iri, version + 1)) {
        version += 1;
    }
    version
}

fn text<'a>(
    properties: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    properties.get(key).and_then(serde_json::Value::as_str)
}

/// The tenant-visible process view: every node the tenant may see, and every `hasStep` and
/// `handsOffTo` edge. A node that carries no tenant belongs to the graph.
pub(super) fn read_view(core: &GraphCore, tenant: &str) -> (Vec<ViewNode>, Vec<ViewEdge>) {
    let nodes = core
        .get_nodes()
        .into_iter()
        .filter_map(|(id, bytes)| {
            let properties = medulla_types::msgpack::decode_property_object(&bytes).ok()?;
            if text(&properties, "tenant_id").is_some_and(|owner| owner != tenant) {
                return None;
            }
            let class = text(&properties, "type")
                .or_else(|| text(&properties, "node_type"))
                .unwrap_or_default()
                .to_string();
            let label = text(&properties, "name")
                .or_else(|| text(&properties, "label"))
                .unwrap_or(&id)
                .to_string();
            Some(ViewNode {
                class,
                label,
                role: text(&properties, "role").map(str::to_string),
                reference: text(&properties, "reference").map(str::to_string),
                id,
            })
        })
        .collect();
    let edges = core
        .get_edges()
        .into_iter()
        .filter_map(|(source, target, bytes)| {
            let properties = medulla_types::msgpack::decode_property_object(&bytes).ok()?;
            if !matches!(
                text(&properties, "relationship"),
                Some("hasStep" | "handsOffTo")
            ) {
                return None;
            }
            let flag = |key: &str| {
                properties
                    .get(key)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            };
            Some(ViewEdge {
                condition: text(&properties, "condition").map(str::to_string),
                default: flag("default"),
                loop_gate: flag("loopGate"),
                source,
                target,
            })
        })
        .collect();
    (nodes, edges)
}

/// Map a compile refusal to its wire envelope; an import error keeps its own code.
fn compile_refusal(error: &ProcessCompilationError) -> String {
    refuse(error.code(), error)
}

/// One admitted compile: the batch to commit, the answer and the version it was admitted at.
pub(super) struct Admitted {
    pub(super) write: Method,
    pub(super) answer: ProcessCompiled,
    pub(super) stored: u32,
}

/// Admit one compile against the visible view, writing nothing.
pub(super) fn admit_compile(
    core: &GraphCore,
    tenant: &str,
    process_iri: &str,
    expected_version: Option<u32>,
) -> Result<Admitted, String> {
    let annotations = bpmn_kind_annotations().map_err(|error| {
        refuse(error.code(), format!("the process annotations are unreadable: {error}"))
    })?;
    let (nodes, edges) = read_view(core, tenant);
    let CompiledProcess {
        workflow,
        process_revision,
    } = compile_process_view(
        process_iri,
        &nodes,
        &edges,
        knobs().scheduler_process_compile_max_nodes,
        annotations,
    )
    .map_err(|error| compile_refusal(&error))?;
    let stored = stored_version(core, process_iri);
    if expected_version.is_some_and(|expected| expected != stored) {
        return Err(refuse(
            WORKFLOW_VERSION_CONFLICT,
            format!(
                "process {process_iri} is at version {stored}, the caller expected {}",
                expected_version.unwrap_or_default()
            ),
        ));
    }
    let version = stored + 1;
    let workflow_id = workflow_node_id(process_iri, version);
    let canonical = serde_json::to_string(&workflow)
        .map_err(|error| refuse("PARAMETER_INVALID", format!("workflow encoding failed: {error}")))?;
    let steps: Vec<&str> = workflow.steps.iter().map(|step| step.id.as_str()).collect();
    let operations = vec![
        serde_json::json!({
            "op": "upsert_node",
            "id": workflow_id,
            "properties": {
                "type": "WorkflowPattern",
                "tenant_id": tenant,
                "process_iri": process_iri,
                "version": version,
                "steps": steps,
                "workflowDigest": workflow.digest,
                "processRevision": process_revision,
                "canonical": canonical,
            },
        }),
        serde_json::json!({
            "op": "upsert_edge",
            "source": workflow_id,
            "target": process_iri,
            "properties": {
                "relationship": "compiledFrom",
                "processRevision": process_revision,
                "compilerId": COMPILER_ID,
                "workflowDigest": workflow.digest,
            },
        }),
    ];
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| refuse("PARAMETER_INVALID", format!("compile encoding failed: {error}")))?;
    Ok(Admitted {
        write: Method::BatchUpdate { operations_msgpack },
        answer: ProcessCompiled {
            workflow_id,
            version,
            workflow_digest: workflow.digest,
            process_revision,
        },
        stored,
    })
}

/// The `Route::ProcessCompile` entry: admit the compile, then commit the batch through the
/// staged gateway.
pub(super) async fn handle_process_compile(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::ProcessCompile {
        process_iri,
        expected_version,
    } = method
    else {
        return None;
    };
    if process_iri.is_empty() {
        return Some(Response::err(
            ctx.req_id,
            refuse("PARAMETER_INVALID", "process_iri is empty"),
        ));
    }
    let tenant = ctx.tenant_scope;
    let Admitted {
        write,
        answer,
        stored,
    } = match admit_compile(ctx.core, tenant, process_iri, *expected_version) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("admit_compile builds a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let plan = MutationPlan::for_method(&write);
    let process_iri = process_iri.clone();
    Some(
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
            let current = stored_version(core, &process_iri);
            if current != stored {
                return Err(refuse(
                    WORKFLOW_VERSION_CONFLICT,
                    format!(
                        "process {process_iri} moved to version {current} while version {} was compiled",
                        stored + 1
                    ),
                ));
            }
            medulla_graph_algos::algorithms::batch_update(core, &operations)
                .and_then(|_| ResultPayload::of::<ProcessCompile>(answer))
        })
        .await,
    )
}
