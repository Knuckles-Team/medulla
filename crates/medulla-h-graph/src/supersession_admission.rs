//! Request-boundary check of the supersessions one change envelope carries
//! (MEDULLA-REPO-INGEST-R002.651.2).
//!
//! A supersession is two methods every envelope batch already carries: an
//! `AddEdge` with relationship `supersedes` from the new fact to the old one, and a
//! `CompareAndSetNodeFields` that marks the old fact `superseded`
//! (`supersession_methods`). The engine verifies instead of mutating, so the
//! envelope bytes stay stable for replay. The check runs under the graph write lock,
//! before any durable work.

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::change_envelope::{ChangeEnvelope, SUPERSEDES};
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::ServerErrorCode;

fn refuse(code: ServerErrorCode, detail: String) -> String {
    format!("{}: {detail}", code.as_str())
}

fn decode(blob: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    medulla_types::msgpack::decode_property_object(blob).unwrap_or_default()
}

/// Verify the supersessions of `envelope` against `core`.
///
/// For each `AddEdge` whose relationship is `supersedes`, the target must exist in
/// `core` or in an `AddNode` earlier in the same batch (`SUPERSESSION_UNKNOWN_FACT`).
/// The batch must also hold the `CompareAndSetNodeFields` on that target whose
/// updates set `superseded` to `true` and `superseded_by` to the edge source, and
/// every such update must have its edge (`SUPERSESSION_UNPAIRED`).
pub fn check_supersessions(envelope: &ChangeEnvelope, core: &GraphCore) -> Result<(), String> {
    check_supersession_methods(
        envelope.mutation.operations.iter().map(|op| &op.method),
        |node_id| core.has_node(node_id),
    )
}

/// [`check_supersessions`] over the methods of one batch and a node-existence probe.
pub fn check_supersession_methods<'a>(
    methods: impl Iterator<Item = &'a Method>,
    exists: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let mut added: BTreeSet<&str> = BTreeSet::new();
    // (new fact, old fact) of each supersedes edge; (old fact, new fact) of each mark.
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut marks: BTreeSet<(String, String)> = BTreeSet::new();
    for method in methods {
        match method {
            Method::AddNode { node_id, .. } => {
                added.insert(node_id.as_str());
            }
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => {
                let properties = decode(properties_msgpack);
                if properties.get("relationship").and_then(serde_json::Value::as_str)
                    != Some(SUPERSEDES)
                {
                    continue;
                }
                if !added.contains(target_id.as_str()) && !exists(target_id) {
                    return Err(refuse(
                        ServerErrorCode::SupersessionUnknownFact,
                        format!("the superseded fact '{target_id}' does not exist"),
                    ));
                }
                edges.insert((source_id.clone(), target_id.clone()));
            }
            Method::CompareAndSetNodeFields {
                node_id,
                updates_msgpack,
                ..
            } => {
                let updates = decode(updates_msgpack);
                if updates.get("superseded") == Some(&serde_json::Value::Bool(true)) {
                    let by = updates
                        .get("superseded_by")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    marks.insert((node_id.clone(), by.to_string()));
                }
            }
            _ => {}
        }
    }
    for (new_fact, old_fact) in &edges {
        if !marks.contains(&(old_fact.clone(), new_fact.clone())) {
            return Err(refuse(
                ServerErrorCode::SupersessionUnpaired,
                format!("the supersedes edge from '{new_fact}' to '{old_fact}' has no superseded mark"),
            ));
        }
    }
    for (old_fact, new_fact) in &marks {
        if !edges.contains(&(new_fact.clone(), old_fact.clone())) {
            return Err(refuse(
                ServerErrorCode::SupersessionUnpaired,
                format!("the superseded mark on '{old_fact}' has no supersedes edge from '{new_fact}'"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_ingestion_types::ingest_capabilities::Supersession;
    use medulla_protocol_types::change_envelope::supersession_methods;

    fn supersession(old: &str, new: &str) -> Supersession {
        Supersession {
            schema_version: 1,
            old_fact: old.into(),
            new_fact: new.into(),
            envelope_id: "env-1".into(),
        }
    }

    fn add_node(id: &str) -> Method {
        Method::AddNode {
            node_id: id.into(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"type": "Fact"}))
                .unwrap(),
        }
    }

    // spec: MEDULLA-REPO-INGEST-R002.651.2
    #[test]
    fn unknown_and_unpaired_supersessions_are_refused() {
        let held = |id: &str| id == "fact-old";
        let [edge, mark] = supersession_methods(&supersession("fact-old", "fact-new")).unwrap();

        // A complete pair over a stored old fact and a new fact added in the batch.
        let pair = [add_node("fact-new"), edge.clone(), mark.clone()];
        assert!(check_supersession_methods(pair.iter(), held).is_ok());

        // An absent old fact refuses, even with both methods present.
        let [absent_edge, absent_mark] =
            supersession_methods(&supersession("fact-gone", "fact-new")).unwrap();
        let absent = [add_node("fact-new"), absent_edge, absent_mark];
        let error = check_supersession_methods(absent.iter(), held).unwrap_err();
        assert!(error.starts_with("SUPERSESSION_UNKNOWN_FACT"), "{error}");

        // An old fact added earlier in the same batch is known.
        let [batch_edge, batch_mark] =
            supersession_methods(&supersession("fact-batch", "fact-new")).unwrap();
        let in_batch = [add_node("fact-batch"), add_node("fact-new"), batch_edge, batch_mark];
        assert!(check_supersession_methods(in_batch.iter(), held).is_ok());

        // An edge with no mark, and a mark with no edge, are unpaired.
        let edge_only = [add_node("fact-new"), edge];
        let error = check_supersession_methods(edge_only.iter(), held).unwrap_err();
        assert!(error.starts_with("SUPERSESSION_UNPAIRED"), "{error}");
        let mark_only = [add_node("fact-new"), mark];
        let error = check_supersession_methods(mark_only.iter(), held).unwrap_err();
        assert!(error.starts_with("SUPERSESSION_UNPAIRED"), "{error}");
    }
}
