//! Inbound open metadata mapping (MEDULLA-TYPED-PACKS-R110.3).
//!
//! One pure function turns an [`OpenMetadataRecord`] into the catalog writes
//! that apply it: a node of the mapped class keyed by the GUID, one node per
//! mapped classification and one edge per relationship. The type mapping comes
//! from the `cat:openMetadataType` annotations of the catalog ontology
//! (R110.1); a type with no annotated class is unmapped. The function writes
//! nothing: the sync adapter commits the returned mutations in its batch.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{OpenMetadataError, OpenMetadataRecord};

/// Property carrying the GUID of the open metadata instance (`cat:externalGuid`).
pub const EXTERNAL_GUID_PROPERTY: &str =
    "https://knuckles-team.github.io/ontology/catalog#externalGuid";
/// Property carrying the last synced store version (`cat:externalVersion`).
pub const EXTERNAL_VERSION_PROPERTY: &str =
    "https://knuckles-team.github.io/ontology/catalog#externalVersion";
/// Edge property naming the first end (`cat:from`).
const FROM_PROPERTY: &str = "https://knuckles-team.github.io/ontology/catalog#from";
/// Edge property naming the second end (`cat:to`).
const TO_PROPERTY: &str = "https://knuckles-team.github.io/ontology/catalog#to";

/// The open metadata type mapping: type name to catalog class IRI, read from
/// the ontology's `cat:openMetadataType` annotations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenMetadataTypeView {
    classes: BTreeMap<String, String>,
}

impl OpenMetadataTypeView {
    /// Builds the view from `(type name, class IRI)` annotation pairs. A type
    /// name carried by two classes is ambiguous and is left unmapped (the
    /// ontology test pins exactly one class per type name).
    pub fn from_annotations<I, A, B>(annotations: I) -> Self
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<String>,
        B: Into<String>,
    {
        let mut classes: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (type_name, class_iri) in annotations {
            let slot = classes.entry(type_name.into()).or_insert(None);
            *slot = match slot {
                None => Some(class_iri.into()),
                Some(_) => None,
            };
        }
        Self {
            classes: classes
                .into_iter()
                .filter_map(|(name, class)| class.map(|c| (name, c)))
                .collect(),
        }
    }

    /// The class IRI an open metadata type name maps to, if any.
    pub fn class_for(&self, type_name: &str) -> Option<&str> {
        self.classes.get(type_name).map(String::as_str)
    }

    /// The open metadata type name a catalog class IRI maps to, if any. A class
    /// annotated with two type names is ambiguous and is left unmapped.
    pub fn type_for(&self, class_iri: &str) -> Option<&str> {
        let mut found = self
            .classes
            .iter()
            .filter(|(_, class)| class.as_str() == class_iri)
            .map(|(name, _)| name.as_str());
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    fn require(&self, type_name: &str) -> Result<&str, OpenMetadataError> {
        self.class_for(type_name)
            .ok_or_else(|| OpenMetadataError::TypeUnmapped {
                type_name: type_name.to_string(),
            })
    }
}

/// One catalog write produced by the inbound mapping.
#[derive(Clone, Debug, PartialEq)]
pub enum OpenMetadataMutation {
    /// Write or update one catalog node.
    UpsertNode {
        /// Node id (the GUID, or `<guid>#<classification type>` for a classification).
        id: String,
        /// Class IRI of the node.
        class_iri: String,
        /// Properties to write on the node, keyed by property IRI or source name.
        properties: BTreeMap<String, Value>,
    },
    /// Write or update one edge of the mapped edge class.
    UpsertEdge {
        /// Edge id (the relationship GUID).
        id: String,
        /// Edge class IRI.
        class_iri: String,
        /// GUID of the first end.
        from: String,
        /// GUID of the second end.
        to: String,
        /// Properties to write on the edge.
        properties: BTreeMap<String, Value>,
    },
}

/// Maps one open metadata record to catalog writes. The record key is the
/// GUID. A mapped type writes one node with `cat:externalGuid` and
/// `cat:externalVersion`; a mapped classification writes one node tied to it;
/// a relationship writes one edge of its mapped class. A type with no mapped
/// class returns [`OpenMetadataError::TypeUnmapped`] and nothing is written.
pub fn inbound_mutations(
    view: &OpenMetadataTypeView,
    record: &OpenMetadataRecord,
) -> Result<Vec<OpenMetadataMutation>, OpenMetadataError> {
    let class_iri = view.require(&record.type_name)?.to_string();
    let mut out = Vec::new();

    let mut properties = record.properties.clone();
    stamp(&mut properties, &record.guid, record.version);
    out.push(OpenMetadataMutation::UpsertNode {
        id: record.guid.clone(),
        class_iri,
        properties,
    });

    for classification in &record.classifications {
        let class_iri = view.require(&classification.type_name)?.to_string();
        let mut properties = classification.properties.clone();
        stamp(&mut properties, &record.guid, record.version);
        out.push(OpenMetadataMutation::UpsertNode {
            id: format!("{}#{}", record.guid, classification.type_name),
            class_iri,
            properties,
        });
    }

    for relationship in &record.relationships {
        let class_iri = view.require(&relationship.type_name)?.to_string();
        let mut properties = BTreeMap::new();
        stamp(&mut properties, &relationship.guid, relationship.version);
        properties.insert(
            FROM_PROPERTY.to_string(),
            Value::from(relationship.end1_guid.clone()),
        );
        properties.insert(
            TO_PROPERTY.to_string(),
            Value::from(relationship.end2_guid.clone()),
        );
        out.push(OpenMetadataMutation::UpsertEdge {
            id: relationship.guid.clone(),
            class_iri,
            from: relationship.end1_guid.clone(),
            to: relationship.end2_guid.clone(),
            properties,
        });
    }
    Ok(out)
}

fn stamp(properties: &mut BTreeMap<String, Value>, guid: &str, version: u64) {
    properties.insert(EXTERNAL_GUID_PROPERTY.to_string(), Value::from(guid));
    properties.insert(EXTERNAL_VERSION_PROPERTY.to_string(), Value::from(version));
}
