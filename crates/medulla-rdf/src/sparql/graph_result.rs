use super::*;

// ── CONSTRUCT / DESCRIBE (CONCEPT:MEDULLA-KG.query.named-graph-support) ───────────────────────────────────────

/// Instantiate a CONSTRUCT template against each WHERE solution → the result graph.
/// A pattern whose terms can't all be resolved/built for a given solution is skipped
/// (SPARQL: an unbound or ill-typed template slot yields no triple for that solution).
#[cfg(feature = "rdf")]
pub(super) fn construct_graph(
    template: &[TriplePattern],
    solutions: &[Solution],
) -> Vec<oxrdf::Triple> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sol in solutions {
        for tp in template {
            if let Some(t) = instantiate_triple(tp, sol) {
                if seen.insert(t.to_string()) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Resolve ONE template triple pattern against a solution to a concrete RDF triple.
/// A bound term keeps its datatype and language tag; a slot that is unbound, or bound to a
/// term its position cannot hold (a literal subject, a non-IRI predicate), yields no triple.
#[cfg(feature = "rdf")]
pub(super) fn instantiate_triple(tp: &TriplePattern, sol: &Solution) -> Option<oxrdf::Triple> {
    use oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};

    let subject: NamedOrBlankNode = match &tp.subject {
        TermPattern::NamedNode(n) => NamedOrBlankNode::NamedNode(n.clone()),
        TermPattern::BlankNode(b) => NamedOrBlankNode::BlankNode(b.clone()),
        TermPattern::Variable(v) => match sol.get(v.as_str())?.to_oxrdf_term()? {
            Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
            Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b),
            _ => return None,
        },
        _ => return None,
    };
    let predicate: NamedNode = match &tp.predicate {
        NamedNodePattern::NamedNode(n) => n.clone(),
        NamedNodePattern::Variable(v) => match sol.get(v.as_str())? {
            Binding::Iri(iri) => NamedNode::new(iri.as_str()).ok()?,
            _ => return None,
        },
    };
    let object: Term = match &tp.object {
        TermPattern::NamedNode(n) => Term::NamedNode(n.clone()),
        TermPattern::BlankNode(b) => Term::BlankNode(b.clone()),
        TermPattern::Literal(l) => Term::Literal(l.clone()),
        TermPattern::Variable(v) => sol.get(v.as_str())?.to_oxrdf_term()?,
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    Some(Triple::new(subject, predicate, object))
}

/// Build the DESCRIBE graph: every resource bound (subject- or object-position) by the
/// WHERE pattern's variables, described by all triples of the active graph that mention
/// it (subject OR object position) — a minimal concise bounded description.
#[cfg(feature = "rdf")]
pub(super) fn describe_resources(
    ctx: &Ctx,
    vars: &[String],
    solutions: &[Solution],
) -> Vec<oxrdf::Triple> {
    let resources = describe_resource_set(vars, solutions);
    // All triples of the active graph (term-string form), filtered to those touching a
    // described resource in subject or object position.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (s, p, o) in all_triples_terms(ctx) {
        if !describes(&resources, &s, &o) {
            continue;
        }
        if let Some(t) = build_triple(&s, &p, &o) {
            if seen.insert(t.to_string()) {
                out.push(t);
            }
        }
    }
    out
}

/// The resource set: the node keys of every IRI or blank-node binding across the projected
/// vars. A `DESCRIBE <iri>` constant arrives as an `Extend`-bound IRI, and a `DESCRIBE ?x`
/// as the IRI or blank node the variable holds.
#[cfg(feature = "rdf")]
pub(super) fn describe_resource_set(
    vars: &[String],
    solutions: &[Solution],
) -> std::collections::HashSet<String> {
    let mut resources: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sol in solutions {
        for v in vars {
            if let Some(b) = sol.get(v) {
                resources.extend(b.node_keys());
            }
        }
    }
    resources
}

/// Whether a triple (subject node key, object term) touches a described resource in
/// subject or object position.
#[cfg(feature = "rdf")]
pub(super) fn describes(
    resources: &std::collections::HashSet<String>,
    s: &str,
    o: &Binding,
) -> bool {
    resources.contains(s) || o.node_keys().iter().any(|key| resources.contains(key))
}

/// Enumerate the active graph as `(subject node key, predicate, object term)` — the same
/// projection the `?s ?p ?o` BGP scan produces.
#[cfg(feature = "rdf")]
pub(super) fn all_triples_terms(ctx: &Ctx) -> Vec<(String, String, Binding)> {
    let view = ctx.active;
    let proj = ctx.proj;
    let mut out = Vec::new();
    push_edge_triples_terms(view, proj, &mut out);
    push_node_triples_terms(view, proj, &mut out);
    out
}

/// The `edge_properties` half of [`all_triples_terms`]: every relationship edge as a
/// `(subject, predicate, object, true)` term triple.
#[cfg(feature = "rdf")]
pub(super) fn push_edge_triples_terms(
    view: &GraphView,
    proj: &Projection,
    out: &mut Vec<(String, String, Binding)>,
) {
    for ((s, o), blobs) in &view.edge_properties {
        for blob in blobs {
            let Ok(v) = medulla_types::msgpack::decode_property_value(blob.as_slice()) else {
                continue;
            };
            let Some(rel) = v.get("relationship").and_then(|x| x.as_str()) else {
                continue;
            };
            out.push((
                proj.node_iri(s),
                proj.pred_iri(rel),
                Binding::from_node_key(&proj.node_iri(o)),
            ));
        }
    }
}

/// The `node_properties` half of [`all_triples_terms`]: each node's `rdf:type` triple
/// (if typed) plus one literal triple per remaining scalar property.
#[cfg(feature = "rdf")]
pub(super) fn push_node_triples_terms(
    view: &GraphView,
    proj: &Projection,
    out: &mut Vec<(String, String, Binding)>,
) {
    for (id, blob) in &view.node_properties {
        let Ok(v) = medulla_types::msgpack::decode_property_value(blob.as_slice()) else {
            continue;
        };
        let Some(obj) = v.as_object() else { continue };
        let subj_iri = proj.node_iri(id);
        if let Some(ty) = obj
            .get("type")
            .or_else(|| obj.get("node_type"))
            .and_then(|x| x.as_str())
        {
            if let Some(type_obj) = proj.node_type_iri(id, ty) {
                out.push((
                    subj_iri.clone(),
                    RDF_TYPE_IRI.to_string(),
                    Binding::from_node_key(&type_obj),
                ));
            }
        }
        for (k, cell) in literal_cells(obj) {
            if let Some(lit_val) = Binding::from_cell(cell) {
                out.push((subj_iri.clone(), proj.pred_iri(k), lit_val));
            }
        }
    }
}

/// An RDF triple from a subject node key, a bare predicate IRI and an object term; `None`
/// when a position cannot hold its term.
#[cfg(feature = "rdf")]
pub(super) fn build_triple(s: &str, p: &str, o: &Binding) -> Option<oxrdf::Triple> {
    use oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};
    let subj = match Binding::from_node_key(s).to_oxrdf_term()? {
        Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
        Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b),
        _ => return None,
    };
    let pred = NamedNode::new(p.trim_start_matches('<').trim_end_matches('>')).ok()?;
    Some(Triple::new(subj, pred, o.to_oxrdf_term()?))
}
