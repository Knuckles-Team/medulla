//! The connector surface of a pack head, as triples (MEDULLA-TYPED-PACKS-R102.2,
//! R102.6).
//!
//! One node per tool, prompt, skill and server member. The engine stamps the
//! node's type, `cp:componentId`, entry revision, pack digest and lifecycle;
//! the declared surface links come from the pack's ontology documents.
//! Projection writes the result into the pack graph beside the schema source.

use std::sync::Arc;

use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_catalog_types::connector_pack::PackEntryKind;
use medulla_persistence_records::connector_pack::projection::{
    ConnectorPackProjectionPlan, PackSurfaceMember,
};
use medulla_rdf::oxrdf::{Literal, NamedNode, NamedOrBlankNode, Term, Triple};

use crate::admin::connector_pack::surface_vocab::{link_object, CP, RDF_TYPE};
use crate::graph_schema::PackSchemaDocument;

const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// The class the engine types each surface member with.
fn surface_class(kind: PackEntryKind) -> &'static str {
    match kind {
        PackEntryKind::Tool => "Tool",
        PackEntryKind::Prompt => "Prompt",
        PackEntryKind::Skill => "Skill",
        _ => "Connector",
    }
}

/// The lifecycle literal a node carries.
fn lifecycle_label(lifecycle: AgentLibraryLifecycle) -> &'static str {
    match lifecycle {
        AgentLibraryLifecycle::Published => "active",
        AgentLibraryLifecycle::Retired => "retired",
        AgentLibraryLifecycle::Withdrawn => "withdrawn",
    }
}

fn cp(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CP}{local}"))
}

fn node_of(member: &PackSurfaceMember) -> Result<NamedNode, String> {
    NamedNode::new(&member.uri).map_err(|error| {
        format!(
            "PACK_PROJECTION_SURFACE_IRI_INVALID: member '{}' is not an IRI: {error}",
            member.uri
        )
    })
}

fn stamp(subject: &NamedNode, predicate: &str, value: Literal) -> Triple {
    Triple::new(subject.clone(), cp(predicate), Term::from(value))
}

/// The triples the engine stamps on one member's node.
fn stamped(member: &PackSurfaceMember, digest: &str) -> Result<Vec<Triple>, String> {
    let node = node_of(member)?;
    let revision = Literal::new_typed_literal(
        member.entry_revision.to_string(),
        NamedNode::new_unchecked(XSD_INTEGER),
    );
    Ok(vec![
        Triple::new(
            node.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            Term::from(cp(surface_class(member.kind))),
        ),
        stamp(
            &node,
            "componentId",
            Literal::new_simple_literal(&member.component_id),
        ),
        stamp(&node, "entryRevision", revision),
        stamp(&node, "packDigest", Literal::new_simple_literal(digest)),
        stamp(
            &node,
            "lifecycle",
            Literal::new_simple_literal(lifecycle_label(member.lifecycle)),
        ),
    ])
}

/// The surface links the pack ontologies declare, in document order.
fn declared_links(ontologies: &[PackSchemaDocument]) -> Result<Vec<Triple>, String> {
    let mut links = Vec::new();
    for document in ontologies {
        let triples = medulla_rdf::mapping::parse_turtle(&document.body)?;
        links.extend(triples.into_iter().filter(|triple| {
            matches!(triple.subject, NamedOrBlankNode::NamedNode(_))
                && link_object(triple.predicate.as_str()).is_some()
        }));
    }
    Ok(links)
}

/// The surface of `plan`'s head as an N-Triples document, or `None` when the
/// head has no surface member.
pub(super) fn surface_triples(
    plan: &ConnectorPackProjectionPlan,
    ontologies: &[PackSchemaDocument],
) -> Result<Option<Arc<str>>, String> {
    if plan.surface.is_empty() {
        return Ok(None);
    }
    let digest = plan.pack_digest().to_hex();
    let mut triples = Vec::new();
    for member in &plan.surface {
        triples.extend(stamped(member, &digest)?);
    }
    triples.extend(declared_links(ontologies)?);
    let document = medulla_rdf::mapping::to_ntriples(&triples)?;
    Ok(Some(Arc::from(document)))
}

/// Write the surface document into the pack graph, inside the graph gateway.
///
/// The nodes the document names are replaced, not merged: every `cp:` triple a
/// node held from an earlier head goes first, so a changed revision, digest or
/// lifecycle never leaves its old value beside the new one.
pub(crate) fn write_surface(
    core: &medulla_graph_core::graph::GraphCore,
    graph: &str,
    surface: Option<&str>,
) -> Result<(), String> {
    let Some(document) = surface else {
        return Ok(());
    };
    let fresh = medulla_rdf::mapping::parse_ntriples(document)?;
    let nodes: std::collections::HashSet<_> = fresh.iter().map(|t| t.subject.clone()).collect();
    let stale: Vec<Triple> = medulla_rdf::mapping::export_triples(core, graph)?
        .into_iter()
        .filter(|t| nodes.contains(&t.subject) && t.predicate.as_str().starts_with(CP))
        .collect();
    medulla_rdf::update::remove_triples(core, &stale);
    medulla_rdf::update::insert_triples(core, &fresh)?;
    Ok(())
}
