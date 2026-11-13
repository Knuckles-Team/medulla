//! Capability descriptors of a pack head, as the properties of their
//! `Capability` nodes (MEDULLA-DECISION-ENGINE-R650.2.1).
//!
//! A pack ontology declares a capability as a `cp:Capability` individual with
//! the descriptor's fields as `cp:` properties, and each port as a `cp:Port`
//! individual. Projection reads every such node into one typed
//! [`CapabilityDescriptor`], validates it, and writes the descriptor's
//! canonical triples in place of the declared ones. Reliability is derived
//! from the learned reward when a descriptor is served (`derived_reliability`)
//! and is never stored on the node: a pack that declares `cp:reliability` is
//! refused.

use std::collections::{BTreeMap, HashSet};

use medulla_agent_types::agent_component::capability_descriptor::{
    derived_reliability, ApprovalClass, CapabilityDescriptor, DataPort, Locality, SideEffectClass,
    CAPABILITY_DESCRIPTOR_VERSION,
};
use medulla_decision_types::decision::capability_designation::{
    DESCRIPTOR_APPROVAL_CLASS_MISSING, DESCRIPTOR_SIDE_EFFECT_MISSING,
};
use medulla_rdf::oxrdf::{Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use medulla_types::contract::{ResourceId, ScopeKind, TenantId};

use crate::admin::connector_pack::surface_vocab::{CP, RDF_TYPE};

const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const INVALID: &str = "PACK_PROJECTION_CAPABILITY_INVALID";

fn cp(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CP}{local}"))
}

fn is_typed(triples: &[Triple], subject: &NamedOrBlankNode, class: &str) -> bool {
    triples.iter().any(|t| {
        &t.subject == subject
            && t.predicate.as_str() == RDF_TYPE
            && matches!(&t.object, Term::NamedNode(n) if n.as_str() == format!("{CP}{class}"))
    })
}

fn invalid(subject: &NamedOrBlankNode, detail: impl std::fmt::Display) -> String {
    format!("{INVALID}: capability {subject}: {detail}")
}

/// The literal values of `predicate` on `subject`.
fn literals<'a>(
    triples: &'a [Triple],
    subject: &'a NamedOrBlankNode,
    predicate: &'a str,
) -> impl Iterator<Item = &'a Term> + 'a {
    let predicate = format!("{CP}{predicate}");
    triples
        .iter()
        .filter(move |t| &t.subject == subject && t.predicate.as_str() == predicate)
        .map(|t| &t.object)
}

fn one_literal(
    triples: &[Triple],
    subject: &NamedOrBlankNode,
    predicate: &str,
) -> Result<String, String> {
    let mut values = literals(triples, subject, predicate);
    match (values.next(), values.next()) {
        (Some(Term::Literal(value)), None) => Ok(value.value().to_string()),
        _ => Err(invalid(
            subject,
            format!("cp:{predicate} must be declared exactly once as a literal"),
        )),
    }
}

fn one_number(
    triples: &[Triple],
    subject: &NamedOrBlankNode,
    predicate: &str,
) -> Result<u64, String> {
    one_literal(triples, subject, predicate)?
        .parse()
        .map_err(|_| invalid(subject, format!("cp:{predicate} is not a whole number")))
}

fn enum_value<T: serde::de::DeserializeOwned>(
    triples: &[Triple],
    subject: &NamedOrBlankNode,
    predicate: &str,
) -> Result<T, String> {
    let text = one_literal(triples, subject, predicate)?;
    serde_json::from_value(serde_json::Value::String(text.clone())).map_err(|_| {
        invalid(
            subject,
            format!("cp:{predicate} '{text}' is not a known value"),
        )
    })
}

fn ports(
    triples: &[Triple],
    subject: &NamedOrBlankNode,
    predicate: &str,
    consumed: &mut HashSet<NamedOrBlankNode>,
) -> Result<Vec<DataPort>, String> {
    let mut found = Vec::new();
    for object in literals(triples, subject, predicate) {
        let Term::NamedNode(node) = object else {
            return Err(invalid(
                subject,
                format!("cp:{predicate} must name a cp:Port node"),
            ));
        };
        let port = NamedOrBlankNode::NamedNode(node.clone());
        if !is_typed(triples, &port, "Port") {
            return Err(invalid(subject, format!("{node} is not a cp:Port")));
        }
        found.push(DataPort {
            name: ResourceId::new(one_literal(triples, &port, "portName")?)
                .map_err(|e| invalid(subject, e))?,
            data_type: ResourceId::new(one_literal(triples, &port, "dataType")?)
                .map_err(|e| invalid(subject, e))?,
        });
        consumed.insert(port);
    }
    found.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    Ok(found)
}

/// One capability node's descriptor, validated. Reliability is derived from no
/// reward (neutral) here: the node never stores it.
fn descriptor_of(
    tenant: &TenantId,
    triples: &[Triple],
    subject: &NamedOrBlankNode,
    consumed: &mut HashSet<NamedOrBlankNode>,
) -> Result<CapabilityDescriptor, String> {
    let NamedOrBlankNode::NamedNode(node) = subject else {
        return Err(invalid(subject, "a capability node must be a named node"));
    };
    if literals(triples, subject, "reliability").next().is_some() {
        return Err(invalid(
            subject,
            "cp:reliability is derived from the learned reward and is never declared",
        ));
    }
    for (predicate, code) in [
        ("sideEffectClass", DESCRIPTOR_SIDE_EFFECT_MISSING),
        ("approvalClass", DESCRIPTOR_APPROVAL_CLASS_MISSING),
    ] {
        if literals(triples, subject, predicate).next().is_none() {
            return Err(format!(
                "{}: capability {subject}: cp:{predicate} is required before a descriptor is published",
                code.as_str()
            ));
        }
    }
    let mut scopes = Vec::new();
    for value in literals(triples, subject, "scope") {
        let Term::Literal(scope) = value else {
            return Err(invalid(subject, "cp:scope must be a literal"));
        };
        scopes.push(ScopeKind::new(scope.value()).map_err(|e| invalid(subject, e))?);
    }
    scopes.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let descriptor = CapabilityDescriptor {
        version: CAPABILITY_DESCRIPTOR_VERSION,
        tenant: tenant.clone(),
        capability_id: ResourceId::new(node.as_str()).map_err(|e| invalid(subject, e))?,
        inputs: ports(triples, subject, "input", consumed)?,
        outputs: ports(triples, subject, "output", consumed)?,
        side_effect_class: enum_value::<SideEffectClass>(triples, subject, "sideEffectClass")?,
        scopes,
        cost: one_number(triples, subject, "cost")?,
        latency_ms: one_number(triples, subject, "latencyMs")?,
        reliability: derived_reliability(None),
        locality: enum_value::<Locality>(triples, subject, "locality")?,
        approval_class: enum_value::<ApprovalClass>(triples, subject, "approvalClass")?,
    };
    descriptor.validate().map_err(|e| invalid(subject, e))?;
    Ok(descriptor)
}

fn enum_literal<T: serde::Serialize>(value: &T) -> Literal {
    let text = serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    Literal::new_simple_literal(text)
}

fn number(value: u64) -> Literal {
    Literal::new_typed_literal(value.to_string(), NamedNode::new_unchecked(XSD_INTEGER))
}

/// The canonical triples of one descriptor: its properties on the capability
/// node, without `reliability`, and one node per port.
fn triples_of(descriptor: &CapabilityDescriptor) -> Result<Vec<Triple>, String> {
    let node = NamedNode::new(descriptor.capability_id.as_str()).map_err(|e| {
        format!(
            "{INVALID}: capability '{}' is not an IRI: {e}",
            descriptor.capability_id.as_str()
        )
    })?;
    let ty = NamedNode::new_unchecked(RDF_TYPE);
    let mut out = vec![
        Triple::new(node.clone(), ty.clone(), Term::from(cp("Capability"))),
        Triple::new(
            node.clone(),
            cp("descriptorVersion"),
            Term::from(number(u64::from(descriptor.version))),
        ),
        Triple::new(
            node.clone(),
            cp("sideEffectClass"),
            Term::from(enum_literal(&descriptor.side_effect_class)),
        ),
        Triple::new(
            node.clone(),
            cp("cost"),
            Term::from(number(descriptor.cost)),
        ),
        Triple::new(
            node.clone(),
            cp("latencyMs"),
            Term::from(number(descriptor.latency_ms)),
        ),
        Triple::new(
            node.clone(),
            cp("locality"),
            Term::from(enum_literal(&descriptor.locality)),
        ),
        Triple::new(
            node.clone(),
            cp("approvalClass"),
            Term::from(enum_literal(&descriptor.approval_class)),
        ),
    ];
    for scope in &descriptor.scopes {
        out.push(Triple::new(
            node.clone(),
            cp("scope"),
            Term::from(Literal::new_simple_literal(scope.as_str())),
        ));
    }
    for (predicate, list) in [
        ("input", &descriptor.inputs),
        ("output", &descriptor.outputs),
    ] {
        for port in list {
            let port_node = NamedNode::new(format!(
                "{}/{predicate}/{}",
                descriptor.capability_id.as_str(),
                port.name.as_str()
            ))
            .map_err(|e| {
                format!(
                    "{INVALID}: port '{}' is not an IRI: {e}",
                    port.name.as_str()
                )
            })?;
            out.push(Triple::new(
                node.clone(),
                cp(predicate),
                Term::from(port_node.clone()),
            ));
            out.push(Triple::new(
                port_node.clone(),
                ty.clone(),
                Term::from(cp("Port")),
            ));
            out.push(Triple::new(
                port_node.clone(),
                cp("portName"),
                Term::from(Literal::new_simple_literal(port.name.as_str())),
            ));
            out.push(Triple::new(
                port_node,
                cp("dataType"),
                Term::from(Literal::new_simple_literal(port.data_type.as_str())),
            ));
        }
    }
    Ok(out)
}

/// Replace every declared `cp:Capability` node of `individuals` (and the ports
/// it consumes) by the canonical triples of its validated descriptor. Every
/// other individual passes through. Descriptors come back in capability-id
/// order, so the projection is deterministic.
pub(super) fn persist_descriptors(
    tenant: &str,
    individuals: Vec<Triple>,
) -> Result<Vec<Triple>, String> {
    let subjects: BTreeMap<String, NamedOrBlankNode> = individuals
        .iter()
        .map(|t| &t.subject)
        .filter(|s| is_typed(&individuals, s, "Capability"))
        .map(|s| (s.to_string(), s.clone()))
        .collect();
    if subjects.is_empty() {
        return Ok(individuals);
    }
    let tenant = TenantId::new(tenant).map_err(|e| format!("{INVALID}: tenant: {e}"))?;
    let mut consumed = HashSet::new();
    let mut descriptors = Vec::new();
    for subject in subjects.values() {
        descriptors.push(descriptor_of(
            &tenant,
            &individuals,
            subject,
            &mut consumed,
        )?);
    }
    let replaced: HashSet<&NamedOrBlankNode> = subjects.values().chain(consumed.iter()).collect();
    let mut out: Vec<Triple> = individuals
        .iter()
        .filter(|t| !replaced.contains(&t.subject))
        .cloned()
        .collect();
    for descriptor in &descriptors {
        out.extend(triples_of(descriptor)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: &str = "eg:capability/translate";

    fn node(iri: &str) -> NamedOrBlankNode {
        NamedOrBlankNode::NamedNode(NamedNode::new_unchecked(iri))
    }

    fn declared(extra: &[(&str, Term)]) -> Vec<Triple> {
        let ty = NamedNode::new_unchecked(RDF_TYPE);
        let lit = |v: &str| Term::from(Literal::new_simple_literal(v));
        let port = "eg:capability/translate#text";
        let mut triples = vec![
            Triple::new(
                NamedNode::new_unchecked(CAP),
                ty.clone(),
                Term::from(cp("Capability")),
            ),
            Triple::new(
                NamedNode::new_unchecked(CAP),
                cp("sideEffectClass"),
                lit("external"),
            ),
            Triple::new(
                NamedNode::new_unchecked(CAP),
                cp("cost"),
                Term::from(number(12)),
            ),
            Triple::new(
                NamedNode::new_unchecked(CAP),
                cp("latencyMs"),
                Term::from(number(300)),
            ),
            Triple::new(
                NamedNode::new_unchecked(CAP),
                cp("locality"),
                lit("network"),
            ),
            Triple::new(
                NamedNode::new_unchecked(CAP),
                cp("approvalClass"),
                lit("low"),
            ),
            Triple::new(NamedNode::new_unchecked(CAP), cp("scope"), lit("tenant")),
            Triple::new(
                NamedNode::new_unchecked(CAP),
                cp("input"),
                Term::from(NamedNode::new_unchecked(port)),
            ),
            Triple::new(NamedNode::new_unchecked(port), ty, Term::from(cp("Port"))),
            Triple::new(NamedNode::new_unchecked(port), cp("portName"), lit("text")),
            Triple::new(
                NamedNode::new_unchecked(port),
                cp("dataType"),
                lit("eg:type/text"),
            ),
        ];
        for (predicate, value) in extra {
            triples.push(Triple::new(
                NamedNode::new_unchecked(CAP),
                cp(predicate),
                value.clone(),
            ));
        }
        triples
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.2.1
    #[test]
    fn declared_capability_is_persisted_without_reliability() {
        let out = persist_descriptors("tenant-a", declared(&[])).unwrap();
        let has = |p: &str| {
            out.iter()
                .any(|t| t.predicate.as_str() == format!("{CP}{p}"))
        };
        for property in [
            "sideEffectClass",
            "cost",
            "latencyMs",
            "locality",
            "approvalClass",
            "scope",
            "input",
            "portName",
            "dataType",
            "descriptorVersion",
        ] {
            assert!(has(property), "{property}");
        }
        assert!(
            !has("reliability"),
            "the stored node never holds reliability"
        );
        // The projection is deterministic.
        assert_eq!(out, persist_descriptors("tenant-a", declared(&[])).unwrap());
        assert!(out.iter().any(|t| t.subject == node(CAP)));
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.2.1
    #[test]
    fn a_declared_reliability_or_invalid_field_is_refused() {
        let reliability = declared(&[("reliability", Term::from(number(9000)))]);
        let error = persist_descriptors("tenant-a", reliability).unwrap_err();
        assert!(
            error.starts_with("PACK_PROJECTION_CAPABILITY_INVALID"),
            "{error}"
        );
        let mut bad = declared(&[]);
        bad.retain(|t| t.predicate.as_str() != format!("{CP}cost"));
        let error = persist_descriptors("tenant-a", bad).unwrap_err();
        assert!(error.contains("cp:cost"), "{error}");
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.4
    #[test]
    fn a_descriptor_without_a_side_effect_or_approval_class_is_refused() {
        for (predicate, code) in [
            ("sideEffectClass", "DESCRIPTOR_SIDE_EFFECT_MISSING"),
            ("approvalClass", "DESCRIPTOR_APPROVAL_CLASS_MISSING"),
        ] {
            let mut triples = declared(&[]);
            triples.retain(|t| t.predicate.as_str() != format!("{CP}{predicate}"));
            // The import writes nothing for the descriptor: no triples come
            // back, only the typed refusal.
            let error = persist_descriptors("tenant-a", triples).unwrap_err();
            assert!(error.starts_with(&format!("{code}:")), "{error}");
            assert!(error.contains(predicate), "{error}");
        }
        // A complete descriptor is not refused.
        assert!(persist_descriptors("tenant-a", declared(&[])).is_ok());
    }
}
