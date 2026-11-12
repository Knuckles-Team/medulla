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

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> OpenMetadataRecord {
        OpenMetadataRecord {
            schema_version: OPEN_METADATA_SCHEMA_VERSION,
            guid: "g-1".into(),
            type_name: "GlossaryTerm".into(),
            version: 7,
            update_time_ms: 1_700_000_000_000,
            properties: BTreeMap::from([("displayName".into(), serde_json::json!("Customer"))]),
            classifications: vec![OpenMetadataClassification {
                schema_version: OPEN_METADATA_SCHEMA_VERSION,
                type_name: "Confidentiality".into(),
                properties: BTreeMap::from([("level".into(), serde_json::json!(2))]),
            }],
            relationships: vec![OpenMetadataRelationship {
                schema_version: OPEN_METADATA_SCHEMA_VERSION,
                guid: "r-1".into(),
                type_name: "TermAnchor".into(),
                end1_guid: "g-1".into(),
                end2_guid: "g-2".into(),
                version: 3,
            }],
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R110.2
    #[test]
    fn record_round_trips_and_refuses_unknown_fields() {
        let original = record();
        let wire = serde_json::to_value(&original).unwrap();
        let back: OpenMetadataRecord = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(back, original);

        let mut extra = wire.clone();
        extra["surprise"] = serde_json::json!(true);
        assert!(serde_json::from_value::<OpenMetadataRecord>(extra).is_err());

        let mut nested = wire.clone();
        nested["relationships"][0]["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<OpenMetadataRecord>(nested).is_err());

        let mut classification = wire;
        classification["classifications"][0]["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<OpenMetadataRecord>(classification).is_err());
    }
}
