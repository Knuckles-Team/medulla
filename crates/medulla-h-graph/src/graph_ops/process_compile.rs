//! Served process compile and canonical workflow import and export (MEDULLA-CQ-ORCH-R700.3,
//! R700.4, R700.5, R701.3).
//!
//! `ProcessCompile` is gateway-routed (`Route::ProcessCompile`). The handler reads the
//! caller's tenant-visible process view (the `hasStep` and `handsOffTo` edges and the nodes
//! they join), compiles it with `compile_process_view` and writes the next
//! `WorkflowPattern` node `workflow:<process_iri>:v<n>` and its `compiledFromProcess` edge to the
//! process node as the one `BatchUpdate` through the staged gateway. The write is a
//! compare-and-set on `expected_version`: a version that is not the stored one refuses
//! `WORKFLOW_VERSION_CONFLICT`, and the commit closure re-reads the stored version so two
//! racing compiles never both write version `n + 1`. A published version is never mutated.
//! A refusal writes nothing. The `compiledFromProcess` edge carries `processRevision`, `compilerId`
//! and `workflowDigest`; the answer returns the digest, the version and the process revision.
//!
//! `ProcessModelImport` is gateway-routed beside it (`Route::ProcessModelImport`): the stated
//! canonical workflow is admitted with `admit_canonical_with_limit` and committed as the next
//! version of `workflow:<workflow_id>:v<n>` under the same compare-and-set. `ProcessModelExport`
//! is a tenant-bound graph read of one stored version: the canonical form, the deployable BPMN
//! document for `bpmn20`, or the canonical form with the losses of the target.

use super::*;
use medulla_core::process_compile::{
    compile_process_view, CompiledProcess, ProcessCompilationError, ViewEdge, ViewNode,
};
use super::terminal::GraphOpsContext;
use medulla_core::process_exchange::bpmn_lower::bpmn_kind_annotations;
use medulla_core::process_exchange::bpmn_write::write_bpmn;
use medulla_core::process_exchange::workflow_bpmn::workflow_to_bpmn;
use medulla_core::process_import::{admit_canonical_with_limit, export_canonical_strict};
use medulla_protocol_types::result_contract::coordination::{
    ProcessCompile, ProcessModelExport, ProcessModelImport,
};
use medulla_server_api::knob_config::knobs;
use medulla_types::process_import::{
    CanonicalWorkflow, ImportError, ProcessCompiled, ProcessModelExported, ProcessModelImported,
    ProcessSourceSystem, stored_workflow_digest, PROCESS_EXPORT_NOT_DEPLOYABLE,
    WORKFLOW_VERSION_CONFLICT,
};

/// The id of the compiler recorded on every `compiledFromProcess` edge.
pub(super) const COMPILER_ID: &str = "medulla.process-compile/v1";

pub(super) fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

pub(super) use medulla_types::process_import::workflow_node_id;

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

/// The compare-and-set of the version store: the stored version of `key`, or
/// `WORKFLOW_VERSION_CONFLICT` when the caller expected another.
fn check_version(
    core: &GraphCore,
    key: &str,
    expected_version: Option<u32>,
) -> Result<u32, String> {
    let stored = stored_version(core, key);
    match expected_version {
        Some(expected) if expected != stored => Err(refuse(
            WORKFLOW_VERSION_CONFLICT,
            format!("workflow {key} is at version {stored}, the caller expected {expected}"),
        )),
        _ => Ok(stored),
    }
}

/// The `upsert_node` operation of one stored workflow version.
fn workflow_node(
    tenant: &str,
    key: &str,
    version: u32,
    workflow: &CanonicalWorkflow,
    preconditions: &str,
    extra: &[(&str, &str)],
) -> Result<serde_json::Value, String> {
    let canonical = serde_json::to_string(workflow).map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("workflow encoding failed: {error}"),
        )
    })?;
    let steps: Vec<&str> = workflow.steps.iter().map(|step| step.id.as_str()).collect();
    let mut properties = serde_json::json!({
        "type": "WorkflowPattern",
        "tenant_id": tenant,
        "process_iri": key,
        "version": version,
        "steps": steps,
        "workflowDigest": stored_workflow_digest(&workflow.digest),
        "canonical": canonical,
        // MEDULLA-ORCH-VOCAB-R800.3: the JSON text of the preconditions; not part of the digest.
        "preconditions": preconditions,
    });
    for (name, value) in extra {
        properties[*name] = serde_json::Value::String((*value).to_string());
    }
    Ok(serde_json::json!({
        "op": "upsert_node",
        "id": workflow_node_id(key, version),
        "properties": properties,
    }))
}

/// One admitted write: the batch to commit, the answer and the version it was admitted at.
pub(super) struct Admitted<A> {
    pub(super) write: Method,
    pub(super) answer: A,
    pub(super) stored: u32,
}

pub(super) fn batch(operations: &[serde_json::Value]) -> Result<Method, String> {
    let operations_msgpack = rmp_serde::to_vec_named(operations).map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("workflow encoding failed: {error}"),
        )
    })?;
    Ok(Method::BatchUpdate { operations_msgpack })
}

/// The most preconditions a stored pattern holds and the longest IRI of one (MEDULLA-ORCH-VOCAB-R800.3).
const MAX_PRECONDITIONS: usize = 32;
const MAX_PRECONDITION_IRI_BYTES: usize = 512;

/// The JSON text of the preconditions a pattern is stored with, or `PARAMETER_INVALID` when
/// there are too many or one is too long.
fn preconditions_text(
    preconditions: &[medulla_work_types::orchestration::definitions::PatternPrecondition],
) -> Result<String, String> {
    use medulla_work_types::orchestration::definitions::PatternPrecondition;
    let too_long = preconditions.iter().any(|precondition| match precondition {
        PatternPrecondition::Capability(iri) | PatternPrecondition::Fact(iri) => {
            iri.chars().count() > MAX_PRECONDITION_IRI_BYTES
        }
    });
    if preconditions.len() > MAX_PRECONDITIONS || too_long {
        return Err(refuse(
            "PARAMETER_INVALID",
            format!(
                "preconditions hold at most {MAX_PRECONDITIONS} entries of at most {MAX_PRECONDITION_IRI_BYTES} characters"
            ),
        ));
    }
    serde_json::to_string(preconditions).map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("preconditions encoding failed: {error}"),
        )
    })
}

/// Admit one compile against the visible view, writing nothing.
pub(super) fn admit_compile(
    core: &GraphCore,
    tenant: &str,
    process_iri: &str,
    expected_version: Option<u32>,
    preconditions: &[medulla_work_types::orchestration::definitions::PatternPrecondition],
) -> Result<Admitted<ProcessCompiled>, String> {
    let preconditions = preconditions_text(preconditions)?;
    let annotations = bpmn_kind_annotations().map_err(|error| {
        refuse(
            error.code(),
            format!("the process annotations are unreadable: {error}"),
        )
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
    let stored = check_version(core, process_iri, expected_version)?;
    let version = stored + 1;
    let workflow_id = workflow_node_id(process_iri, version);
    let operations = vec![
        workflow_node(
            tenant,
            process_iri,
            version,
            &workflow,
            &preconditions,
            &[("processRevision", process_revision.as_str())],
        )?,
        serde_json::json!({
            "op": "upsert_edge",
            "source": workflow_id,
            "target": process_iri,
            "properties": {
                "relationship": "compiledFromProcess",
                "processRevision": process_revision,
                "compilerId": COMPILER_ID,
                "workflowDigest": stored_workflow_digest(&workflow.digest),
            },
        }),
    ];
    Ok(Admitted {
        write: batch(&operations)?,
        answer: ProcessCompiled {
            workflow_id,
            version,
            workflow_digest: stored_workflow_digest(&workflow.digest),
            process_revision,
        },
        stored,
    })
}

/// Admit one canonical import, writing nothing.
pub(super) fn admit_import(
    core: &GraphCore,
    tenant: &str,
    workflow: &CanonicalWorkflow,
    workflow_id: &str,
    expected_version: Option<u32>,
    preconditions: &[medulla_work_types::orchestration::definitions::PatternPrecondition],
) -> Result<Admitted<ProcessModelImported>, String> {
    let preconditions = preconditions_text(preconditions)?;
    let admitted = admit_canonical_with_limit(
        workflow,
        knobs().scheduler_process_import_max_elements,
    )
    .map_err(|error: ImportError| refuse(error.code(), error))?;
    let stored = check_version(core, workflow_id, expected_version)?;
    let version = stored + 1;
    let annex = serde_json::to_string(&admitted.annex).map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("annex encoding failed: {error}"),
        )
    })?;
    let node_id = workflow_node_id(workflow_id, version);
    let system = serde_json::to_value(admitted.source.system)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let mut operations = vec![workflow_node(
        tenant,
        workflow_id,
        version,
        &admitted,
        &preconditions,
        &[
            ("importedFrom", admitted.source.model_id.as_str()),
            ("processAnnex", annex.as_str()),
        ],
    )?];
    for step in &admitted.steps {
        let step_id = format!("{node_id}:step:{}", step.id);
        operations.push(serde_json::json!({
            "op": "upsert_node",
            "id": step_id,
            "properties": {
                "type": "WorkflowStep",
                "tenant_id": tenant,
                "stepId": step.id,
                "label": step.label,
                "sourceSystem": system,
                "sourceModelId": admitted.source.model_id,
                "sourceElementId": admitted.source.elements.get(&step.id),
            },
        }));
        operations.push(serde_json::json!({
            "op": "upsert_edge",
            "source": node_id,
            "target": step_id,
            "properties": { "relationship": "hasStep" },
        }));
    }
    Ok(Admitted {
        write: batch(&operations)?,
        answer: ProcessModelImported {
            workflow_id: workflow_node_id(workflow_id, version),
            version,
            workflow_digest: stored_workflow_digest(&admitted.digest),
        },
        stored,
    })
}

/// Commit one admitted version through the staged gateway. The commit closure re-reads the
/// stored version with `stored_of` so two racing writers never both write version `n + 1`.
pub(super) async fn commit_version<A>(
    ctx: &MutationCtx<'_>,
    admitted: Admitted<A>,
    key: String,
    stored_of: fn(&GraphCore, &str) -> u32,
    seal: impl FnOnce(A) -> Result<ResultPayload, String>,
) -> Response {
    let Admitted {
        write,
        answer,
        stored,
    } = admitted;
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("an admitted version is a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let plan = MutationPlan::for_method(&write);
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
        let current = stored_of(core, &key);
        if current != stored {
            return Err(refuse(
                WORKFLOW_VERSION_CONFLICT,
                format!(
                    "workflow {key} moved to version {current} while version {} was admitted",
                    stored + 1
                ),
            ));
        }
        medulla_graph_algos::algorithms::batch_update(core, &operations)
            .and_then(|_| seal(answer))
    })
    .await
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
        preconditions,
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
    let admitted = match admit_compile(
        ctx.core,
        ctx.tenant_scope,
        process_iri,
        *expected_version,
        preconditions,
    ) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    Some(
        commit_version(ctx, admitted, process_iri.clone(), stored_version, |answer| {
            ResultPayload::of::<ProcessCompile>(answer)
        })
        .await,
    )
}

/// The `Route::ProcessModelImport` entry: admit the canonical document, then commit the next
/// version through the staged gateway.
pub(super) async fn handle_process_model_import(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::ProcessModelImport {
        workflow,
        workflow_id,
        expected_version,
        preconditions,
    } = method
    else {
        return None;
    };
    if workflow_id.is_empty() {
        return Some(Response::err(
            ctx.req_id,
            refuse("PARAMETER_INVALID", "workflow_id is empty"),
        ));
    }
    let admitted = match admit_import(
        ctx.core,
        ctx.tenant_scope,
        workflow,
        workflow_id,
        *expected_version,
        preconditions,
    ) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    Some(
        commit_version(ctx, admitted, workflow_id.clone(), stored_version, |answer| {
            ResultPayload::of::<ProcessModelImport>(answer)
        })
        .await,
    )
}

/// The one non-disclosing refusal of an absent, foreign or malformed stored workflow.
fn unknown_model(workflow_id: &str, version: u32) -> String {
    let error = ImportError::UnknownModel {
        workflow_id: format!("{workflow_id}:v{version}"),
    };
    refuse(
        error.code(),
        format!("no workflow {workflow_id} version {version} is available to the caller's tenant"),
    )
}

/// The stored canonical workflow `workflow_id` at `version` that `tenant` may see.
pub(super) fn stored_workflow(
    core: &GraphCore,
    tenant: &str,
    workflow_id: &str,
    version: u32,
) -> Result<CanonicalWorkflow, String> {
    let unknown = || unknown_model(workflow_id, version);
    let blob = core
        .get_node_properties(&workflow_node_id(workflow_id, version))
        .ok_or_else(unknown)?;
    let properties =
        medulla_types::msgpack::decode_property_object(&blob).map_err(|_| unknown())?;
    if text(&properties, "type") != Some("WorkflowPattern")
        || text(&properties, "tenant_id").is_some_and(|owner| owner != tenant)
    {
        return Err(unknown());
    }
    let canonical = text(&properties, "canonical").ok_or_else(unknown)?;
    serde_json::from_str(canonical).map_err(|_| unknown())
}

/// Answer one export of a stored workflow version.
pub(super) fn export_model(
    core: &GraphCore,
    tenant: &str,
    workflow_id: &str,
    version: u32,
    target: Option<ProcessSourceSystem>,
    strict: bool,
) -> Result<ProcessModelExported, String> {
    let stored = stored_workflow(core, tenant, workflow_id, version)?;
    let Some(target) = target else {
        return Ok(ProcessModelExported {
            workflow: stored,
            document: None,
        });
    };
    let workflow = export_canonical_strict(&stored, target, strict).map_err(|lossy| {
        let named: Vec<String> = lossy
            .losses
            .iter()
            .map(|loss| format!("{}@{}", loss.construct, loss.element))
            .collect();
        refuse(lossy.code(), named.join(", "))
    })?;
    let document = if target == ProcessSourceSystem::Bpmn20 {
        let bpmn = workflow_to_bpmn(&workflow)
            .and_then(|doc| write_bpmn(&doc))
            .map_err(|error| refuse(PROCESS_EXPORT_NOT_DEPLOYABLE, error))?;
        Some(String::from_utf8(bpmn).map_err(|error| {
            refuse(
                PROCESS_EXPORT_NOT_DEPLOYABLE,
                format!("the BPMN document is not text: {error}"),
            )
        })?)
    } else {
        None
    };
    Ok(ProcessModelExported { workflow, document })
}

/// Route the export read served from the graph.
pub(super) async fn try_handle_process_model_export(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ProcessModelExport {
        workflow_id,
        version,
        target,
        strict,
    } = &method
    else {
        return ControlFlow::Continue(method);
    };
    let tenant = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_owned())
        .unwrap_or_default();
    let response = match export_model(ctx.core, &tenant, workflow_id, *version, *target, *strict) {
        Ok(exported) => Response::ok(
            ctx.req_id,
            ResultPayload::of::<ProcessModelExport>(exported),
        ),
        Err(error) => Response::err(ctx.req_id, error),
    };
    ControlFlow::Break(response)
}
