//! The ArchiMate element sync adapter (MEDULLA-CQ-ORCH-R705.7): the
//! [`SyncAdapter`] of the classes that resolve an `:archimateType`.
//!
//! A record is one element with its relationships:
//! `{identifier, element_type, name, properties: {ref: value}, relationships:
//! [{identifier, relationship_type, source, target}]}`. The key is the element
//! identifier; the digest covers type, name, properties and relationships, so
//! two records that differ only in layout or folder compare equal. Inbound
//! changes write the same node properties as the exchange mapping
//! (`medulla_core::architecture_exchange::graph::exchange_mutations`).

use std::collections::BTreeMap;

use medulla_core::architecture_exchange::graph::node_id;
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

use crate::source_sync::{GraphNodes, SyncAdapter, SyncMutation};

/// Syncs the elements of one imported ArchiMate model.
#[derive(Clone, Debug)]
pub struct ArchitectureSyncAdapter {
    /// The model identifier the node ids of its elements carry.
    pub model_id: String,
}

fn text<'a>(record: &'a Value, key: &str) -> &'a str {
    record.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn properties(record: &Value) -> BTreeMap<String, String> {
    record
        .get("properties")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// The relationships of a record in identifier order, each as
/// `(identifier, type, source, target)`.
fn relationships(record: &Value) -> Vec<(String, String, String, String)> {
    let mut out: Vec<_> = record
        .get("relationships")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|r| {
                    (
                        text(r, "identifier").to_owned(),
                        text(r, "relationship_type").to_owned(),
                        text(r, "source").to_owned(),
                        text(r, "target").to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

impl SyncAdapter for ArchitectureSyncAdapter {
    fn record_key(&self, record: &Value) -> Option<String> {
        let identifier = text(record, "identifier");
        (!identifier.is_empty()).then(|| identifier.to_owned())
    }

    fn record_digest(&self, record: &Value) -> String {
        let canonical = json!({
            "type": text(record, "element_type"),
            "name": text(record, "name"),
            "properties": properties(record),
            "relationships": relationships(record),
        });
        Digest256::sha256(canonical.to_string().as_bytes()).to_hex()
    }

    fn inbound_mutations(&self, key: &str, record: &Value) -> Vec<SyncMutation> {
        let mut props = Map::new();
        props.insert("archimateIdentifier".into(), json!(key));
        props.insert(
            "archimateElementType".into(),
            json!(text(record, "element_type")),
        );
        props.insert("name".into(), json!(text(record, "name")));
        for (reference, value) in properties(record) {
            props.insert(format!("prop:{reference}"), json!(value));
        }
        vec![SyncMutation {
            node_id: node_id(&self.model_id, key),
            properties: props,
        }]
    }

    fn outbound_payload(&self, key: &str, record: &Value) -> Value {
        json!({
            "identifier": key,
            "element_type": text(record, "element_type"),
            "name": text(record, "name"),
            "properties": properties(record),
            "relationships": relationships(record)
                .into_iter()
                .map(|(identifier, relationship_type, source, target)| json!({
                    "identifier": identifier,
                    "relationship_type": relationship_type,
                    "source": source,
                    "target": target,
                }))
                .collect::<Vec<_>>(),
        })
    }

    fn graph_records(&self, nodes: &GraphNodes) -> BTreeMap<String, Value> {
        let prefix = node_id(&self.model_id, "");
        let identifier_of = |id: &str| {
            nodes
                .get(id)
                .and_then(|props| props.get("archimateIdentifier"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let mut records = BTreeMap::new();
        for (id, props) in nodes {
            let Some(key) = id.strip_prefix(&prefix) else {
                continue;
            };
            let Some(element_type) = props.get("archimateElementType").and_then(Value::as_str)
            else {
                continue;
            };
            let properties: Map<String, Value> = props
                .iter()
                .filter_map(|(k, v)| Some((k.strip_prefix("prop:")?.to_owned(), v.clone())))
                .collect();
            let relationships: Vec<Value> = nodes
                .values()
                .filter(|rel| {
                    rel.get("type").and_then(Value::as_str) == Some("ArchimateRelationship")
                        && rel.get("relSource").and_then(Value::as_str) == Some(id.as_str())
                })
                .map(|rel| {
                    json!({
                        "identifier": rel.get("archimateIdentifier").cloned().unwrap_or(Value::Null),
                        "relationship_type": rel.get("relationshipType").cloned().unwrap_or(Value::Null),
                        "source": key,
                        "target": rel
                            .get("relTarget")
                            .and_then(Value::as_str)
                            .and_then(identifier_of)
                            .map_or(Value::Null, Value::String),
                    })
                })
                .collect();
            records.insert(
                key.to_owned(),
                json!({
                    "identifier": key,
                    "element_type": element_type,
                    "name": props.get("name").cloned().unwrap_or(Value::Null),
                    "properties": properties,
                    "relationships": relationships,
                }),
            );
        }
        records
    }
}
