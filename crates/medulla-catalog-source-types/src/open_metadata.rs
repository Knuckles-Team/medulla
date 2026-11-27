//! Open metadata record model (MEDULLA-TYPED-PACKS-R110.2).
//!
//! `OpenMetadataRecord` mirrors the instance header of an open metadata store:
//! GUID, type name and version, with its properties, classifications and
//! relationships. The open-metadata sync adapter reads and writes these
//! records; the catalog stays the system of record. Pure serde.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Wire schema version of every open metadata record type.
pub const OPEN_METADATA_SCHEMA_VERSION: u16 = 1;

fn schema_version() -> u16 {
    OPEN_METADATA_SCHEMA_VERSION
}

/// One classification attached to an open metadata record.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMetadataClassification {
    /// Wire schema version.
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    /// Classification type name, for example `Confidentiality`.
    pub type_name: String,
    /// Classification properties.
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// One relationship between two open metadata records.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMetadataRelationship {
    /// Wire schema version.
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    /// Relationship GUID.
    pub guid: String,
    /// Relationship type name.
    pub type_name: String,
    /// GUID of the first end.
    pub end1_guid: String,
    /// GUID of the second end.
    pub end2_guid: String,
    /// Store version of the relationship.
    pub version: u64,
}

/// One open metadata instance with its header, properties, classifications
/// and relationships.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMetadataRecord {
    /// Wire schema version.
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    /// Instance GUID.
    pub guid: String,
    /// Open metadata type name, for example `GlossaryTerm`.
    pub type_name: String,
    /// Store version; an outbound write carries the last synced one.
    pub version: u64,
    /// Last update time in milliseconds since the epoch.
    pub update_time_ms: i64,
    /// Instance properties.
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Classifications attached to the instance.
    #[serde(default)]
    pub classifications: Vec<OpenMetadataClassification>,
    /// Relationships the instance takes part in.
    #[serde(default)]
    pub relationships: Vec<OpenMetadataRelationship>,
}

mod inbound;
mod outbound;

pub use inbound::{
    inbound_mutations, OpenMetadataMutation, OpenMetadataTypeView, EXTERNAL_GUID_PROPERTY,
    EXTERNAL_VERSION_PROPERTY, FROM_PROPERTY, TO_PROPERTY,
};
pub use outbound::{outbound_record, OutboundNode};

/// Report code: a source type no catalog class carries (MEDULLA-TYPED-PACKS-R110.6). It is a
/// code of the sync report, not a request refusal: the other records still sync.
pub const METADATA_TYPE_UNMAPPED: &str = "METADATA_TYPE_UNMAPPED";
/// Report code: the source store holds a newer version of a record than the graph's last
/// synced one (MEDULLA-TYPED-PACKS-R110.6).
pub const METADATA_VERSION_CONFLICT: &str = "METADATA_VERSION_CONFLICT";
/// The report codes of the open metadata sync.
pub const REFUSALS: [&str; 2] = [METADATA_TYPE_UNMAPPED, METADATA_VERSION_CONFLICT];

/// Why an open metadata record could not be mapped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpenMetadataError {
    /// The record (or relationship) type has no class annotated with its name
    /// (`cat:openMetadataType`), so it is not mapped.
    #[error("open metadata type {type_name} is unmapped")]
    TypeUnmapped {
        /// The unmapped open metadata type name.
        type_name: String,
    },
    /// The catalog class has no open metadata type annotated on it, so a node of
    /// it is not proposed outbound.
    #[error("catalog class {class_iri} has no open metadata type")]
    ClassUnmapped {
        /// The unmapped catalog class IRI.
        class_iri: String,
    },
}

impl OpenMetadataError {
    /// The report code of this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TypeUnmapped { .. } | Self::ClassUnmapped { .. } => METADATA_TYPE_UNMAPPED,
        }
    }
}

#[cfg(test)]
mod tests;
