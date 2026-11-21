//! The semantic source-dirty wakeup of a graph commit (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! A semantic index built from a graph text property is woken by the commit that changes one of
//! its nodes. [`compile_methods`](super::compile_methods) is the one chokepoint every graph write
//! passes, so the decision lives here, and it is derived from the node rows the commit writes,
//! never from which method asked for the write: a runtime-derived write (an RDF load, a
//! conditional update, a job write-set) commits through a staged node row delta, and that delta
//! is what [`SourceDirtyBasis::RowDelta`] names. A direct row write (`AddNode`, `RemoveNode`, a
//! `BatchUpdate` of them) is its own row delta, so [`SourceDirtyBasis::Methods`] reads the
//! methods. A replayed commit reproduces the decision the original commit recorded
//! ([`SourceDirtyBasis::Recorded`]), because the wakeup is part of the batch's payload digest.
//!
//! A row writes a source node when it carries a bound type and the bound property. A removal
//! row names no type, and a field update that sets a node's `type` has no before image to say
//! which type the node left, so both wake every source bound in the graph; a wakeup is a hint
//! and the reconcile reads the source, so an extra one costs a read and never changes an answer.
//!
//! A bound type or property matches a node's by its exact canonical IRI, never by a suffix: a
//! bare name or a `:`-prefixed name expands into the core namespace, a full IRI stands for
//! itself, and the two sides are compared after that expansion.

use medulla_graph_algos::algorithms::{decode_batch_operations, BatchOperation};
use medulla_index_types::semantic_index::{
    bound_graph_sources, SemanticDigest, SemanticSourceDirtyIntent, SEMANTIC_SOURCE_DIRTY_TOPIC,
};
use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_protocol_types::protocol::Method;
use sha2::{Digest, Sha256};

/// Where a commit's node writes are read from.
pub enum SourceDirtyBasis<'a> {
    /// The methods are the row writes (a direct row commit).
    Methods,
    /// The staged node row delta of the commit.
    RowDelta(&'a [Method]),
    /// A replay: the wakeup input the original commit recorded, if it recorded one.
    Recorded(Option<SemanticDigest>),
}

/// The wakeup input digest recorded in an already committed batch, if it carries one.
pub fn recorded_source_dirty_input(batch: &MutationBatch) -> Option<SemanticDigest> {
    batch
        .outbox
        .iter()
        .find(|intent| intent.topic == SEMANTIC_SOURCE_DIRTY_TOPIC)
        .and_then(|intent| SemanticSourceDirtyIntent::from_canonical_cbor(&intent.payload).ok())
        .map(|intent| intent.input_digest)
}

/// The wakeup input digest of a commit of `methods` to `graph` of `tenant`, or `None` when the
/// commit writes no node of a bound `(type, property)`.
pub(super) fn graph_source_dirty_input(
    tenant: &str,
    graph: &str,
    methods: &[Method],
    basis: &SourceDirtyBasis<'_>,
) -> Option<SemanticDigest> {
    let rows = match basis {
        SourceDirtyBasis::Recorded(recorded) => return *recorded,
        SourceDirtyBasis::Methods => methods,
        SourceDirtyBasis::RowDelta(rows) => rows,
    };
    let bound = bound_graph_sources(tenant, graph);
    if bound.is_empty() || !rows.iter().any(|method| writes_bound_node(&bound, method)) {
        return None;
    }
    let encoded = rmp_serde::to_vec_named(rows).ok()?;
    Some(SemanticDigest::from_bytes(Sha256::digest(&encoded).into()))
}

/// The canonical IRI of a type or property name: a full IRI is itself, a bare or `:`-prefixed
/// name lives in the core namespace.
fn canonical_iri(name: &str) -> String {
    let name = name.trim();
    if name.contains("://") {
        name.to_string()
    } else {
        format!(
            "{}{}",
            medulla_types::ontology_base::CORE_NAMESPACE,
            name.trim_start_matches(':')
        )
    }
}

/// Whether `actual` names the same type or property as `bound`, by exact canonical IRI.
fn names(actual: &str, bound: &str) -> bool {
    actual == bound || canonical_iri(actual) == canonical_iri(bound)
}

/// The key of a node's type property.
const TYPE_KEY: &str = "type";

/// Whether `properties_msgpack` is a node of a bound type that carries the bound property.
fn is_source_node(bound: &[(String, String)], properties_msgpack: &[u8]) -> bool {
    let Ok(properties) = medulla_types::msgpack::decode_property_object(properties_msgpack) else {
        return false;
    };
    let node_type = properties.get("type").and_then(|value| value.as_str());
    bound.iter().any(|(type_id, property_id)| {
        node_type.is_some_and(|actual| names(actual, type_id))
            && properties.keys().any(|key| names(key, property_id))
    })
}

/// Whether `updates_msgpack` sets a bound property, or sets a node's type: the update names the
/// type the node moves to and not the one it leaves, so any source of the graph may have lost it.
fn updates_bound_property(bound: &[(String, String)], updates_msgpack: &[u8]) -> bool {
    medulla_types::msgpack::decode_property_object(updates_msgpack).is_ok_and(|updates| {
        updates.contains_key(TYPE_KEY)
            || bound.iter().any(|(_, property_id)| {
                updates.keys().any(|key| names(key, property_id))
            })
    })
}

fn writes_bound_node(bound: &[(String, String)], method: &Method) -> bool {
    match method {
        Method::AddNode {
            properties_msgpack, ..
        }
        | Method::CreateNodeIfAbsent {
            properties_msgpack, ..
        } => is_source_node(bound, properties_msgpack),
        Method::CompareAndSetNodeFields { updates_msgpack, .. } => {
            updates_bound_property(bound, updates_msgpack)
        }
        Method::RemoveNode { .. } => true,
        Method::BatchUpdate { operations_msgpack } => decode_batch_operations(operations_msgpack)
            .map(|operations| {
                operations.iter().any(|operation| match operation {
                    BatchOperation::AddNode {
                        properties_msgpack, ..
                    } => is_source_node(bound, properties_msgpack),
                    BatchOperation::RemoveNode { .. } => true,
                    _ => false,
                })
            })
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_index_types::semantic_index::{bind_graph_source, GraphTextPropertyRef};
    use std::collections::BTreeMap;

    fn node(properties: &[(&str, &str)]) -> Vec<u8> {
        let map: BTreeMap<&str, &str> = properties.iter().copied().collect();
        rmp_serde::to_vec_named(&map).unwrap()
    }

    fn bind(tenant: &str, graph: &str) {
        bind_graph_source(
            tenant,
            &GraphTextPropertyRef {
                graph_id: graph.into(),
                type_id: "Bookmark".into(),
                property_id: "text".into(),
            },
        );
    }

    fn add(id: &str, properties: &[(&str, &str)]) -> Method {
        Method::AddNode {
            node_id: id.into(),
            properties_msgpack: node(properties),
        }
    }

    #[test]
    fn a_commit_of_a_bound_node_wakes_the_source() {
        bind("dirty-t", "dirty-g");
        let wake = |methods: Vec<Method>| graph_source_dirty_input("dirty-t", "dirty-g", &methods, &SourceDirtyBasis::Methods);
        assert!(wake(vec![add("bookmark:1", &[("type", "Bookmark"), ("text", "hello")])]).is_some());
        let removal = Method::RemoveNode {
            node_id: "bookmark:1".into(),
        };
        assert!(wake(vec![removal]).is_some());
        let update = Method::CompareAndSetNodeFields {
            node_id: "bookmark:1".into(),
            conditions_msgpack: node(&[]),
            updates_msgpack: node(&[("text", "edited")]),
        };
        assert!(wake(vec![update]).is_some());
    }

    #[test]
    fn a_type_or_property_matches_by_its_exact_canonical_iri_never_by_a_suffix() {
        bind("dirty-t6", "dirty-g6");
        let wake = |properties: &[(&str, &str)]| {
            graph_source_dirty_input(
                "dirty-t6",
                "dirty-g6",
                &[add("bookmark:1", properties)],
                &SourceDirtyBasis::Methods,
            )
        };
        let core = medulla_types::ontology_base::CORE_NAMESPACE;
        let core_type = format!("{core}Bookmark");
        let core_text = format!("{core}text");
        // The bare name, the prefixed name and the core IRI are one canonical type and property.
        assert!(wake(&[("type", "Bookmark"), ("text", "x")]).is_some());
        assert!(wake(&[("type", ":Bookmark"), ("text", "x")]).is_some());
        assert!(wake(&[("type", core_type.as_str()), ("text", "x")]).is_some());
        assert!(wake(&[("type", "Bookmark"), (core_text.as_str(), "x")]).is_some());
        // A different namespace ending in the same name is a different type and property.
        assert!(wake(&[("type", "https://other.test/vocab#Bookmark"), ("text", "x")]).is_none());
        assert!(wake(&[("type", "https://other.test/vocab/Bookmark"), ("text", "x")]).is_none());
        assert!(wake(&[("type", "Bookmark"), ("https://other.test/vocab#text", "x")]).is_none());
        assert!(wake(&[("type", "MyBookmark"), ("text", "x")]).is_none());
    }

    #[test]
    fn a_removal_and_a_type_change_wake_every_bound_source_of_the_graph() {
        bind("dirty-t7", "dirty-g7");
        let wake = |graph: &str, methods: &[Method]| {
            graph_source_dirty_input("dirty-t7", graph, methods, &SourceDirtyBasis::Methods)
        };
        let removal = Method::RemoveNode { node_id: "doc:anything".into() };
        assert!(wake("dirty-g7", &[removal.clone()]).is_some());
        // A removal in a graph with no bound source wakes nothing.
        assert!(wake("unbound-graph", &[removal]).is_none());
        // A field update that sets `type` wakes the sources of the graph, whatever type it sets
        // and whether or not it names the bound property.
        let retype = |to: &str| Method::CompareAndSetNodeFields {
            node_id: "bookmark:1".into(),
            conditions_msgpack: node(&[]),
            updates_msgpack: node(&[("type", to)]),
        };
        assert!(wake("dirty-g7", &[retype("Document")]).is_some());
        assert!(wake("dirty-g7", &[retype("Bookmark")]).is_some());
        assert!(wake("unbound-graph", &[retype("Document")]).is_none());
        // An update of a field that is neither the type nor the bound property wakes nothing.
        let other = Method::CompareAndSetNodeFields {
            node_id: "bookmark:1".into(),
            conditions_msgpack: node(&[]),
            updates_msgpack: node(&[("title", "edited")]),
        };
        assert!(wake("dirty-g7", &[other]).is_none());
        // The same holds through a staged row delta.
        let delta = [Method::RemoveNode { node_id: "bookmark:1".into() }];
        assert!(row_delta_wake("dirty-t7", "dirty-g7", &delta).is_some());
    }

    #[test]
    fn a_commit_elsewhere_wakes_nothing() {
        bind("dirty-t2", "dirty-g2");
        let wake = |tenant: &str, graph: &str, methods: Vec<Method>| {
            graph_source_dirty_input(tenant, graph, &methods, &SourceDirtyBasis::Methods)
        };
        let other_type = add("doc:1", &[("type", "Document"), ("text", "hello")]);
        assert!(wake("dirty-t2", "dirty-g2", vec![other_type]).is_none());
        let no_property = add("bookmark:2", &[("type", "Bookmark")]);
        assert!(wake("dirty-t2", "dirty-g2", vec![no_property]).is_none());
        let bookmark = add("bookmark:3", &[("type", "Bookmark"), ("text", "hello")]);
        assert!(wake("dirty-t2", "other-graph", vec![bookmark.clone()]).is_none());
        assert!(wake("other-tenant", "dirty-g2", vec![bookmark]).is_none());
    }

    fn row_delta_wake(tenant: &str, graph: &str, staged: &[Method]) -> Option<SemanticDigest> {
        graph_source_dirty_input(tenant, graph, &[], &SourceDirtyBasis::RowDelta(staged))
    }

    /// The node row delta a runtime-derived commit produces: `methods` applied to an empty
    /// graph, diffed, as the staged commit path does.
    #[cfg(feature = "rdf")]
    fn staged_rows(methods: &[Method]) -> Vec<Method> {
        let base = medulla_graph_core::graph::GraphCore::new().snapshot();
        let staged = medulla_graph_core::graph::GraphCore::from_snapshot(base.clone(), 0).unwrap();
        for method in methods {
            crate::mutation_apply::apply(&staged, method).unwrap();
        }
        medulla_kernel::graph_delta::GraphRowDelta::between(&base, &staged.snapshot())
            .unwrap()
            .operations()
            .to_vec()
    }

    #[cfg(feature = "rdf")]
    #[test]
    fn an_rdf_load_of_a_bound_node_wakes_the_source_through_the_row_delta() {
        bind("dirty-t3", "dirty-g3");
        let load = |turtle: &str| {
            staged_rows(&[Method::AddTriples {
                turtle: turtle.to_string(),
                ntriples: String::new(),
            }])
        };
        let bookmark = load(
            "<urn:bookmark:1> a <https://knuckles-team.github.io/ontology/core#Bookmark> ; \
             <https://knuckles-team.github.io/ontology/core#text> \"hello\" .",
        );
        assert!(row_delta_wake("dirty-t3", "dirty-g3", &bookmark).is_some());
        let other = load(
            "<urn:doc:1> a <https://knuckles-team.github.io/ontology/core#Document> ; \
             <https://knuckles-team.github.io/ontology/core#text> \"hello\" .",
        );
        assert!(row_delta_wake("dirty-t3", "dirty-g3", &other).is_none());
        let untexted = load("<urn:bookmark:2> a <https://knuckles-team.github.io/ontology/core#Bookmark> .");
        assert!(row_delta_wake("dirty-t3", "dirty-g3", &untexted).is_none());
    }

    #[test]
    fn an_add_node_row_of_a_bound_node_wakes_the_source_through_the_row_delta() {
        bind("dirty-t4", "dirty-g4");
        let bookmark = [add("bookmark:9", &[("type", "Bookmark"), ("text", "hello")])];
        assert!(row_delta_wake("dirty-t4", "dirty-g4", &bookmark).is_some());
        let other = [add("doc:9", &[("type", "Document"), ("text", "hello")])];
        assert!(row_delta_wake("dirty-t4", "dirty-g4", &other).is_none());
        assert!(row_delta_wake("dirty-t4", "unbound-graph", &bookmark).is_none());
    }

    #[test]
    fn a_replayed_commit_reproduces_the_recorded_wakeup() {
        let recorded = SemanticDigest::from_bytes([7; 32]);
        let wake = |basis: SourceDirtyBasis<'_>| {
            graph_source_dirty_input("dirty-t5", "dirty-g5", &[], &basis)
        };
        assert_eq!(wake(SourceDirtyBasis::Recorded(Some(recorded))), Some(recorded));
        assert_eq!(wake(SourceDirtyBasis::Recorded(None)), None);
    }
}
