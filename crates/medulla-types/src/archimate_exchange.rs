//! Architecture exchange records (MEDULLA-CQ-ORCH-R705.2).
//!
//! The Open Group ArchiMate Model Exchange Format as typed records. The
//! reader (`medulla_core::architecture_exchange::read`) returns these types and
//! the writer consumes them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current record version of the exchange types.
pub const ARCHIMATE_EXCHANGE_SCHEMA_VERSION: u16 = 1;

fn schema_version() -> u16 {
    ARCHIMATE_EXCHANGE_SCHEMA_VERSION
}

/// One element of the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeElement {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub identifier: String,
    /// The ArchiMate element type, e.g. `BusinessProcess`.
    pub element_type: String,
    pub name: String,
    /// Property definition reference to value.
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

/// One relationship of the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeRelationship {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub identifier: String,
    /// The ArchiMate relationship type, e.g. `Serving`.
    pub relationship_type: String,
    pub source: String,
    pub target: String,
}

/// One organization folder and the items it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeOrganization {
    pub label: String,
    #[serde(default)]
    pub item_refs: Vec<String>,
    #[serde(default)]
    pub children: Vec<ExchangeOrganization>,
}

/// One declared property definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangePropertyDefinition {
    pub identifier: String,
    pub name: String,
    pub data_type: String,
}

/// One node of a view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeViewNode {
    pub identifier: String,
    #[serde(default)]
    pub element_ref: Option<String>,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// One connection of a view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeConnection {
    pub identifier: String,
    #[serde(default)]
    pub relationship_ref: Option<String>,
    pub source: String,
    pub target: String,
}

/// One stored view (diagram) with its nodes and connections.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeView {
    pub identifier: String,
    pub name: String,
    #[serde(default)]
    pub viewpoint: Option<String>,
    #[serde(default)]
    pub nodes: Vec<ExchangeViewNode>,
    #[serde(default)]
    pub connections: Vec<ExchangeConnection>,
}

/// One whole exchange document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchimateExchange {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub model_id: String,
    pub name: String,
    #[serde(default)]
    pub elements: Vec<ExchangeElement>,
    #[serde(default)]
    pub relationships: Vec<ExchangeRelationship>,
    #[serde(default)]
    pub organizations: Vec<ExchangeOrganization>,
    #[serde(default)]
    pub views: Vec<ExchangeView>,
    #[serde(default)]
    pub property_definitions: Vec<ExchangePropertyDefinition>,
    /// Root children the model does not type, kept as XML.
    #[serde(default)]
    pub preserved: Vec<String>,
}

/// SHA-256 of `bytes` as 64 lowercase hex characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl ArchimateExchange {
    /// SHA-256 digest (64 lowercase hex characters) of the canonical model: identifier order, no
    /// organizations or preserved fragments.
    pub fn canonical_digest(&self) -> String {
        let mut canon = self.clone();
        canon.organizations.clear();
        canon.preserved.clear();
        canon
            .elements
            .sort_by(|a, b| a.identifier.cmp(&b.identifier));
        canon
            .relationships
            .sort_by(|a, b| a.identifier.cmp(&b.identifier));
        canon.views.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        canon
            .property_definitions
            .sort_by(|a, b| a.identifier.cmp(&b.identifier));
        sha256_hex(&serde_json::to_vec(&canon).unwrap_or_default())
    }
}

/// Why an exchange document was not read or written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArchimateExchangeError {
    Malformed { line: u64, column: u64 },
    EntityRefused,
    TooLarge { bytes: u64 },
    NamespaceUnknown { namespace: String },
    RelationshipInvalid { identifier: String },
}

impl ArchimateExchangeError {
    /// The stable wire code of this refusal.
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::Malformed { .. } => "ARCHIMATE_DOC_MALFORMED",
            Self::EntityRefused => "ARCHIMATE_DOC_ENTITY_REFUSED",
            Self::TooLarge { .. } => "ARCHIMATE_DOC_TOO_LARGE",
            Self::NamespaceUnknown { .. } => "ARCHIMATE_NAMESPACE_UNKNOWN",
            Self::RelationshipInvalid { .. } => "ARCHIMATE_RELATIONSHIP_INVALID",
        }
    }
}

impl std::fmt::Display for ArchimateExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.wire_code();
        match self {
            Self::Malformed { line, column } => write!(f, "{code}: malformed at {line}:{column}"),
            Self::EntityRefused => write!(f, "{code}: document declares an entity"),
            Self::TooLarge { bytes } => write!(f, "{code}: document exceeds {bytes} bytes"),
            Self::NamespaceUnknown { namespace } => {
                write!(f, "{code}: namespace {namespace} is unknown")
            }
            Self::RelationshipInvalid { identifier } => {
                write!(f, "{code}: relationship {identifier} is not allowed")
            }
        }
    }
}

impl std::error::Error for ArchimateExchangeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // spec: MEDULLA-CQ-ORCH-R705.2
    #[test]
    fn exchange_records_round_trip() {
        let doc = ArchimateExchange {
            schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
            model_id: "id-model".into(),
            name: "Estate".into(),
            elements: vec![ExchangeElement {
                schema_version: 1,
                identifier: "id-1".into(),
                element_type: "BusinessProcess".into(),
                name: "Order".into(),
                properties: BTreeMap::from([("pd-1".into(), "gold".into())]),
            }],
            relationships: vec![ExchangeRelationship {
                schema_version: 1,
                identifier: "id-r".into(),
                relationship_type: "Serving".into(),
                source: "id-1".into(),
                target: "id-1".into(),
            }],
            organizations: vec![ExchangeOrganization {
                label: "Business".into(),
                item_refs: vec!["id-1".into()],
                children: vec![],
            }],
            views: vec![ExchangeView {
                identifier: "id-v".into(),
                name: "Main".into(),
                viewpoint: None,
                nodes: vec![ExchangeViewNode {
                    identifier: "id-n".into(),
                    element_ref: Some("id-1".into()),
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4,
                }],
                connections: vec![],
            }],
            property_definitions: vec![ExchangePropertyDefinition {
                identifier: "pd-1".into(),
                name: "tier".into(),
                data_type: "string".into(),
            }],
            preserved: vec![],
        };
        let wire = serde_json::to_value(&doc).unwrap();
        assert_eq!(
            serde_json::from_value::<ArchimateExchange>(wire.clone()).unwrap(),
            doc
        );
        let mut unknown = wire;
        unknown["surprise"] = json!(1);
        assert!(serde_json::from_value::<ArchimateExchange>(unknown).is_err());
        let error = ArchimateExchangeError::RelationshipInvalid {
            identifier: "id-r".into(),
        };
        let wire = serde_json::to_value(&error).unwrap();
        assert_eq!(wire["code"], "relationship_invalid");
        assert_eq!(
            serde_json::from_value::<ArchimateExchangeError>(wire).unwrap(),
            error
        );
        assert_eq!(doc.canonical_digest(), doc.clone().canonical_digest());
    }
}
