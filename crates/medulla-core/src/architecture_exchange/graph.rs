//! Exchange document to graph mapping (MEDULLA-CQ-ORCH-R705.4).

use std::collections::BTreeMap;

use medulla_types::archimate_exchange::ArchimateExchange;
use serde_json::Value;

use super::ExchangeView;

/// One graph write the mapping proposes. The served import handler turns the
/// list into one `MutationBatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeMutation {
    /// Create or update one node with string properties.
    UpsertNode {
        id: String,
        label: String,
        properties: BTreeMap<String, String>,
    },
    /// Create one direct edge.
    Link {
        source: String,
        target: String,
        predicate: String,
    },
}

pub(crate) type Props = serde_json::Map<String, Value>;

pub(crate) fn decode(properties: &[u8]) -> Props {
    match rmp_serde::from_slice::<Value>(properties) {
        Ok(Value::Object(map)) => map,
        _ => Props::new(),
    }
}

pub(crate) fn text<'a>(props: &'a Props, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}

/// The IRI-like id a node of the imported model receives.
pub fn node_id(model_id: &str, identifier: &str) -> String {
    format!("archimate:{model_id}:{identifier}")
}

/// Map a document onto the visible graph. An element whose
/// `archimateIdentifier` matches a visible node updates that node; any other
/// element creates a node of the class annotated with its type. A
/// relationship becomes a reified `ArchimateRelationship` node and a direct
/// edge; a view becomes an `ArchimateView` node. The mapping never asserts
/// `owl:sameAs`: a name match to a native node writes an
/// `EntityResolutionProposal` for operator review. Output order is
/// identifier order, so equal input gives equal output.
pub fn exchange_mutations(
    view: &ExchangeView<'_>,
    doc: &ArchimateExchange,
) -> Vec<ExchangeMutation> {
    let mut known: BTreeMap<String, String> = BTreeMap::new();
    let mut native_by_name: BTreeMap<String, String> = BTreeMap::new();
    for (id, bytes) in view.core.get_nodes() {
        let props = decode(&bytes);
        if let Some(identifier) = text(&props, "archimateIdentifier") {
            known.insert(identifier.to_owned(), id);
        } else if let Some(name) = text(&props, "name") {
            native_by_name.entry(name.to_lowercase()).or_insert(id);
        }
    }
    let digest = doc.canonical_digest();
    let definitions: BTreeMap<&str, &str> = doc
        .property_definitions
        .iter()
        .map(|d| (d.identifier.as_str(), d.name.as_str()))
        .collect();
    let id_of = |identifier: &str| {
        known
            .get(identifier)
            .cloned()
            .unwrap_or_else(|| node_id(&doc.model_id, identifier))
    };

    let mut out = Vec::new();
    let mut elements: Vec<_> = doc.elements.iter().collect();
    elements.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    for element in elements {
        let id = id_of(&element.identifier);
        let label = view
            .types
            .class_of(&element.element_type)
            .unwrap_or("ArchimateElement")
            .to_owned();
        let mut properties = BTreeMap::from([
            ("type".to_owned(), label.clone()),
            ("archimateIdentifier".to_owned(), element.identifier.clone()),
            (
                "archimateElementType".to_owned(),
                element.element_type.clone(),
            ),
            ("name".to_owned(), element.name.clone()),
            ("importedFrom".to_owned(), digest.clone()),
        ]);
        for (definition, value) in &element.properties {
            let key = definitions
                .get(definition.as_str())
                .copied()
                .unwrap_or(definition);
            properties.insert(format!("prop:{key}"), value.clone());
        }
        out.push(ExchangeMutation::UpsertNode {
            id: id.clone(),
            label,
            properties,
        });
        if !known.contains_key(&element.identifier) {
            if let Some(native) = native_by_name.get(&element.name.to_lowercase()) {
                let proposal = format!("proposal:{}:{}", doc.model_id, element.identifier);
                out.push(ExchangeMutation::UpsertNode {
                    id: proposal.clone(),
                    label: "EntityResolutionProposal".to_owned(),
                    properties: BTreeMap::from([
                        ("type".to_owned(), "EntityResolutionProposal".to_owned()),
                        ("decision_status".to_owned(), "proposed".to_owned()),
                        ("candidate".to_owned(), id.clone()),
                        ("native".to_owned(), native.clone()),
                    ]),
                });
                out.push(ExchangeMutation::Link {
                    source: proposal,
                    target: native.clone(),
                    predicate: "PROPOSES_RESOLUTION".to_owned(),
                });
            }
        }
    }

    let mut relationships: Vec<_> = doc.relationships.iter().collect();
    relationships.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    for r in relationships {
        let reified = id_of(&r.identifier);
        let (source, target) = (id_of(&r.source), id_of(&r.target));
        out.push(ExchangeMutation::UpsertNode {
            id: reified,
            label: "ArchimateRelationship".to_owned(),
            properties: BTreeMap::from([
                ("type".to_owned(), "ArchimateRelationship".to_owned()),
                ("archimateIdentifier".to_owned(), r.identifier.clone()),
                ("relationshipType".to_owned(), r.relationship_type.clone()),
                ("relSource".to_owned(), source.clone()),
                ("relTarget".to_owned(), target.clone()),
                ("importedFrom".to_owned(), digest.clone()),
            ]),
        });
        let predicate = view
            .types
            .edges
            .get(&r.relationship_type)
            .cloned()
            .unwrap_or_else(|| r.relationship_type.clone());
        out.push(ExchangeMutation::Link {
            source,
            target,
            predicate,
        });
    }

    let mut views: Vec<_> = doc.views.iter().collect();
    views.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    for v in views {
        out.push(ExchangeMutation::UpsertNode {
            id: id_of(&v.identifier),
            label: "ArchimateView".to_owned(),
            properties: BTreeMap::from([
                ("type".to_owned(), "ArchimateView".to_owned()),
                ("archimateIdentifier".to_owned(), v.identifier.clone()),
                ("name".to_owned(), v.name.clone()),
                (
                    "view".to_owned(),
                    serde_json::to_string(v).unwrap_or_default(),
                ),
                ("importedFrom".to_owned(), digest.clone()),
            ]),
        });
    }
    out
}
