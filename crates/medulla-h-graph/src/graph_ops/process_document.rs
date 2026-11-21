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
//! refuses writes nothing.

use super::process_compile::{batch, commit_version, refuse, Admitted};
use super::*;
use medulla_core::architecture_exchange::graph::ExchangeMutation;
use medulla_core::process_exchange::bpmn_graph::bpmn_mutations;
use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::dmn_graph::dmn_mutations;
use medulla_core::process_exchange::dmn_read::parse_dmn;
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_protocol_types::result_contract::coordination::ProcessDocumentImport;
use medulla_server_api::knob_config::knobs;
use medulla_types::archimate_exchange::sha256_hex;
use medulla_types::process_exchange::{
    ProcessDocument, ProcessDocumentFormat, ProcessDocumentImported,
};
use medulla_types::process_import::WORKFLOW_VERSION_CONFLICT;

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
                .map_err(|error| refuse("INVALID_ARGUMENT", error))
        }
        ProcessDocumentFormat::Dmn13 => {
            let limits = XmlLimits {
                max_bytes,
                max_depth,
            };
            parse_dmn(&document.document, limits)
                .map(|parsed| dmn_mutations(&parsed, model_iri))
                .map_err(|error| refuse("INVALID_ARGUMENT", error))
        }
    }
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
    let mut operations = vec![serde_json::json!({
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
        },
    })];
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
