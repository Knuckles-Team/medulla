//! Outbound open metadata payload (MEDULLA-TYPED-PACKS-R110.4).
//!
//! One pure function turns a catalog node into the open metadata record the
//! sync adapter proposes to the store. The payload carries the last synced
//! `cat:externalVersion` as the expected version, so the connector can refuse
//! the write when the store version moved. A node that was never synced (no
//! `cat:externalGuid`) becomes a create: an empty GUID and version zero.
//! The function writes nothing.

use std::collections::BTreeMap;

use serde_json::Value;

use super::inbound::{EXTERNAL_GUID_PROPERTY, EXTERNAL_VERSION_PROPERTY};
use super::{
    OpenMetadataError, OpenMetadataRecord, OpenMetadataTypeView, OPEN_METADATA_SCHEMA_VERSION,
};

/// The catalog node an outbound proposal is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboundNode {
    /// Class IRI of the catalog node.
    pub class_iri: String,
    /// Node properties keyed by property IRI or source name.
    pub properties: BTreeMap<String, Value>,
    /// Last update time of the node in milliseconds since the epoch.
    pub update_time_ms: i64,
}

/// Builds the outbound record for one catalog node. The record's `version`
/// is the last synced `cat:externalVersion` (the expected store version; zero
/// for a create) and its `guid` is `cat:externalGuid` (empty for a create).
/// The sync stamps are not copied into the record's properties. A class with
/// no `cat:openMetadataType` annotation returns
/// [`OpenMetadataError::ClassUnmapped`] and nothing is proposed.
pub fn outbound_record(
    view: &OpenMetadataTypeView,
    node: &OutboundNode,
) -> Result<OpenMetadataRecord, OpenMetadataError> {
    let type_name = view
        .type_for(&node.class_iri)
        .ok_or_else(|| OpenMetadataError::ClassUnmapped {
            class_iri: node.class_iri.clone(),
        })?
        .to_string();
    let guid = node
        .properties
        .get(EXTERNAL_GUID_PROPERTY)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let version = node
        .properties
        .get(EXTERNAL_VERSION_PROPERTY)
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let properties = node
        .properties
        .iter()
        .filter(|(key, _)| {
            key.as_str() != EXTERNAL_GUID_PROPERTY && key.as_str() != EXTERNAL_VERSION_PROPERTY
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Ok(OpenMetadataRecord {
        schema_version: OPEN_METADATA_SCHEMA_VERSION,
        guid,
        type_name,
        version,
        update_time_ms: node.update_time_ms,
        properties,
        classifications: Vec::new(),
        relationships: Vec::new(),
    })
}
