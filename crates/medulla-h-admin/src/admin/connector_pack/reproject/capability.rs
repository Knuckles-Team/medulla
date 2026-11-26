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

use medulla_persistence_records::connector_pack::descriptor::{
    descriptor_of, is_typed, triples_of, INVALID,
};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Triple};
use medulla_types::contract::TenantId;

/// The `cp:Capability` nodes among `triples`, keyed by their rendered subject.
///
/// One pass over the triples. The projection now carries every triple the pack
/// ontologies declare, so deciding "is this subject a capability" by rescanning all
/// triples per subject (as `is_typed` does) was quadratic in the pack size.
fn capability_subjects(triples: &[Triple]) -> BTreeMap<String, NamedOrBlankNode> {
    let class = format!("{CP}Capability");
    triples
        .iter()
        .inspect(|_| {
            #[cfg(test)]
            SUBJECT_SCAN_VISITS.with(|visits| visits.set(visits.get() + 1));
        })
        .filter(|t| {
            t.predicate.as_str() == RDF_TYPE
                && matches!(&t.object, Term::NamedNode(n) if n.as_str() == class)
        })
        .map(|t| (t.subject.to_string(), t.subject.clone()))
        .collect()
}

#[cfg(test)]
thread_local! {
    /// Triples visited by [`capability_subjects`] on this thread (complexity pin).
    static SUBJECT_SCAN_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Replace every declared `cp:Capability` node of `individuals` (and the ports
/// it consumes) by the canonical triples of its validated descriptor. Every
/// other individual passes through. Descriptors come back in capability-id
/// order, so the projection is deterministic.
pub(super) fn persist_descriptors(
    tenant: &str,
    individuals: Vec<Triple>,
) -> Result<Vec<Triple>, String> {
    let subjects = capability_subjects(&individuals);
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
    use crate::admin::connector_pack::surface_vocab::{CP, RDF_TYPE};
    use medulla_persistence_records::connector_pack::descriptor::{cp, number};
    use medulla_rdf::oxrdf::{Literal, NamedNode, Term};

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

    // spec: MEDULLA-DECISION-ENGINE-R650.2.1
    #[test]
    fn capability_subject_discovery_is_one_pass_over_the_pack_triples() {
        let ty = NamedNode::new_unchecked(RDF_TYPE);
        let mut triples = declared(&[]);
        for i in 0..5_000 {
            let subject = NamedNode::new_unchecked(format!("urn:pack:class/{i}"));
            triples.push(Triple::new(
                subject.clone(),
                ty.clone(),
                Term::from(NamedNode::new_unchecked("http://www.w3.org/2002/07/owl#Class")),
            ));
            triples.push(Triple::new(
                subject,
                NamedNode::new_unchecked("http://www.w3.org/2000/01/rdf-schema#label"),
                Term::from(Literal::new_simple_literal("c")),
            ));
        }
        SUBJECT_SCAN_VISITS.with(|visits| visits.set(0));
        let out = persist_descriptors("tenant-a", triples.clone()).unwrap();
        let visits = SUBJECT_SCAN_VISITS.with(|visits| visits.get());
        assert_eq!(
            visits,
            triples.len(),
            "capability discovery must visit each triple once, not once per subject"
        );
        assert!(out.len() >= 10_000, "every non-capability triple passes through");
    }
}
