//! Open Exchange Format writer (MEDULLA-CQ-ORCH-R705.5).

use std::collections::BTreeMap;

use medulla_types::archimate_exchange::sha256_hex as hex_sha256;
use medulla_types::archimate_exchange::{
    ArchimateExchange, ArchimateExchangeError, ExchangeElement, ExchangeRelationship,
    ExchangeView as StoredView, ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
};

use super::graph::{decode, text};
use super::{ExchangeView, ExportScope, ARCHIMATE_NS};
use crate::process_exchange::xml_tree::escape;

/// The identifier a node without `archimateIdentifier` receives: `id-` and
/// the first 32 hex characters of the SHA-256 of its IRI.
pub fn derived_identifier(iri: &str) -> String {
    format!("id-{}", &hex_sha256(iri.as_bytes())[..32])
}

/// Collect the exchange document of the visible architecture nodes in scope.
pub fn collect_exchange(view: &ExchangeView<'_>, scope: &ExportScope) -> ArchimateExchange {
    let mut identifiers: BTreeMap<String, String> = BTreeMap::new();
    let mut doc = ArchimateExchange {
        schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
        model_id: format!("id-{}", &hex_sha256(scope.tenant.as_bytes())[..32]),
        name: scope.graph.clone().unwrap_or_else(|| scope.tenant.clone()),
        elements: Vec::new(),
        relationships: Vec::new(),
        organizations: Vec::new(),
        views: Vec::new(),
        property_definitions: Vec::new(),
        preserved: Vec::new(),
    };
    let mut nodes = Vec::new();
    for (id, bytes) in view.core.get_nodes() {
        let props = decode(&bytes);
        if text(&props, "tenant_id").is_some_and(|t| t != scope.tenant) {
            continue;
        }
        if let Some(graph) = &scope.graph {
            if text(&props, "graph") != Some(graph.as_str()) {
                continue;
            }
        }
        let identifier = text(&props, "archimateIdentifier")
            .map(str::to_owned)
            .unwrap_or_else(|| derived_identifier(&id));
        identifiers.insert(id.clone(), identifier);
        nodes.push((id, props));
    }
    for (id, props) in &nodes {
        let identifier = identifiers[id].clone();
        let label = text(props, "type").unwrap_or_default();
        match label {
            "ArchimateRelationship" => {
                let end = |key: &str| {
                    text(props, key)
                        .map(|n| identifiers.get(n).cloned().unwrap_or_else(|| n.to_owned()))
                        .unwrap_or_default()
                };
                doc.relationships.push(ExchangeRelationship {
                    schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
                    identifier,
                    relationship_type: text(props, "relationshipType")
                        .unwrap_or_default()
                        .to_owned(),
                    source: end("relSource"),
                    target: end("relTarget"),
                });
            }
            "ArchimateView" => {
                if let Some(stored) =
                    text(props, "view").and_then(|v| serde_json::from_str::<StoredView>(v).ok())
                {
                    doc.views.push(StoredView {
                        identifier,
                        ..stored
                    });
                }
            }
            _ => {
                let Some(element_type) = view.types.element_type_of(label) else {
                    continue;
                };
                let properties = props
                    .iter()
                    .filter_map(|(k, v)| {
                        Some((k.strip_prefix("prop:")?.to_owned(), v.as_str()?.to_owned()))
                    })
                    .collect();
                doc.elements.push(ExchangeElement {
                    schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
                    identifier,
                    element_type: element_type.to_owned(),
                    name: text(props, "name").unwrap_or_default().to_owned(),
                    properties,
                });
            }
        }
    }
    // Property definitions: one per property name, identifier from the name.
    let names: std::collections::BTreeSet<String> = doc
        .elements
        .iter()
        .flat_map(|e| e.properties.keys().cloned())
        .collect();
    for element in &mut doc.elements {
        element.properties = std::mem::take(&mut element.properties)
            .into_iter()
            .map(|(k, v)| (property_identifier(&k), v))
            .collect();
    }
    for name in names {
        doc.property_definitions.push(
            medulla_types::archimate_exchange::ExchangePropertyDefinition {
                identifier: property_identifier(&name),
                name,
                data_type: "string".to_owned(),
            },
        );
    }
    doc.elements.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    doc.relationships
        .sort_by(|a, b| a.identifier.cmp(&b.identifier));
    doc.views.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    doc
}

fn property_identifier(name: &str) -> String {
    format!("propid-{}", &hex_sha256(name.as_bytes())[..16])
}

fn name_xml(out: &mut String, name: &str) {
    out.push_str(&format!("<name xml:lang=\"en\">{}</name>", escape(name)));
}

/// Serialize a document in identifier order; equal documents give equal bytes.
pub fn exchange_bytes(doc: &ArchimateExchange) -> Vec<u8> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<model xmlns=\"{ARCHIMATE_NS}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" identifier=\"{}\">",
        escape(&doc.model_id)
    ));
    name_xml(&mut out, &doc.name);
    out.push_str("<elements>");
    for e in &doc.elements {
        out.push_str(&format!(
            "<element identifier=\"{}\" xsi:type=\"{}\">",
            escape(&e.identifier),
            escape(&e.element_type)
        ));
        name_xml(&mut out, &e.name);
        if !e.properties.is_empty() {
            out.push_str("<properties>");
            for (k, v) in &e.properties {
                out.push_str(&format!(
                    "<property propertyDefinitionRef=\"{}\"><value xml:lang=\"en\">{}</value></property>",
                    escape(k),
                    escape(v)
                ));
            }
            out.push_str("</properties>");
        }
        out.push_str("</element>");
    }
    out.push_str("</elements><relationships>");
    for r in &doc.relationships {
        out.push_str(&format!(
            "<relationship identifier=\"{}\" source=\"{}\" target=\"{}\" xsi:type=\"{}\"/>",
            escape(&r.identifier),
            escape(&r.source),
            escape(&r.target),
            escape(&r.relationship_type)
        ));
    }
    out.push_str("</relationships>");
    if !doc.property_definitions.is_empty() {
        out.push_str("<propertyDefinitions>");
        for d in &doc.property_definitions {
            out.push_str(&format!(
                "<propertyDefinition identifier=\"{}\" type=\"{}\">",
                escape(&d.identifier),
                escape(&d.data_type)
            ));
            name_xml(&mut out, &d.name);
            out.push_str("</propertyDefinition>");
        }
        out.push_str("</propertyDefinitions>");
    }
    if !doc.views.is_empty() {
        out.push_str("<views><diagrams>");
        for v in &doc.views {
            out.push_str(&format!(
                "<view identifier=\"{}\" xsi:type=\"Diagram\"",
                escape(&v.identifier)
            ));
            if let Some(viewpoint) = &v.viewpoint {
                out.push_str(&format!(" viewpoint=\"{}\"", escape(viewpoint)));
            }
            out.push('>');
            name_xml(&mut out, &v.name);
            for n in &v.nodes {
                out.push_str(&format!("<node identifier=\"{}\"", escape(&n.identifier)));
                if let Some(r) = &n.element_ref {
                    out.push_str(&format!(" elementRef=\"{}\"", escape(r)));
                }
                out.push_str(&format!(
                    " xsi:type=\"Element\" x=\"{}\" y=\"{}\" w=\"{}\" h=\"{}\"/>",
                    n.x, n.y, n.w, n.h
                ));
            }
            for c in &v.connections {
                out.push_str(&format!(
                    "<connection identifier=\"{}\"",
                    escape(&c.identifier)
                ));
                if let Some(r) = &c.relationship_ref {
                    out.push_str(&format!(" relationshipRef=\"{}\"", escape(r)));
                }
                out.push_str(&format!(
                    " xsi:type=\"Relationship\" source=\"{}\" target=\"{}\"/>",
                    escape(&c.source),
                    escape(&c.target)
                ));
            }
            out.push_str("</view>");
        }
        out.push_str("</diagrams></views>");
    }
    out.push_str("</model>\n");
    out.into_bytes()
}

/// Write the visible nodes whose class resolves an `:archimateType` within the
/// scope as one Open Exchange Format document. Equal graphs give equal bytes.
pub fn write_exchange(
    view: &ExchangeView<'_>,
    scope: &ExportScope,
) -> Result<Vec<u8>, ArchimateExchangeError> {
    Ok(exchange_bytes(&collect_exchange(view, scope)))
}
