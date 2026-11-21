//! Served process document import (MEDULLA-CQ-ORCH-R702.9).
//!
//! `ProcessDocumentImport` is gateway-routed beside `ProcessModelImport`
//! (`Route::ProcessDocumentImport`). The document is read natively: a BPMN 2.0 document with
//! `parse_bpmn` and `bpmn_mutations`, a DMN 1.3 document with `parse_dmn` and `dmn_mutations`,
//! both under the scheduler's document byte and depth knobs. The element nodes and the model
//! node `processmodel:<model_iri>:v<n>` commit as the one `BatchUpdate` through the staged
//! gateway, under the same compare-and-set as a workflow version: a version that is not the
//! stored one refuses `WORKFLOW_VERSION_CONFLICT`, and the commit closure re-reads the stored
//! version so two racing imports never both write version `n + 1`. A document the reader
//! refuses writes nothing. A re-import replaces the model: the previous version's element nodes
//! the new document no longer has are removed, and so are its edges, before the new ones are
//! written; the model node's `elementIds` records the exact element set of its version.
//!
//! `ProcessDocumentExport` (MEDULLA-CQ-ORCH-R702.10, R702.11) is the tenant-bound read: a model
//! is rebuilt from its element nodes and edges with `bpmn_document` or `dmn_document` and
//! written with `write_bpmn` or `write_dmn`; a stored workflow goes through `workflow_to_bpmn`
//! and then `write_bpmn`.

use super::process_compile::{batch, commit_version, refuse, stored_workflow, Admitted};
use super::terminal::GraphOpsContext;
use super::*;
use medulla_core::architecture_exchange::graph::ExchangeMutation;
use medulla_core::process_exchange::bpmn_graph::{bpmn_document, bpmn_mutations};
use medulla_core::process_exchange::bpmn_write::write_bpmn;
use medulla_core::process_exchange::dmn_graph::dmn_document;
use medulla_core::process_exchange::dmn_write::write_dmn;
use medulla_core::process_exchange::workflow_bpmn::workflow_to_bpmn;
use medulla_core::process_exchange::ProcessExchangeError;
use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::dmn_graph::dmn_mutations;
use medulla_core::process_exchange::dmn_read::parse_dmn;
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_protocol_types::result_contract::coordination::{
    ProcessDocumentExport, ProcessDocumentImport,
};
use medulla_server_api::knob_config::knobs;
use medulla_types::archimate_exchange::sha256_hex;
use medulla_types::process_exchange::{
    ExportSource, ProcessDocument, ProcessDocumentExported, ProcessDocumentFormat,
    ProcessDocumentImported,
};
use medulla_types::process_import::{
    PROCESS_EXPORT_NOT_DEPLOYABLE, PROCESS_IMPORT_UNKNOWN_MODEL, WORKFLOW_VERSION_CONFLICT,
};

/// The id of version `version` of the model `model_iri`.
pub(super) fn model_node_id(model_iri: &str, version: u32) -> String {
    format!("processmodel:{model_iri}:v{version}")
}

/// The stored version of the model `model_iri`: 0 when none is stored.
pub(super) fn stored_model_version(core: &GraphCore, model_iri: &str) -> u32 {
    let mut version = 0u32;
    while core.has_node(&model_node_id(model_iri, version + 1)) {
        version += 1;
    }
    version
}

/// The mapping of one document onto graph writes, or the reader's refusal.
fn document_mutations(
    document: &ProcessDocument,
    model_iri: &str,
) -> Result<Vec<ExchangeMutation>, String> {
    let configured = knobs();
    let (max_bytes, max_depth) = (
        configured.scheduler_process_document_max_bytes,
        configured.scheduler_process_document_max_depth,
    );
    match document.format {
        ProcessDocumentFormat::Bpmn20 => {
            let limits = BpmnLimits {
                max_bytes,
                max_depth,
            };
            parse_bpmn(&document.document, limits)
                .map(|parsed| bpmn_mutations(&parsed, model_iri))
                .map_err(|error| exchange_refusal(&error))
        }
        ProcessDocumentFormat::Dmn13 => {
            let limits = XmlLimits {
                max_bytes,
                max_depth,
            };
            parse_dmn(&document.document, limits)
                .map(|parsed| dmn_mutations(&parsed, model_iri))
                .map_err(|error| exchange_refusal(&error))
        }
    }
}

/// The wire refusal of a reader or writer error.
fn exchange_refusal(error: &ProcessExchangeError) -> String {
    match error {
        ProcessExchangeError::NotDeployable { .. } => {
            refuse(PROCESS_EXPORT_NOT_DEPLOYABLE, error)
        }
        _ => refuse("INVALID_ARGUMENT", error),
    }
}

/// The element node ids a stored model version records.
fn recorded_elements(properties: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    properties
        .get("elementIds")
        .and_then(serde_json::Value::as_str)
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

/// The decoded properties of one stored node.
fn node_properties(
    core: &GraphCore,
    id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let blob = core.get_node_properties(id)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

/// Admit one document import, writing nothing.
pub(super) fn admit_document_import(
    core: &GraphCore,
    tenant: &str,
    document: &ProcessDocument,
    model_iri: &str,
    expected_version: Option<u32>,
) -> Result<Admitted<ProcessDocumentImported>, String> {
    let mutations = document_mutations(document, model_iri)?;
    let stored = stored_model_version(core, model_iri);
    if let Some(expected) = expected_version {
        if expected != stored {
            return Err(refuse(
                WORKFLOW_VERSION_CONFLICT,
                format!("model {model_iri} is at version {stored}, the caller expected {expected}"),
            ));
        }
    }
    let version = stored + 1;
    let digest = sha256_hex(&document.document);
    let format = serde_json::to_value(document.format)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let node_id = model_node_id(model_iri, version);
    let mut element_ids: Vec<&str> = mutations
        .iter()
        .filter_map(|mutation| match mutation {
            ExchangeMutation::UpsertNode { id, .. } => Some(id.as_str()),
            ExchangeMutation::Link { .. } => None,
        })
        .collect();
    element_ids.sort_unstable();
    element_ids.dedup();
    // The previous version's edges and the nodes the new document no longer has go first, so
    // the stored element set is exactly the new document's.
    let mut operations: Vec<serde_json::Value> = Vec::new();
    let previous = node_properties(core, &model_node_id(model_iri, stored))
        .map(|properties| recorded_elements(&properties))
        .unwrap_or_default();
    for from in &previous {
        for to in core.get_successors(from).unwrap_or_default() {
            if previous.contains(&to) {
                operations.push(serde_json::json!({"op": "remove_edge", "source": from, "target": to}));
            }
        }
    }
    for gone in previous.iter().filter(|id| !element_ids.contains(&id.as_str())) {
        operations.push(serde_json::json!({"op": "remove_node", "id": gone}));
    }
    operations.push(serde_json::json!({
        "op": "upsert_node",
        "id": node_id,
        "properties": {
            "type": "ProcessModel",
            "tenant_id": tenant,
            "modelIri": model_iri,
            "version": version,
            "format": format,
            "importedFrom": digest,
            "sourceDocumentDigest": digest,
            "elementIds": serde_json::to_string(&element_ids).unwrap_or_default(),
        },
    }));
    let (mut nodes, mut edges) = (0u64, 0u64);
    for mutation in &mutations {
        operations.push(match mutation {
            ExchangeMutation::UpsertNode { id, properties, .. } => {
                nodes += 1;
                serde_json::json!({"op": "upsert_node", "id": id, "properties": properties})
            }
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => {
                edges += 1;
                serde_json::json!({
                    "op": "upsert_edge",
                    "source": source,
                    "target": target,
                    "properties": {"relationship": predicate},
                })
            }
        });
    }
    Ok(Admitted {
        write: batch(&operations)?,
        answer: ProcessDocumentImported {
            model_node_id: node_id,
            version,
            format: document.format,
            nodes,
            edges,
            source_document_digest: digest,
        },
        stored,
    })
}

/// The `Route::ProcessDocumentImport` entry.
pub(super) async fn handle_process_document_import(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::ProcessDocumentImport {
        document,
        model_iri,
        expected_version,
    } = method
    else {
        return None;
    };
    if model_iri.is_empty() {
        return Some(Response::err(
            ctx.req_id,
            refuse("PARAMETER_INVALID", "model_iri is empty"),
        ));
    }
    let admitted = match admit_document_import(
        ctx.core,
        ctx.tenant_scope,
        document,
        model_iri,
        *expected_version,
    ) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    Some(
        commit_version(ctx, admitted, model_iri.clone(), stored_model_version, |answer| {
            ResultPayload::of::<ProcessDocumentImport>(answer)
        })
        .await,
    )
}

/// The element writes that stored the latest version of the model `model_iri`, read back from
/// the graph; the model must be of the tenant and of `format`.
fn stored_model_mutations(
    core: &GraphCore,
    tenant: &str,
    model_iri: &str,
    format: ProcessDocumentFormat,
) -> Result<Vec<ExchangeMutation>, String> {
    let unknown = || {
        refuse(
            PROCESS_IMPORT_UNKNOWN_MODEL,
            format!("no model {model_iri} is available to the caller's tenant"),
        )
    };
    let version = stored_model_version(core, model_iri);
    if version == 0 {
        return Err(unknown());
    }
    let model = node_properties(core, &model_node_id(model_iri, version)).ok_or_else(unknown)?;
    let text = |key: &str| model.get(key).and_then(serde_json::Value::as_str);
    if text("type") != Some("ProcessModel") || text("tenant_id").is_some_and(|t| t != tenant) {
        return Err(unknown());
    }
    let stored_format = serde_json::to_value(format)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    if text("format") != Some(stored_format.as_str()) {
        return Err(refuse(
            PROCESS_EXPORT_NOT_DEPLOYABLE,
            format!(
                "model {model_iri} was imported as {}, not {stored_format}",
                text("format").unwrap_or_default()
            ),
        ));
    }
    let ids = recorded_elements(&model);
    let mut out = Vec::new();
    for id in &ids {
        let Some(properties) = node_properties(core, id) else {
            continue;
        };
        let strings: std::collections::BTreeMap<String, String> = properties
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|v| (key.clone(), v.to_string())))
            .collect();
        out.push(ExchangeMutation::UpsertNode {
            id: id.clone(),
            label: strings.get("type").cloned().unwrap_or_default(),
            properties: strings,
        });
    }
    for from in &ids {
        for to in core.get_successors(from).unwrap_or_default() {
            if !ids.contains(&to) {
                continue;
            }
            for blob in core.get_edge_properties(from, &to) {
                let relationship = medulla_types::msgpack::decode_property_object(&blob)
                    .ok()
                    .and_then(|edge| {
                        edge.get("relationship")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    });
                if let Some(predicate) = relationship {
                    out.push(ExchangeMutation::Link {
                        source: from.clone(),
                        target: to.clone(),
                        predicate,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Answer one document export, writing nothing.
pub(super) fn export_document(
    core: &GraphCore,
    tenant: &str,
    source: &ExportSource,
    format: ProcessDocumentFormat,
) -> Result<ProcessDocumentExported, String> {
    let bytes = match (source, format) {
        (ExportSource::Model { model_iri }, ProcessDocumentFormat::Bpmn20) => {
            let stored = stored_model_mutations(core, tenant, model_iri, format)?;
            bpmn_document(&stored, model_iri)
                .and_then(|doc| write_bpmn(&doc))
                .map_err(|error| exchange_refusal(&error))?
        }
        (ExportSource::Model { model_iri }, ProcessDocumentFormat::Dmn13) => {
            let stored = stored_model_mutations(core, tenant, model_iri, format)?;
            dmn_document(&stored, model_iri)
                .and_then(|doc| write_dmn(&doc))
                .map_err(|error| exchange_refusal(&error))?
        }
        (
            ExportSource::Workflow {
                workflow_id,
                version,
            },
            ProcessDocumentFormat::Bpmn20,
        ) => {
            let workflow = stored_workflow(core, tenant, workflow_id, *version)?;
            workflow_to_bpmn(&workflow)
                .and_then(|doc| write_bpmn(&doc))
                .map_err(|error| exchange_refusal(&error))?
        }
        (ExportSource::Workflow { .. }, ProcessDocumentFormat::Dmn13) => {
            return Err(refuse(
                PROCESS_EXPORT_NOT_DEPLOYABLE,
                "a workflow cannot be written as a DMN document",
            ));
        }
    };
    let document_digest = sha256_hex(&bytes);
    Ok(ProcessDocumentExported {
        document: ProcessDocument {
            schema_version: medulla_types::process_exchange::PROCESS_EXCHANGE_SCHEMA_VERSION,
            format,
            document: bytes,
            source_ref: None,
        },
        document_digest,
    })
}

/// Route the document export read served from the graph.
pub(super) async fn try_handle_process_document_export(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ProcessDocumentExport { source, format } = &method else {
        return ControlFlow::Continue(method);
    };
    let tenant = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_owned())
        .unwrap_or_default();
    let response = match export_document(ctx.core, &tenant, source, *format) {
        Ok(exported) => Response::ok(
            ctx.req_id,
            ResultPayload::of::<ProcessDocumentExport>(exported),
        ),
        Err(error) => Response::err(ctx.req_id, error),
    };
    ControlFlow::Break(response)
}
