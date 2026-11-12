//! The connector surface of a pack head, as triples (MEDULLA-TYPED-PACKS-R102.2,
//! R102.6).
//!
//! One node per tool, prompt, skill and server member. The engine stamps the
//! node's type, `cp:componentId`, entry revision, pack digest and lifecycle;
//! the declared surface links come from the pack's ontology documents.
//! The individuals those documents describe (MEDULLA-MEDIA-VOCAB-R655) ride the
//! same document, so served SPARQL answers questions about pack data.
//! Projection writes the result into the pack graph beside the schema source.

use std::sync::Arc;

use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_catalog_types::connector_pack::PackEntryKind;
use medulla_persistence_records::connector_pack::projection::{
    ConnectorPackProjectionPlan, PackSurfaceMember,
};
use medulla_rdf::oxrdf::{Literal, NamedNode, NamedOrBlankNode, Term, Triple};

use crate::admin::connector_pack::surface_vocab::{is_member_uri, link_object, CP, RDF_TYPE};
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

/// Namespaces whose classes type schema terms, not data: a subject typed only
/// with these is vocabulary the schema source already carries.
const SCHEMA_NAMESPACES: [&str; 4] = [
    "http://www.w3.org/2002/07/owl#",
    "http://www.w3.org/2000/01/rdf-schema#",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "http://www.w3.org/ns/shacl#",
];

/// The named individuals among `triples`: subjects typed with at least one class outside the
/// schema namespaces, and not a surface member (the engine stamps those).
fn individual_subjects(triples: &[Triple]) -> std::collections::HashSet<&NamedOrBlankNode> {
    triples
        .iter()
        .filter(|triple| {
            triple.predicate.as_str() == RDF_TYPE
                && matches!(&triple.subject, NamedOrBlankNode::NamedNode(node) if !is_member_uri(node.as_str()))
                && matches!(&triple.object, Term::NamedNode(class)
                    if !SCHEMA_NAMESPACES.iter().any(|ns| class.as_str().starts_with(ns)))
        })
        .map(|triple| &triple.subject)
        .collect()
}

/// Every triple about a named individual the pack ontologies describe.
fn declared_individuals(ontologies: &[PackSchemaDocument]) -> Result<Vec<Triple>, String> {
    let mut individuals = Vec::new();
    for document in ontologies {
        let triples = medulla_rdf::mapping::parse_turtle(&document.body)?;
        let subjects = individual_subjects(&triples);
        individuals.extend(
            triples
                .iter()
                .filter(|triple| subjects.contains(&triple.subject))
                .cloned(),
        );
    }
    Ok(individuals)
}

/// The surface of `plan`'s head and the individuals its ontologies describe as
/// one N-Triples document, or `None` when the head has neither.
pub(super) fn surface_triples(
    plan: &ConnectorPackProjectionPlan,
    ontologies: &[PackSchemaDocument],
) -> Result<Option<Arc<str>>, String> {
    let mut triples = Vec::new();
    if !plan.surface.is_empty() {
        let digest = plan.pack_digest().to_hex();
        for member in &plan.surface {
            triples.extend(stamped(member, &digest)?);
        }
        triples.extend(declared_links(ontologies)?);
    }
    triples.extend(declared_individuals(ontologies)?);
    if triples.is_empty() {
        return Ok(None);
    }
    let document = medulla_rdf::mapping::to_ntriples(&triples)?;
    Ok(Some(Arc::from(document)))
}

/// Write the surface document into the pack graph, inside the graph gateway.
///
/// The projection of a newer head replaces the older one in this one commit
/// (MEDULLA-MEDIA-VOCAB-R655). Every `cp:` triple a node the document names held from an
/// earlier head goes first, and every triple of every individual the graph holds, whether or
/// not the new head still names it, so a changed value never leaves its old value beside the
/// new one and an individual the new head dropped disappears. A head with no surface and no
/// individuals (`None`) clears the individuals too.
pub(crate) fn write_surface(
    core: &medulla_graph_core::graph::GraphCore,
    graph: &str,
    surface: Option<&str>,
) -> Result<(), String> {
    let fresh = match surface {
        Some(document) => medulla_rdf::mapping::parse_ntriples(document)?,
        None => Vec::new(),
    };
    let nodes: std::collections::HashSet<_> = fresh.iter().map(|t| t.subject.clone()).collect();
    let existing = medulla_rdf::mapping::export_triples(core, graph)?;
    let held: std::collections::HashSet<NamedOrBlankNode> = individual_subjects(&existing)
        .into_iter()
        .cloned()
        .collect();
    let stale: Vec<Triple> = existing
        .into_iter()
        .filter(|t| {
            held.contains(&t.subject)
                || (nodes.contains(&t.subject)
                    && (t.predicate.as_str().starts_with(CP)
                        || !matches!(&t.subject, NamedOrBlankNode::NamedNode(node) if is_member_uri(node.as_str()))))
        })
        .collect();
    medulla_rdf::update::remove_triples(core, &stale);
    medulla_rdf::update::insert_triples(core, &fresh)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn individuals_are_projected_and_schema_terms_are_not() {
        let body = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
            @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            :Extra a owl:Class .\n\
            :amount_a a :NutrientAmount ; :nutrientId \"1003\" .\n";
        let documents = [PackSchemaDocument {
            uri: "ontology://food/foods".to_string(),
            body: Arc::from(body),
        }];
        let triples = declared_individuals(&documents).unwrap();
        assert_eq!(triples.len(), 2, "{triples:?}");
        assert!(triples
            .iter()
            .all(|t| t.subject.to_string() == "<https://knuckles-team.github.io/ontology/core#amount_a>"));
    }

    fn individuals(ids: &[&str]) -> String {
        let mut document = String::new();
        for id in ids {
            document.push_str(&format!(
                "<https://knuckles-team.github.io/ontology/core#{id}> <{RDF_TYPE}> <https://knuckles-team.github.io/ontology/core#NutrientAmount> .\n\
                 <https://knuckles-team.github.io/ontology/core#{id}> <https://knuckles-team.github.io/ontology/core#nutrientId> \"1003\" .\n"
            ));
        }
        document
    }

    fn subjects_of(core: &medulla_graph_core::graph::GraphCore) -> Vec<String> {
        let mut out: Vec<String> = medulla_rdf::mapping::export_triples(core, "pack")
            .unwrap()
            .iter()
            .map(|t| t.subject.to_string())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    // spec: MEDULLA-MEDIA-VOCAB-R655
    #[test]
    fn a_newer_head_drops_the_individuals_it_no_longer_names() {
        let core = medulla_graph_core::graph::GraphCore::new();
        write_surface(&core, "pack", Some(&individuals(&["amount_a", "amount_b"]))).unwrap();
        assert_eq!(subjects_of(&core).len(), 2);
        write_surface(&core, "pack", Some(&individuals(&["amount_a"]))).unwrap();
        assert_eq!(
            subjects_of(&core),
            vec!["<https://knuckles-team.github.io/ontology/core#amount_a>".to_string()],
            "amount_b was dropped from the newer head"
        );
        write_surface(&core, "pack", None).unwrap();
        assert!(
            subjects_of(&core).is_empty(),
            "a head with no individuals clears them"
        );
    }
}
