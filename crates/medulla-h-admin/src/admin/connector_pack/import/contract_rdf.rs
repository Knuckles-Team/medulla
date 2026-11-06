//! The pure JSON-to-RDF mapping of a connector's ingest contract
//! (MEDULLA-TYPED-PACKS-R101.2).
//!
//! `contract_graph` turns the connector declaration into the graph the contract
//! shapes in `connector_pack-v1.shapes.ttl` validate. It emits exactly what the
//! declaration states and never judges it: a missing required key leaves its
//! triple out, and the shape (not this function) refuses the contract with
//! `CONTRACT_SHACL_VIOLATION`.

use medulla_catalog_types::connector_pack::PackViolationCode;
use medulla_rdf::oxrdf::{Graph, Literal, NamedNode, Subject, Term, Triple};
use serde_json::Value;

use crate::admin::connector_pack::surface_vocab::{CONNECTOR_BASE, CP, RDF_TYPE};

const CONTRACT_BASE: &str = "http://knuckles.team/kg/connector-pack/contract/";

/// Percent-encode one IRI path segment (RFC 3986 unreserved characters stay).
pub(super) fn segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn cp(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CP}{local}"))
}

struct Builder {
    graph: Graph,
}

impl Builder {
    fn node(&self, iri: String) -> Result<NamedNode, PackViolationCode> {
        NamedNode::new(iri).map_err(|_| PackViolationCode::ContractShaclViolation)
    }

    fn typed(&mut self, subject: &NamedNode, class: &str) {
        self.graph.insert(&Triple::new(
            Subject::from(subject.clone()),
            NamedNode::new_unchecked(RDF_TYPE),
            Term::from(cp(class)),
        ));
    }

    fn link(&mut self, subject: &NamedNode, predicate: &str, object: &NamedNode) {
        self.graph.insert(&Triple::new(
            Subject::from(subject.clone()),
            cp(predicate),
            Term::from(object.clone()),
        ));
    }

    fn text(&mut self, subject: &NamedNode, predicate: &str, value: &str) {
        self.graph.insert(&Triple::new(
            Subject::from(subject.clone()),
            cp(predicate),
            Term::from(Literal::new_simple_literal(value)),
        ));
    }

    fn text_opt(&mut self, subject: &NamedNode, predicate: &str, value: Option<&Value>) {
        if let Some(text) = value.and_then(Value::as_str) {
            self.text(subject, predicate, text);
        }
    }

    fn texts(&mut self, subject: &NamedNode, predicate: &str, values: Option<&Value>) {
        for text in values
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            self.text(subject, predicate, text);
        }
    }
}

/// Map one connector declaration onto the contract graph.
///
/// Subject IRIs are `http://knuckles.team/kg/connector-pack/contract/<connector>`
/// for the contract with `/<object>`, `/<object>/key`,
/// `/<object>/field/<source_field>`, `/cursor`, `/access` and `/provenance`
/// suffixes, each segment percent-encoded.
pub fn contract_graph(connector: &str, declaration: &Value) -> Result<Graph, PackViolationCode> {
    let mut b = Builder {
        graph: Graph::new(),
    };
    let base = format!("{CONTRACT_BASE}{}", segment(connector));
    let contract = b.node(base.clone())?;
    b.typed(&contract, "IngestContract");
    let connector_node = b.node(format!("{CONNECTOR_BASE}{}", segment(connector)))?;
    b.link(&contract, "contractOf", &connector_node);

    let body = declaration.get("contract");
    b.text_opt(
        &contract,
        "contractVersion",
        body.and_then(|c| c.get("version")),
    );
    b.text_opt(
        &contract,
        "contractDigest",
        body.and_then(|c| c.get("digest")),
    );
    b.text_opt(
        &contract,
        "privacyClass",
        body.and_then(|c| c.get("privacy_class")),
    );

    if let Some(mappings) = declaration
        .get("schema_mappings")
        .and_then(Value::as_object)
    {
        for (object, mapping) in mappings {
            let object_iri = b.node(format!("{base}/{}", segment(object)))?;
            b.typed(&object_iri, "SourceObject");
            b.link(&contract, "hasSourceObject", &object_iri);
            b.text(&object_iri, "objectName", object);
            if let Some(class) = mapping
                .get("ontology_class")
                .and_then(Value::as_str)
                .filter(|class| !class.is_empty())
            {
                let class = b.node(class.to_string())?;
                b.link(&object_iri, "ontologyClass", &class);
            }
            let keys = mapping
                .get("key_fields")
                .and_then(Value::as_array)
                .map(|keys| keys.as_slice())
                .unwrap_or_default();
            if !keys.is_empty() {
                let key_rule = b.node(format!("{base}/{}/key", segment(object)))?;
                b.typed(&key_rule, "KeyRule");
                b.link(&object_iri, "hasKeyRule", &key_rule);
                for key in keys.iter().filter_map(Value::as_str) {
                    b.text(&key_rule, "keyField", key);
                }
            }
            if let Some(fields) = mapping.get("fields").and_then(Value::as_object) {
                for (source_field, predicate) in fields {
                    let field = b.node(format!(
                        "{base}/{}/field/{}",
                        segment(object),
                        segment(source_field)
                    ))?;
                    b.typed(&field, "FieldMapping");
                    b.link(&object_iri, "hasFieldMapping", &field);
                    b.text(&field, "sourceField", source_field);
                    b.text_opt(&field, "mapsPredicate", Some(predicate));
                }
            }
        }
    }

    if let Some(cursor) = body.and_then(|c| c.get("cursor")) {
        let node = b.node(format!("{base}/cursor"))?;
        b.typed(&node, "CursorPolicy");
        b.link(&contract, "hasCursorPolicy", &node);
        b.text_opt(&node, "cursorKind", cursor.get("kind"));
    }
    if let Some(access) = body.and_then(|c| c.get("access")) {
        let node = b.node(format!("{base}/access"))?;
        b.typed(&node, "AccessPolicy");
        b.link(&contract, "hasAccessPolicy", &node);
        b.text_opt(&node, "defaultAcl", access.get("default_acl"));
        b.text_opt(&node, "aclField", access.get("acl_field"));
        b.text_opt(&node, "tenantField", access.get("tenant_field"));
        b.texts(&node, "readRole", access.get("read_roles"));
    }
    if let Some(provenance) = body.and_then(|c| c.get("provenance")) {
        let node = b.node(format!("{base}/provenance"))?;
        b.typed(&node, "ProvenanceRequirement");
        b.link(&contract, "requiresProvenance", &node);
        b.texts(&node, "requiredField", provenance.get("required_fields"));
    }
    Ok(b.graph)
}

/// The contract shapes document from the composed core catalog.
pub fn contract_shapes() -> Option<&'static str> {
    use std::sync::OnceLock;
    static SHAPES: OnceLock<Option<String>> = OnceLock::new();
    SHAPES
        .get_or_init(|| {
            medulla_graph_core::graph::GraphSchemaSources::default()
                .shapes()
                .find(|(id, _)| id.starts_with("core:connector-pack@"))
                .map(|(_, text)| text.to_string())
        })
        .as_deref()
}

#[cfg(all(test, feature = "shacl"))]
mod tests {
    use super::*;

    fn declaration() -> Value {
        serde_json::json!({
            "schema_mappings": {"ticket": {
                "ontology_class": "http://knuckles.team/kg#Ticket",
                "key_fields": ["id"],
                "fields": {"title": "http://knuckles.team/kg#title"}
            }},
            "contract": {
                "version": "1", "digest": "sha256:abc", "privacy_class": "internal",
                "cursor": {"kind": "timestamp"},
                "access": {"default_acl": "quarantine", "read_roles": ["reader"]},
                "provenance": {"required_fields": ["source_id"]}
            }
        })
    }

    fn conforms(declaration: &Value) -> bool {
        let shapes = medulla_shacl::graph_from_turtle(contract_shapes().expect("registered"))
            .expect("shapes parse");
        let data = contract_graph("acme", declaration).expect("maps");
        medulla_shacl::validate(&shapes, &data)
            .expect("validates")
            .conforms
    }

    #[test]
    fn mapped_contract_conforms_and_a_missing_key_is_refused_by_the_shape() {
        assert!(conforms(&declaration()));
        let mut missing_key = declaration();
        missing_key["schema_mappings"]["ticket"]["key_fields"] = serde_json::json!([]);
        assert!(!conforms(&missing_key));
        let mut bad_cursor = declaration();
        bad_cursor["contract"]["cursor"]["kind"] = serde_json::json!("watermark");
        assert!(!conforms(&bad_cursor));
    }
}
