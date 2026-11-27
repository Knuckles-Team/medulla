//! The process model sync adapter (MEDULLA-CQ-ORCH-R703.2): the [`SyncAdapter`] of the
//! classes `:ProcessModel` and `:DecisionLogic`.
//!
//! A record is one model definition as the process engine connector delivers it:
//! `{system: "bpmn20" | "aris_epc" | "dmn", model_key, version, document?, canonical?}`. The
//! key is `<system>:<model_key>:<version>`. The digest of a BPMN or EPC record is the digest of
//! the canonical workflow it compiles to, so one process in either format compares equal; the
//! digest of a DMN record is its model digest. A record that does not parse or admit digests as
//! `invalid:<code>`, so it reconciles as a source change and the inbound write is refused by
//! the mapping. Inbound changes write the same nodes and edges as the exchange mapping
//! (`bpmn_mutations`, `dmn_mutations`), under one model node per record; `graph_records`
//! rebuilds each record from those nodes and edges.

use std::collections::BTreeMap;

use medulla_core::architecture_exchange::graph::ExchangeMutation;
use medulla_core::process_exchange::bpmn_graph::{bpmn_document, bpmn_mutations};
use medulla_core::process_exchange::bpmn_lower::{bpmn_kind_annotations, bpmn_source_graph};
use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::bpmn_write::write_bpmn;
use medulla_core::process_exchange::dmn_graph::{dmn_document, dmn_mutations};
use medulla_core::process_exchange::dmn_read::parse_dmn;
use medulla_core::process_exchange::dmn_write::{dmn_model_digest, write_dmn};
use medulla_core::process_exchange::workflow_bpmn::workflow_to_bpmn;
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_core::process_import::{admit_canonical, compile_process, export_canonical};
use medulla_source_types::source_sync::{OutboundDraft, SyncReport};
use medulla_types::contract::Digest256;
use medulla_types::contract::ServerErrorCode;
use medulla_types::process_exchange::{
    ProcessDocumentExported, PROCESS_MODEL_CLASS_IRI, PROCESS_SYNC_SOURCE_NOT_SECONDARY,
};
use medulla_types::process_import::{CanonicalWorkflow, ProcessSourceSystem};
use serde_json::{json, Map, Value};

use crate::source_sync::{
    GraphEdges, GraphNodes, RecordRole, SyncAdapter, SyncAuthority, SyncEdge, SyncMutation,
};

/// The `type` of the node that holds one synced model.
const MODEL_TYPE: &str = "ProcessModel";

/// The systems a record names.
const BPMN: &str = "bpmn20";
const EPC: &str = "aris_epc";
const DMN: &str = "dmn";

/// The answer of one `ProcessModelDeploy` before the router queues it (MEDULLA-CQ-ORCH-R703.3):
/// a sync report that proposes exactly one outbound change, the export of `model_iri`. The
/// draft's record key is the model IRI and its payload digest the SHA-256 of the exported
/// document, so the write-back queue the router derives from it names the document. Nothing
/// here calls the source: the connector deploys the proposal once it is approved and released.
pub fn deploy_report(
    model_iri: &str,
    connection_iri: &str,
    connector_operation: Option<String>,
    exported: &ProcessDocumentExported,
) -> SyncReport {
    SyncReport {
        connection_iri: connection_iri.to_string(),
        class_iri: PROCESS_MODEL_CLASS_IRI.to_string(),
        unchanged: 0,
        applied: 0,
        proposed: 1,
        conflicts: Vec::new(),
        outbound: vec![OutboundDraft {
            record_key: model_iri.to_string(),
            payload_digest: exported.document_digest.clone(),
            connector_operation,
        }],
        queued: Vec::new(),
        queue_id: None,
        skipped: Vec::new(),
        by_type: BTreeMap::new(),
    }
}

/// Whether `class_iri` is one of the classes a process sync serves: `:ProcessModel` and
/// `:DecisionLogic` of the core ontology.
pub fn is_process_class(class_iri: &str) -> bool {
    class_iri
        .strip_prefix(CORE_CLASS_BASE)
        .is_some_and(|label| matches!(label, "ProcessModel" | "DecisionLogic"))
}

/// The refusal text of a process sync or deploy against `connection_iri`
/// (MEDULLA-CQ-ORCH-R703.4): the connection is not a federated secondary for the class.
pub fn not_secondary(connection_iri: &str, class_iri: &str) -> String {
    format!("{PROCESS_SYNC_SOURCE_NOT_SECONDARY}: {connection_iri} is not a federated secondary for {class_iri}")
}

/// Admit a deploy to `connection_iri` against the authority rows (MEDULLA-CQ-ORCH-R703.4): the
/// connection must be a federated secondary for `:ProcessModel`, and its direction must allow
/// outbound changes. A refusal is decided before the model is read, so it writes nothing and
/// queues nothing.
pub fn require_deploy_secondary(
    authorities: &[SyncAuthority],
    connection_iri: &str,
) -> Result<Option<String>, String> {
    let secondary = authorities
        .iter()
        .find(|row| {
            row.class == PROCESS_MODEL_CLASS_IRI
                && row.role == RecordRole::FederatedSecondary
                && row.connection == connection_iri
        })
        .ok_or_else(|| not_secondary(connection_iri, PROCESS_MODEL_CLASS_IRI))?;
    if secondary
        .direction
        .is_some_and(|direction| !direction.allows_outbound())
    {
        return Err(format!(
            "{}: {connection_iri} is inbound-only for {PROCESS_MODEL_CLASS_IRI}, yet a deploy is outbound",
            ServerErrorCode::SyncDirectionRefused.as_str()
        ));
    }
    Ok(secondary.deploy_operation.clone())
}

/// The base of a core ontology class IRI.
const CORE_CLASS_BASE: &str = "https://knuckles-team.github.io/ontology/core#";

/// Syncs the model definitions of one process engine connection.
#[derive(Clone, Debug)]
pub struct ProcessSyncAdapter {
    /// The scope the model nodes of this connection carry: a model of record `<key>` lives
    /// under `<model_iri>:<key>`.
    pub model_iri: String,
}

fn text<'a>(record: &'a Value, key: &str) -> &'a str {
    record.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn invalid(code: &str) -> String {
    format!("invalid:{code}")
}

/// The canonical workflow a BPMN document compiles to.
fn compile_bpmn(document: &str) -> Result<CanonicalWorkflow, String> {
    let doc = parse_bpmn(document.as_bytes(), BpmnLimits::default())
        .map_err(|error| error.wire_code().to_string())?;
    let graph = bpmn_source_graph(&doc).map_err(|error| error.code().to_string())?;
    let annotations = bpmn_kind_annotations().map_err(|error| error.code().to_string())?;
    compile_process(&graph, annotations).map_err(|error| error.code().to_string())
}

/// The canonical workflow of a record: compiled from its BPMN document, or admitted from its
/// canonical form.
fn workflow_of(record: &Value) -> Result<CanonicalWorkflow, String> {
    match text(record, "system") {
        BPMN => compile_bpmn(text(record, "document")),
        EPC => {
            let canonical: CanonicalWorkflow =
                serde_json::from_value(record.get("canonical").cloned().unwrap_or(Value::Null))
                    .map_err(|_| "PROCESS_IMPORT_NOT_CANONICAL".to_string())?;
            admit_canonical(&canonical).map_err(|error| error.code().to_string())
        }
        _ => Err("PROCESS_SYNC_UNKNOWN_SYSTEM".to_string()),
    }
}

impl ProcessSyncAdapter {
    /// The model iri of a record: `<model_iri>:<system>:<model_key>`.
    fn model_of(&self, record: &Value) -> String {
        format!(
            "{}:{}:{}",
            self.model_iri,
            text(record, "system"),
            text(record, "model_key")
        )
    }

    /// The scope the element nodes of a record carry: its model iri and version.
    fn scope_of(&self, record: &Value) -> String {
        format!("{}:v{}", self.model_of(record), text(record, "version"))
    }

    /// The element writes that store a record, and the BPMN or DMN scope they were mapped under.
    fn writes(&self, record: &Value) -> Result<Vec<ExchangeMutation>, String> {
        let scope = self.scope_of(record);
        match text(record, "system") {
            BPMN | EPC => {
                let document = match text(record, "system") {
                    BPMN => parse_bpmn(text(record, "document").as_bytes(), BpmnLimits::default())
                        .map_err(|error| error.wire_code().to_string())?,
                    _ => workflow_to_bpmn(&workflow_of(record)?)
                        .map_err(|error| error.wire_code().to_string())?,
                };
                Ok(bpmn_mutations(&document, &scope))
            }
            DMN => {
                let document = parse_dmn(text(record, "document").as_bytes(), XmlLimits::default())
                    .map_err(|error| error.wire_code().to_string())?;
                Ok(dmn_mutations(&document, &scope))
            }
            _ => Err("PROCESS_SYNC_UNKNOWN_SYSTEM".to_string()),
        }
    }
}

impl SyncAdapter for ProcessSyncAdapter {
    fn record_key(&self, record: &Value) -> Option<String> {
        let parts = [
            text(record, "system"),
            text(record, "model_key"),
            text(record, "version"),
        ];
        parts
            .iter()
            .all(|part| !part.is_empty())
            .then(|| parts.join(":"))
    }

    fn record_digest(&self, record: &Value) -> String {
        match text(record, "system") {
            BPMN | EPC => workflow_of(record)
                .map(|workflow| workflow.digest)
                .unwrap_or_else(|code| invalid(&code)),
            DMN => parse_dmn(text(record, "document").as_bytes(), XmlLimits::default())
                .map(|document| dmn_model_digest(&document))
                .unwrap_or_else(|error| invalid(error.wire_code())),
            _ => invalid("PROCESS_SYNC_UNKNOWN_SYSTEM"),
        }
    }

    fn inbound_mutations(&self, _key: &str, record: &Value) -> Vec<SyncMutation> {
        let Ok(writes) = self.writes(record) else {
            return Vec::new();
        };
        let version = text(record, "version");
        let scope = self.scope_of(record);
        let mut element_ids: Vec<&str> = writes
            .iter()
            .filter_map(|write| match write {
                ExchangeMutation::UpsertNode { id, .. } => Some(id.as_str()),
                ExchangeMutation::Link { .. } => None,
            })
            .collect();
        element_ids.sort_unstable();
        element_ids.dedup();
        let mut model = Map::new();
        model.insert("type".into(), json!(MODEL_TYPE));
        model.insert("modelIri".into(), json!(self.model_of(record)));
        model.insert("version".into(), json!(version));
        model.insert("syncSystem".into(), json!(text(record, "system")));
        model.insert("syncModelKey".into(), json!(text(record, "model_key")));
        model.insert(
            "elementIds".into(),
            json!(serde_json::to_string(&element_ids).unwrap_or_default()),
        );
        model.insert(
            "canonicalModelDigest".into(),
            json!(medulla_core::process_exchange::canonical_model_digest(
                &writes, &scope
            )),
        );
        model.insert(
            "sourceDocumentDigest".into(),
            json!(Digest256::sha256(text(record, "document").as_bytes()).to_hex()),
        );
        let mut out = vec![SyncMutation {
            node_id: format!("processmodel:{scope}"),
            properties: model,
        }];
        for write in writes {
            if let ExchangeMutation::UpsertNode { id, properties, .. } = write {
                out.push(SyncMutation {
                    node_id: id,
                    properties: properties
                        .into_iter()
                        .map(|(name, value)| (name, Value::String(value)))
                        .collect(),
                });
            }
        }
        out
    }

    fn inbound_edges(&self, _key: &str, record: &Value) -> Vec<SyncEdge> {
        self.writes(record)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|write| match write {
                ExchangeMutation::Link {
                    source,
                    target,
                    predicate,
                } => Some(SyncEdge {
                    source,
                    target,
                    relationship: predicate,
                }),
                ExchangeMutation::UpsertNode { .. } => None,
            })
            .collect()
    }

    fn outbound_payload(&self, _key: &str, record: &Value) -> Value {
        let payload = match text(record, "system") {
            BPMN => workflow_of(record)
                .and_then(|workflow| {
                    workflow_to_bpmn(&workflow).map_err(|error| error.wire_code().to_string())
                })
                .and_then(|document| {
                    write_bpmn(&document).map_err(|error| error.wire_code().to_string())
                })
                .map(|bytes| json!({"document": String::from_utf8_lossy(&bytes)})),
            EPC => workflow_of(record).and_then(|workflow| {
                serde_json::to_value(export_canonical(&workflow, ProcessSourceSystem::ArisEpc))
                    .map(|canonical| json!({"canonical": canonical}))
                    .map_err(|_| "PROCESS_IMPORT_NOT_CANONICAL".to_string())
            }),
            DMN => parse_dmn(text(record, "document").as_bytes(), XmlLimits::default())
                .and_then(|document| write_dmn(&document))
                .map(|bytes| json!({"document": String::from_utf8_lossy(&bytes)}))
                .map_err(|error| error.wire_code().to_string()),
            _ => Err("PROCESS_SYNC_UNKNOWN_SYSTEM".to_string()),
        };
        payload.unwrap_or_else(|code| json!({"error": code}))
    }

    fn graph_records(&self, nodes: &GraphNodes, edges: &GraphEdges) -> BTreeMap<String, Value> {
        let prefix = format!("{}:", self.model_iri);
        let mut records = BTreeMap::new();
        for props in nodes.values() {
            let string = |name: &str| props.get(name).and_then(Value::as_str);
            if string("type") != Some(MODEL_TYPE)
                || !string("modelIri").is_some_and(|iri| iri.starts_with(&prefix))
            {
                continue;
            }
            let (Some(system), Some(model_key), Some(version), Some(model)) = (
                string("syncSystem"),
                string("syncModelKey"),
                string("version"),
                string("modelIri"),
            ) else {
                continue;
            };
            let key = format!("{system}:{model_key}:{version}");
            let scope = format!("{model}:v{version}");
            let ids: Vec<String> = string("elementIds")
                .and_then(|ids| serde_json::from_str(ids).ok())
                .unwrap_or_default();
            // The model's own writes, read back from the visible nodes and edges.
            let mut writes: Vec<ExchangeMutation> = ids
                .iter()
                .filter_map(|id| {
                    let element = nodes.get(id)?;
                    let strings: BTreeMap<String, String> = element
                        .iter()
                        .filter_map(|(name, value)| {
                            value.as_str().map(|v| (name.clone(), v.to_string()))
                        })
                        .collect();
                    Some(ExchangeMutation::UpsertNode {
                        id: id.clone(),
                        label: strings.get("type").cloned().unwrap_or_default(),
                        properties: strings,
                    })
                })
                .collect();
            writes.extend(edges.iter().filter_map(|edge| {
                let relationship = edge.properties.get("relationship")?.as_str()?;
                (ids.contains(&edge.source) && ids.contains(&edge.target)).then(|| {
                    ExchangeMutation::Link {
                        source: edge.source.clone(),
                        target: edge.target.clone(),
                        predicate: relationship.to_string(),
                    }
                })
            }));
            let mut record = json!({"system": system, "model_key": model_key, "version": version});
            let body = match system {
                DMN => dmn_document(&writes, &scope)
                    .and_then(|document| write_dmn(&document))
                    .map(|bytes| ("document", json!(String::from_utf8_lossy(&bytes)))),
                _ => bpmn_document(&writes, &scope)
                    .and_then(|document| write_bpmn(&document))
                    .map(|bytes| ("document", json!(String::from_utf8_lossy(&bytes)))),
            };
            let Ok((name, value)) = body else {
                continue;
            };
            if system == EPC {
                // The graph holds an EPC model as its BPMN form; the record carries the
                // canonical workflow that form compiles to.
                let Some(workflow) = value.as_str().and_then(|doc| compile_bpmn(doc).ok()) else {
                    continue;
                };
                record["canonical"] = serde_json::to_value(workflow).unwrap_or(Value::Null);
            } else {
                record[name] = value;
            }
            records.insert(key, record);
        }
        records
    }
}
