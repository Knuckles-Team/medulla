//! Generic edge writers may not write an `owl:sameAs` edge (MEDULLA-UNIFIED-DATA-PLANE-R030.4).
//!
//! `ApproveEntityLink` is the only writer of an edge whose property blob names the relation
//! `owl:sameAs`, because it alone checks that the pair is a `Match` and stamps `approved_by`.
//! It writes through `GraphCore::add_edge`, not through a [`Method`], so this guard, which
//! judges only generic edge writers, never sees it. It is called where a generic write is
//! admitted: the server's commit gateway, and the durable row applier inside the write
//! transaction, so staged, batch and replicated edges are judged too.

use crate::protocol::Method;

/// The relation name only `ApproveEntityLink` may write.
const SAME_AS: &str = "owl:sameAs";

/// The refusal of a generic `owl:sameAs` write.
const LINK_NOT_APPROVABLE: &str =
    "LINK_NOT_APPROVABLE: an owl:sameAs edge is written only by ApproveEntityLink";

/// Whether an edge property blob names the relation `owl:sameAs`. The relation is read the
/// way the graph core reads it: the first present of `relationship`, `type` and `label`.
fn is_same_as(properties_msgpack: &[u8]) -> bool {
    let Ok(properties) = medulla_types::msgpack::decode_property_object(properties_msgpack) else {
        return false;
    };
    ["relationship", "type", "label"]
        .iter()
        .find_map(|key| properties.get(*key).and_then(|value| value.as_str()))
        == Some(SAME_AS)
}

/// Refuse a generic edge write that carries an `owl:sameAs` edge: `AddEdge`, `TxnAddEdge`
/// and every `AddEdge` of a `BatchUpdate`. Every other method and edge passes.
pub fn refuse_unapproved_same_as_write(method: &Method) -> Result<(), String> {
    let same_as = match method {
        Method::AddEdge {
            properties_msgpack, ..
        }
        | Method::TxnAddEdge {
            properties_msgpack, ..
        } => is_same_as(properties_msgpack),
        Method::BatchUpdate { operations_msgpack } => {
            use crate::algorithms::BatchOperation;

            crate::algorithms::decode_batch_operations(operations_msgpack)?
                .iter()
                .any(|operation| match operation {
                    BatchOperation::AddEdge {
                        properties_msgpack, ..
                    } => is_same_as(properties_msgpack),
                    _ => false,
                })
        }
        _ => false,
    };
    if same_as {
        Err(LINK_NOT_APPROVABLE.to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgpack(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    fn add_edge(relationship: &str) -> Method {
        Method::AddEdge {
            source_id: "a".into(),
            target_id: "b".into(),
            properties_msgpack: msgpack(serde_json::json!({"relationship": relationship})),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R030.4
    #[test]
    fn owl_same_as_edges_are_refused_on_every_generic_edge_writer() {
        let refused = [
            add_edge("owl:sameAs"),
            Method::TxnAddEdge {
                txn_id: "t".into(),
                source_id: "a".into(),
                target_id: "b".into(),
                properties_msgpack: msgpack(serde_json::json!({"relationship": "owl:sameAs"})),
                graph: None,
            },
            Method::BatchUpdate {
                operations_msgpack: msgpack(serde_json::json!([
                    {"op": "add_edge", "source": "a", "target": "b",
                     "properties": {"relationship": "owl:sameAs"}},
                ])),
            },
        ];
        for method in &refused {
            let error = refuse_unapproved_same_as_write(method).unwrap_err();
            assert!(error.starts_with("LINK_NOT_APPROVABLE"), "{method:?}");
        }
        let passing = [
            add_edge("knows"),
            Method::BatchUpdate {
                operations_msgpack: msgpack(serde_json::json!([
                    {"op": "add_edge", "source": "a", "target": "b",
                     "properties": {"relationship": "knows"}},
                    {"op": "add_node", "id": "n", "properties": {"relationship": "owl:sameAs"}},
                ])),
            },
            Method::NodeCount,
        ];
        for method in &passing {
            assert_eq!(
                refuse_unapproved_same_as_write(method),
                Ok(()),
                "{method:?}"
            );
        }
    }
}
