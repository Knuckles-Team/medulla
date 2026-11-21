//! The semantic source-dirty wakeup of a graph commit (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! A semantic index built from a graph text property is woken by the commit that changes one of
//! its nodes. [`compile_methods`](super::compile_methods) is the one chokepoint every graph write
//! passes, so the decision lives here: a batch writes a source node when it adds or creates a
//! node of a bound type that carries the bound property, updates the property, or removes a node
//! (a removal names no type, so it wakes every source bound in the graph).

use medulla_graph_algos::algorithms::{decode_batch_operations, BatchOperation};
use medulla_index_types::semantic_index::{bound_graph_sources, SemanticDigest};
use medulla_protocol_types::protocol::Method;
use sha2::{Digest, Sha256};

/// The wakeup input digest of `methods` committed to `graph` of `tenant`, or `None` when the
/// batch writes no node of a bound `(type, property)`.
pub(super) fn graph_source_dirty_input(
    tenant: &str,
    graph: &str,
    methods: &[Method],
) -> Option<SemanticDigest> {
    let bound = bound_graph_sources(tenant, graph);
    if bound.is_empty() || !methods.iter().any(|method| writes_bound_node(&bound, method)) {
        return None;
    }
    let encoded = rmp_serde::to_vec_named(methods).ok()?;
    Some(SemanticDigest::from_bytes(Sha256::digest(&encoded).into()))
}

/// Whether `properties_msgpack` is a node of a bound type that carries the bound property.
fn is_source_node(bound: &[(String, String)], properties_msgpack: &[u8]) -> bool {
    let Ok(properties) = medulla_types::msgpack::decode_property_object(properties_msgpack) else {
        return false;
    };
    let node_type = properties.get("type").and_then(|value| value.as_str());
    bound.iter().any(|(type_id, property_id)| {
        node_type == Some(type_id.as_str()) && properties.contains_key(property_id)
    })
}

/// Whether `updates_msgpack` sets a bound property.
fn updates_bound_property(bound: &[(String, String)], updates_msgpack: &[u8]) -> bool {
    medulla_types::msgpack::decode_property_object(updates_msgpack).is_ok_and(|updates| {
        bound
            .iter()
            .any(|(_, property_id)| updates.contains_key(property_id))
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
        let wake = |methods: Vec<Method>| graph_source_dirty_input("dirty-t", "dirty-g", &methods);
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
    fn a_commit_elsewhere_wakes_nothing() {
        bind("dirty-t2", "dirty-g2");
        let wake = |tenant: &str, graph: &str, methods: Vec<Method>| {
            graph_source_dirty_input(tenant, graph, &methods)
        };
        let other_type = add("doc:1", &[("type", "Document"), ("text", "hello")]);
        assert!(wake("dirty-t2", "dirty-g2", vec![other_type]).is_none());
        let no_property = add("bookmark:2", &[("type", "Bookmark")]);
        assert!(wake("dirty-t2", "dirty-g2", vec![no_property]).is_none());
        let bookmark = add("bookmark:3", &[("type", "Bookmark"), ("text", "hello")]);
        assert!(wake("dirty-t2", "other-graph", vec![bookmark.clone()]).is_none());
        assert!(wake("other-tenant", "dirty-g2", vec![bookmark]).is_none());
    }
}
