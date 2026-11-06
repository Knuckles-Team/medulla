//! Generated ontology and shapes for the decision contract
//! (MEDULLA-DECISION-ENGINE-R054.1).
//!
//! Each JSON schema of `DecideRequest`, `DecisionFitOp`, `DecisionEvalOp` and
//! `StatisticalDecisionRecord` becomes one `owl:Class`; each of its top-level
//! properties becomes one property axiom plus one `sh:property` with its
//! datatype and cardinality. Nothing is committed: `medulla-graph-core`'s
//! `build.rs` renders both documents into `OUT_DIR` at compile time
//! (MEDULLA-SDLC-R020.1). Every statement is sorted by IRI, so generation is
//! byte-stable.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::decision::statistical::{DecideRequest, StatisticalDecisionRecord};
use crate::decision::{DecisionEvalOp, DecisionFitOp};
use serde_json::Value;

/// Namespace every generated IRI lives in (the core ontology namespace).
pub const DECISION_NAMESPACE: &str = "http://knuckles.team/kg#";

/// Core class every contract class extends (MEDULLA-TYPED-PACKS-R071.2): the
/// contract records are information content, the foundation's BFO generically
/// dependent continuant.
pub const DECISION_CORE_PARENT: &str = "bfo:0000031";

/// One property of one contract class.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContractProperty {
    /// XSD datatype local name for a scalar, `None` for an object/array/union value.
    datatype: Option<&'static str>,
    required: bool,
    many: bool,
}

/// One contract class: its IRI local name mapped to its properties by name.
type ContractClasses = BTreeMap<String, BTreeMap<String, ContractProperty>>;

fn contract_classes() -> ContractClasses {
    let mut classes = ContractClasses::new();
    for (name, schema) in [
        ("DecideRequest", schemars::schema_for!(DecideRequest)),
        ("DecisionEvalOp", schemars::schema_for!(DecisionEvalOp)),
        ("DecisionFitOp", schemars::schema_for!(DecisionFitOp)),
        (
            "StatisticalDecisionRecord",
            schemars::schema_for!(StatisticalDecisionRecord),
        ),
    ] {
        classes.insert(name.to_string(), properties_of(schema.as_value()));
    }
    classes
}

/// Top-level properties of one schema. A tagged enum (`oneOf`/`anyOf`) unions
/// the properties of its variants; a property is required only when every
/// variant requires it.
fn properties_of(schema: &Value) -> BTreeMap<String, ContractProperty> {
    let variants: Vec<&Value> = ["oneOf", "anyOf"]
        .iter()
        .find_map(|key| schema.get(*key).and_then(Value::as_array))
        .map(|list| list.iter().collect())
        .unwrap_or_else(|| vec![schema]);
    let mut merged: BTreeMap<String, ContractProperty> = BTreeMap::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for variant in &variants {
        let required: Vec<&str> = variant
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some(props) = variant.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (name, prop) in props {
            let property = ContractProperty {
                datatype: datatype_of(prop),
                required: required.contains(&name.as_str()),
                many: prop.get("type").and_then(Value::as_str) == Some("array"),
            };
            *seen.entry(name.clone()).or_default() += 1;
            merged
                .entry(name.clone())
                .and_modify(|existing| {
                    existing.required &= property.required;
                    if existing.datatype != property.datatype {
                        existing.datatype = None;
                    }
                    existing.many |= property.many;
                })
                .or_insert(property);
        }
    }
    for (name, count) in seen {
        if count < variants.len() {
            if let Some(property) = merged.get_mut(&name) {
                property.required = false;
            }
        }
    }
    merged
}

fn datatype_of(prop: &Value) -> Option<&'static str> {
    if prop.get("const").is_some_and(Value::is_string) || prop.get("enum").is_some() {
        return Some("string");
    }
    match prop.get("type").and_then(Value::as_str)? {
        "string" => Some("string"),
        "boolean" => Some("boolean"),
        "integer" => Some("integer"),
        "number" => Some("double"),
        _ => None,
    }
}

fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// Property IRI local name: `<class lowerCamel>_<field>`, so two classes'
/// same-named fields never share one property with conflicting domains.
fn property_local(class: &str, field: &str) -> String {
    format!("{}_{field}", lower_first(class))
}

/// The generated OWL ontology (`decision.ttl`).
pub fn decision_ontology_ttl() -> String {
    let classes = contract_classes();
    let mut out = String::new();
    out.push_str("# Generated by medulla-capabilities::decision_ontology (MEDULLA-DECISION-ENGINE-R054.1). Do not edit.\n");
    let _ = writeln!(out, "@prefix : <{DECISION_NAMESPACE}> .");
    out.push_str("@prefix bfo: <http://purl.obolibrary.org/obo/BFO_> .\n");
    out.push_str("@prefix owl: <http://www.w3.org/2002/07/owl#> .\n");
    out.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    out.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n");
    let mut statements: BTreeMap<String, String> = BTreeMap::new();
    for (class, properties) in &classes {
        statements.insert(
            class.clone(),
            format!(
                ":{class} a owl:Class ;\n    rdfs:label \"{class}\" ;\n    rdfs:subClassOf {DECISION_CORE_PARENT} .\n"
            ),
        );
        for (field, property) in properties {
            let local = property_local(class, field);
            let statement = match property.datatype {
                Some(datatype) => format!(
                    ":{local} a owl:DatatypeProperty ;\n    rdfs:domain :{class} ;\n    rdfs:label \"{field}\" ;\n    rdfs:range xsd:{datatype} .\n"
                ),
                None => format!(
                    ":{local} a owl:ObjectProperty ;\n    rdfs:domain :{class} ;\n    rdfs:label \"{field}\" .\n"
                ),
            };
            statements.insert(local, statement);
        }
    }
    for statement in statements.values() {
        out.push('\n');
        out.push_str(statement);
    }
    out
}

/// The generated SHACL shapes (`decision.shapes.ttl`).
pub fn decision_shapes_ttl() -> String {
    let classes = contract_classes();
    let mut out = String::new();
    out.push_str("# Generated by medulla-capabilities::decision_ontology (MEDULLA-DECISION-ENGINE-R054.1). Do not edit.\n");
    let _ = writeln!(out, "@prefix : <{DECISION_NAMESPACE}> .");
    out.push_str("@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    out.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n");
    for (class, properties) in &classes {
        let _ = write!(out, "\n:{class}Shape a sh:NodeShape ;\n");
        for (field, property) in properties {
            let mut constraints = Vec::new();
            if let Some(datatype) = property.datatype.filter(|_| !property.many) {
                constraints.push(format!("sh:datatype xsd:{datatype}"));
            }
            if !property.many {
                constraints.push("sh:maxCount 1".to_string());
            }
            if property.required {
                constraints.push("sh:minCount 1".to_string());
            }
            constraints.push(format!("sh:path :{}", property_local(class, field)));
            let _ = writeln!(out, "    sh:property [ {} ] ;", constraints.join(" ; "));
        }
        let _ = writeln!(out, "    sh:targetClass :{class} .");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_byte_stable() {
        assert_eq!(decision_ontology_ttl(), decision_ontology_ttl());
        assert_eq!(decision_shapes_ttl(), decision_shapes_ttl());
    }

    #[test]
    fn every_contract_class_is_declared() {
        let ttl = decision_ontology_ttl();
        for class in [
            "DecideRequest",
            "DecisionEvalOp",
            "DecisionFitOp",
            "StatisticalDecisionRecord",
        ] {
            assert!(ttl.contains(&format!(":{class} a owl:Class")), "{class}");
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R071.2
    #[test]
    fn every_contract_class_extends_the_core_parent() {
        let ttl = decision_ontology_ttl();
        assert!(ttl.contains("@prefix bfo: <http://purl.obolibrary.org/obo/BFO_> ."));
        let declared = ttl.matches(" a owl:Class ;").count();
        let rooted = ttl
            .matches(&format!("rdfs:subClassOf {DECISION_CORE_PARENT} ."))
            .count();
        assert_eq!(declared, 4);
        assert_eq!(rooted, declared, "every class extends a core class");
    }
}
