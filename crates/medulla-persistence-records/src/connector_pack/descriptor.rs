//! The capability descriptor codec (MEDULLA-DECISION-ENGINE-R650.2.1, R650.3).
//!
//! A pack ontology declares a capability as a `cp:Capability` individual with
//! the descriptor's fields as `cp:` properties, and each port as a `cp:Port`
//! individual. [`descriptor_of`] reads one node into a typed, validated
//! [`CapabilityDescriptor`] and [`triples_of`] writes its canonical triples, so
//! the projection writer and the designation reader share one codec.
//! Reliability is derived from the learned reward when a descriptor is served
//! and is never stored: a node that declares `cp:reliability` is refused.

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

/// The connector-pack vocabulary namespace.
pub const CP: &str = medulla_types::ontology_iri!("connector-pack#");
/// The `rdf:type` predicate.
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
pub const INVALID: &str = "PACK_PROJECTION_CAPABILITY_INVALID";

pub fn cp(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CP}{local}"))
}

pub fn is_typed(triples: &[Triple], subject: &NamedOrBlankNode, class: &str) -> bool {
    triples.iter().any(|t| {
        &t.subject == subject
            && t.predicate.as_str() == RDF_TYPE
            && matches!(&t.object, Term::NamedNode(n) if n.as_str() == format!("{CP}{class}"))
    })
}

pub fn invalid(subject: &NamedOrBlankNode, detail: impl std::fmt::Display) -> String {
    format!("{INVALID}: capability {subject}: {detail}")
}

/// The literal values of `predicate` on `subject`.
pub fn literals<'a>(
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

pub fn one_literal(
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

pub fn one_number(
    triples: &[Triple],
    subject: &NamedOrBlankNode,
    predicate: &str,
) -> Result<u64, String> {
    one_literal(triples, subject, predicate)?
        .parse()
        .map_err(|_| invalid(subject, format!("cp:{predicate} is not a whole number")))
}

pub fn enum_value<T: serde::de::DeserializeOwned>(
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

pub fn ports(
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
pub fn descriptor_of(
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

pub fn enum_literal<T: serde::Serialize>(value: &T) -> Literal {
    let text = serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    Literal::new_simple_literal(text)
}

pub fn number(value: u64) -> Literal {
    Literal::new_typed_literal(value.to_string(), NamedNode::new_unchecked(XSD_INTEGER))
}

/// The canonical triples of one descriptor: its properties on the capability
/// node, without `reliability`, and one node per port.
pub fn triples_of(descriptor: &CapabilityDescriptor) -> Result<Vec<Triple>, String> {
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


/// Every `cp:Capability` node of `triples` as its descriptor, keyed by
/// capability IRI. Reliability is 0: it is never stored, so a reader derives it
/// from the learned reward (MEDULLA-DECISION-ENGINE-R650.3).
pub fn descriptors_of(
    tenant: &str,
    triples: &[Triple],
) -> Result<BTreeMap<String, CapabilityDescriptor>, String> {
    let tenant = TenantId::new(tenant).map_err(|e| format!("{INVALID}: tenant: {e}"))?;
    let subjects: BTreeMap<String, NamedOrBlankNode> = triples
        .iter()
        .map(|t| &t.subject)
        .filter(|s| is_typed(triples, s, "Capability"))
        .map(|s| (s.to_string(), s.clone()))
        .collect();
    let mut consumed = HashSet::new();
    let mut out = BTreeMap::new();
    for subject in subjects.values() {
        let mut descriptor = descriptor_of(&tenant, triples, subject, &mut consumed)?;
        descriptor.reliability = 0;
        out.insert(descriptor.capability_id.as_str().to_string(), descriptor);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R650.3
    #[test]
    fn descriptor_round_trips_through_triples() {
        let tenant = TenantId::new("tenant-a").unwrap();
        let descriptor = CapabilityDescriptor {
            version: CAPABILITY_DESCRIPTOR_VERSION,
            tenant: tenant.clone(),
            capability_id: ResourceId::new("eg:capability/translate").unwrap(),
            inputs: vec![DataPort {
                name: ResourceId::new("text").unwrap(),
                data_type: ResourceId::new("eg:type/text").unwrap(),
            }],
            outputs: Vec::new(),
            side_effect_class: SideEffectClass::External,
            scopes: vec![ScopeKind::new("tenant").unwrap()],
            cost: 12,
            latency_ms: 300,
            reliability: 0,
            locality: Locality::Network,
            approval_class: ApprovalClass::Low,
        };
        let triples = triples_of(&descriptor).unwrap();
        let read = descriptors_of("tenant-a", &triples).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read["eg:capability/translate"], descriptor);
    }
}
