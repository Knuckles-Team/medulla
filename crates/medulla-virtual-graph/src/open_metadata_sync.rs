//! The open metadata sync adapter (MEDULLA-TYPED-PACKS-R110.5): the
//! [`SyncAdapter`] of the catalog classes annotated with an open metadata type.
//!
//! A source record is one open metadata instance:
//! `{guid | id, type_name, version, update_time_ms, properties,
//! classifications: [{type_name, properties}], relationships: [{guid,
//! type_name, end1_guid, end2_guid, version}]}`. The key is the GUID. The digest
//! covers the type, the properties, the mapped classifications and the mapped
//! relationships; the store version is not part of it, so a version bump alone
//! is no change. A classification or relationship whose type no catalog class
//! carries is left out of the digest and of the writes (MEDULLA-TYPED-PACKS-R110.6
//! reports it). Inbound writes come from `inbound_mutations` (R110.3); a
//! relationship becomes a node of its edge class holding its two ends, because
//! the sync batch writes nodes. The outbound payload is built with
//! `outbound_record` (R110.4).

use std::collections::BTreeMap;

use medulla_catalog_types::open_metadata::{
    inbound_mutations, outbound_record, OpenMetadataClassification, OpenMetadataMutation,
    OpenMetadataRecord, OpenMetadataRelationship, OpenMetadataTypeView, OutboundNode,
    EXTERNAL_GUID_PROPERTY, EXTERNAL_VERSION_PROPERTY, FROM_PROPERTY, OPEN_METADATA_SCHEMA_VERSION,
    TO_PROPERTY,
};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

use crate::source_sync::{GraphEdges, GraphNodes, RecordScreen, SyncAdapter, SyncMutation};

/// Syncs the records of one catalog class annotated with an open metadata type.
#[derive(Clone, Debug)]
pub struct OpenMetadataSyncAdapter {
    /// The type mapping read from the ontology annotations.
    pub view: OpenMetadataTypeView,
    /// The catalog class the sync serves.
    pub class_iri: String,
}

fn label_of(class_iri: &str) -> &str {
    class_iri
        .rsplit_once(['#', '/'])
        .map_or(class_iri, |(_, label)| label)
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn property_map(value: &Value) -> BTreeMap<String, Value> {
    value
        .get("properties")
        .and_then(Value::as_object)
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn items<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

impl OpenMetadataSyncAdapter {
    /// The record a source row describes, with the parts whose type is
    /// unmapped left out. `None` when the row has no GUID or is not of this
    /// adapter's class.
    fn record(&self, row: &Value) -> Option<OpenMetadataRecord> {
        let guid = [text(row, "guid"), text(row, "id")]
            .into_iter()
            .find(|candidate| !candidate.is_empty())?;
        let type_name = text(row, "type_name");
        if self.view.type_for(&self.class_iri) != Some(type_name) {
            return None;
        }
        let classifications = items(row, "classifications")
            .filter(|c| self.view.class_for(text(c, "type_name")).is_some())
            .map(|c| OpenMetadataClassification {
                schema_version: OPEN_METADATA_SCHEMA_VERSION,
                type_name: text(c, "type_name").to_owned(),
                properties: property_map(c),
            })
            .collect();
        let relationships = items(row, "relationships")
            .filter(|r| self.view.class_for(text(r, "type_name")).is_some())
            .map(|r| OpenMetadataRelationship {
                schema_version: OPEN_METADATA_SCHEMA_VERSION,
                guid: text(r, "guid").to_owned(),
                type_name: text(r, "type_name").to_owned(),
                end1_guid: text(r, "end1_guid").to_owned(),
                end2_guid: text(r, "end2_guid").to_owned(),
                version: r.get("version").and_then(Value::as_u64).unwrap_or_default(),
            })
            .collect();
        Some(OpenMetadataRecord {
            schema_version: OPEN_METADATA_SCHEMA_VERSION,
            guid: guid.to_owned(),
            type_name: type_name.to_owned(),
            version: row
                .get("version")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            update_time_ms: row
                .get("update_time_ms")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            properties: property_map(row),
            classifications,
            relationships,
        })
    }

    fn node_properties(
        properties: &BTreeMap<String, Value>,
        class_iri: &str,
    ) -> Map<String, Value> {
        let mut map: Map<String, Value> = properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        map.insert("type".into(), json!(label_of(class_iri)));
        map
    }
}

impl SyncAdapter for OpenMetadataSyncAdapter {
    fn record_key(&self, record: &Value) -> Option<String> {
        [text(record, "guid"), text(record, "id")]
            .into_iter()
            .find(|candidate| !candidate.is_empty())
            .map(str::to_owned)
    }

    fn record_digest(&self, record: &Value) -> String {
        let canonical = match self.record(record) {
            Some(mapped) => {
                let mut classifications: Vec<_> = mapped
                    .classifications
                    .iter()
                    .map(|c| (c.type_name.clone(), c.properties.clone()))
                    .collect();
                classifications.sort_by(|a, b| a.0.cmp(&b.0));
                let mut relationships: Vec<_> = mapped
                    .relationships
                    .iter()
                    .map(|r| {
                        (
                            r.guid.clone(),
                            r.type_name.clone(),
                            r.end1_guid.clone(),
                            r.end2_guid.clone(),
                        )
                    })
                    .collect();
                relationships.sort();
                json!({
                    "type": mapped.type_name,
                    "properties": mapped.properties,
                    "classifications": classifications,
                    "relationships": relationships,
                })
            }
            None => Value::Null,
        };
        Digest256::sha256(canonical.to_string().as_bytes()).to_hex()
    }

    fn inbound_mutations(&self, _key: &str, record: &Value) -> Vec<SyncMutation> {
        let Some(mapped) = self.record(record) else {
            return Vec::new();
        };
        let Ok(writes) = inbound_mutations(&self.view, &mapped) else {
            // `record` kept only mapped types, so this is unreachable for a
            // record the adapter accepted; nothing is written for one it did not.
            return Vec::new();
        };
        writes
            .into_iter()
            .map(|write| match write {
                OpenMetadataMutation::UpsertNode {
                    id,
                    class_iri,
                    properties,
                } => SyncMutation {
                    node_id: id,
                    properties: Self::node_properties(&properties, &class_iri),
                },
                OpenMetadataMutation::UpsertEdge {
                    id,
                    class_iri,
                    properties,
                    ..
                } => SyncMutation {
                    node_id: id,
                    properties: Self::node_properties(&properties, &class_iri),
                },
            })
            .collect()
    }

    fn outbound_payload(&self, _key: &str, record: &Value) -> Value {
        let Some(mapped) = self.record(record) else {
            return Value::Null;
        };
        let mut properties = mapped.properties.clone();
        properties.insert(EXTERNAL_GUID_PROPERTY.to_owned(), json!(mapped.guid));
        properties.insert(EXTERNAL_VERSION_PROPERTY.to_owned(), json!(mapped.version));
        let node = OutboundNode {
            class_iri: self.class_iri.clone(),
            properties,
            update_time_ms: mapped.update_time_ms,
        };
        let Ok(mut proposed) = outbound_record(&self.view, &node) else {
            return Value::Null;
        };
        proposed.classifications = mapped.classifications;
        proposed.relationships = mapped.relationships;
        serde_json::to_value(proposed).unwrap_or(Value::Null)
    }

    fn screen(&self, record: &Value) -> RecordScreen {
        let type_name = text(record, "type_name");
        if self.view.type_for(&self.class_iri) != Some(type_name) {
            return RecordScreen {
                accepted: false,
                type_name: None,
                unmapped_types: Some(type_name)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
            };
        }
        let unmapped: std::collections::BTreeSet<String> = items(record, "classifications")
            .chain(items(record, "relationships"))
            .map(|part| text(part, "type_name"))
            .filter(|name| !name.is_empty() && self.view.class_for(name).is_none())
            .map(str::to_owned)
            .collect();
        RecordScreen {
            accepted: true,
            type_name: Some(type_name.to_owned()),
            unmapped_types: unmapped.into_iter().collect(),
        }
    }

    fn version_ahead(&self, graph: &Value, source: &Value) -> bool {
        let version = |record: &Value| record.get("version").and_then(Value::as_u64).unwrap_or(0);
        version(source) > version(graph)
    }

    fn graph_records(&self, nodes: &GraphNodes, _edges: &GraphEdges) -> BTreeMap<String, Value> {
        let class_label = label_of(&self.class_iri);
        let base = self
            .class_iri
            .strip_suffix(class_label)
            .unwrap_or_default()
            .to_owned();
        let user_properties = |props: &Map<String, Value>| -> Map<String, Value> {
            props
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "type"
                            | EXTERNAL_GUID_PROPERTY
                            | EXTERNAL_VERSION_PROPERTY
                            | FROM_PROPERTY
                            | TO_PROPERTY
                    )
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let mut records = BTreeMap::new();
        for props in nodes.values() {
            if props.get("type").and_then(Value::as_str) != Some(class_label) {
                continue;
            }
            let Some(guid) = props.get(EXTERNAL_GUID_PROPERTY).and_then(Value::as_str) else {
                continue;
            };
            let classification_prefix = format!("{guid}#");
            let classifications: Vec<Value> = nodes
                .iter()
                .filter(|(node_id, _)| node_id.starts_with(&classification_prefix))
                .filter_map(|(_, node)| {
                    let label = node.get("type").and_then(Value::as_str)?;
                    let type_name = self.view.type_for(&format!("{base}{label}"))?;
                    Some(json!({
                        "type_name": type_name,
                        "properties": user_properties(node),
                    }))
                })
                .collect();
            let relationships: Vec<Value> = nodes
                .iter()
                .filter(|(_, node)| node.get(FROM_PROPERTY).and_then(Value::as_str) == Some(guid))
                .filter_map(|(node_id, node)| {
                    let label = node.get("type").and_then(Value::as_str)?;
                    let type_name = self.view.type_for(&format!("{base}{label}"))?;
                    Some(json!({
                        "guid": node_id,
                        "type_name": type_name,
                        "end1_guid": guid,
                        "end2_guid": node.get(TO_PROPERTY).cloned().unwrap_or(Value::Null),
                        "version": node.get(EXTERNAL_VERSION_PROPERTY).cloned().unwrap_or(json!(0)),
                    }))
                })
                .collect();
            records.insert(
                guid.to_owned(),
                json!({
                    "guid": guid,
                    "type_name": self.view.type_for(&self.class_iri),
                    "version": props.get(EXTERNAL_VERSION_PROPERTY).cloned().unwrap_or(json!(0)),
                    "update_time_ms": 0,
                    "properties": user_properties(props),
                    "classifications": classifications,
                    "relationships": relationships,
                }),
            );
        }
        records
    }
}
