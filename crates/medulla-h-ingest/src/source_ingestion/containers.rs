//! Admission of `containers` batches (MEDULLA-INGEST-WORKER-R904.3).
//!
//! A `containers` batch carries the folders, spaces or channels of one source. The
//! plan comes from `medulla_core::source_hierarchy::plan_containers`, the same planner
//! `upsert_containers` applies to a graph core, so a served batch and a core upsert
//! write the same nodes and edges. This module compiles that plan into methods.

use std::collections::BTreeMap;

use medulla_core::source_hierarchy::{self, ContainerError, CONTAINED_IN};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::SourceJson;
use medulla_types::contract::ServerErrorCode;
use medulla_types::source_governance::SourceContainer;
use serde_json::{json, Value};

/// The edge from a record to the container it sits in.
pub const IN_CONTAINER: &str = "inContainer";

/// The methods of one admitted `containers` batch, in write order.
#[derive(Debug, Default)]
pub struct ContainerAdmission {
    /// Nodes first, then dropped `containedIn` edges, then written ones.
    pub methods: Vec<Method>,
    /// The container nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
}

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| {
        format!(
            "{}: {error}",
            ServerErrorCode::SourceContainerInvalid.as_str()
        )
    })
}

/// A hierarchy refusal as a wire refusal: a code already on the wire keeps itself, any
/// other becomes `SOURCE_CONTAINER_INVALID` with the hierarchy code in its text.
fn wire(error: ContainerError) -> String {
    if error.code.starts_with("SOURCE_") {
        error.to_string()
    } else {
        format!(
            "{}: {error}",
            ServerErrorCode::SourceContainerInvalid.as_str()
        )
    }
}

/// Decode the elements of a `containers` extension.
pub fn decode_containers(items: &[SourceJson]) -> Result<Vec<SourceContainer>, String> {
    items
        .iter()
        .map(|item| {
            let decoded: SourceContainer = serde_json::from_value(item.value()?).map_err(|e| {
                format!("{}: {e}", ServerErrorCode::SourceContainerInvalid.as_str())
            })?;
            Ok(decoded)
        })
        .collect()
}

/// The node ids whose held state `admit_containers` needs (see
/// [`source_hierarchy::held_ids`]).
pub fn held_ids(
    tenant: &str,
    containers: &[SourceContainer],
) -> Result<std::collections::BTreeSet<String>, String> {
    source_hierarchy::held_ids(tenant, containers).map_err(wire)
}

/// The node id of the container a record names, under the record's own connector.
pub fn record_container_node_id(
    tenant: &str,
    connector: &str,
    container_id: &str,
) -> Result<String, String> {
    source_hierarchy::container_node_id(tenant, connector, container_id).map_err(wire)
}

/// The `inContainer` edge of a record node to its container node.
pub fn in_container_edge(record_node_id: &str, container_node_id: &str) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: record_node_id.to_string(),
        target_id: container_node_id.to_string(),
        properties_msgpack: encode(&json!({ "relationship": IN_CONTAINER }))?,
    })
}

/// Admit one `containers` batch. `held` maps each node id [`held_ids`] named that the
/// engine already holds to its properties. A refusal admits nothing.
pub fn admit_containers(
    tenant: &str,
    containers: &[SourceContainer],
    held: &BTreeMap<String, Value>,
) -> Result<ContainerAdmission, String> {
    let plan = source_hierarchy::plan_containers(tenant, containers, held).map_err(wire)?;
    let mut admission = ContainerAdmission::default();
    for node in &plan.nodes {
        admission.methods.push(Method::AddNode {
            node_id: node.node_id.clone(),
            properties_msgpack: encode(&node.properties)?,
        });
        admission.node_ids.push(node.node_id.clone());
    }
    for (child, old_parent) in &plan.removed_edges {
        admission.methods.push(Method::RemoveEdge {
            source_id: child.clone(),
            target_id: old_parent.clone(),
        });
    }
    for (child, parent) in &plan.edges {
        admission.methods.push(Method::AddEdge {
            source_id: child.clone(),
            target_id: parent.clone(),
            properties_msgpack: encode(&json!({ "relationship": CONTAINED_IN }))?,
        });
    }
    Ok(admission)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(id: &str, parent: Option<&str>) -> SourceContainer {
        SourceContainer {
            source: "wiki".into(),
            container_id: id.into(),
            parent_id: parent.map(str::to_string),
            title: id.into(),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R904.3
    #[test]
    fn a_tree_compiles_to_nodes_then_edges() {
        let tree = [
            container("root", None),
            container("docs", Some("root")),
            container("api", Some("docs")),
        ];
        let admission = admit_containers("t1", &tree, &BTreeMap::new()).expect("admits");
        assert_eq!(admission.node_ids.len(), 3);
        let kinds: Vec<&str> = admission
            .methods
            .iter()
            .map(|m| match m {
                Method::AddNode { .. } => "node",
                Method::AddEdge { .. } => "edge",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["node", "node", "node", "edge", "edge"]);
    }

    // spec: MEDULLA-INGEST-WORKER-R904.3
    #[test]
    fn a_moved_container_drops_its_old_edge_and_an_unknown_parent_is_refused() {
        let ids = held_ids("t1", &[container("api", Some("docs"))]).expect("ids");
        let docs = source_hierarchy::container_node_id("t1", "wiki", "docs").unwrap();
        let api = source_hierarchy::container_node_id("t1", "wiki", "api").unwrap();
        let guide = source_hierarchy::container_node_id("t1", "wiki", "guide").unwrap();
        assert!(ids.contains(&docs) && ids.contains(&api));
        let refused = admit_containers("t1", &[container("api", Some("docs"))], &BTreeMap::new())
            .expect_err("docs is neither in the batch nor held");
        assert!(refused.starts_with("SOURCE_CONTAINER_INVALID"), "{refused}");
        let held = BTreeMap::from([
            (docs, json!({})),
            (guide, json!({})),
            (api, json!({ "parent_id": "guide" })),
        ]);
        let admission =
            admit_containers("t1", &[container("api", Some("docs"))], &held).expect("admits");
        assert!(admission
            .methods
            .iter()
            .any(|m| matches!(m, Method::RemoveEdge { .. })));
    }
}
