//! Source container hierarchy (MEDULLA-INGEST-WORKER-R904.2).
//!
//! A connector's folders, spaces or projects arrive as `SourceContainer`s. They
//! are upserted as one `SourceContainer` node each, with one `containedIn` edge
//! from a container to its parent. `subtree` walks those edges in reverse to
//! resolve a scope.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use medulla_graph_core::graph::GraphCore;
use medulla_types::contract::Digest256;
use medulla_types::ingest_rules::{check_container_tree, MAX_CONTAINER_DEPTH};
use medulla_types::source_governance::SourceContainer;
use serde_json::{json, Value};

/// The digest domain a container node id is framed under.
pub const CONTAINER_NODE_DOMAIN: &[u8] = b"eg/source-container";
/// Prefix of every container node id.
pub const CONTAINER_NODE_PREFIX: &str = "source-container:";
/// The class label a container node carries.
pub const CONTAINER_CLASS: &str = "SourceContainer";
/// The edge from a container to its parent.
pub const CONTAINED_IN: &str = "containedIn";

/// A refusal from the hierarchy functions. A refusal writes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerError {
    /// The stable code.
    pub code: String,
    /// What was refused.
    pub detail: String,
}

impl std::fmt::Display for ContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ContainerError {}

fn refuse(code: &str, detail: impl Into<String>) -> ContainerError {
    ContainerError {
        code: code.to_string(),
        detail: detail.into(),
    }
}

/// What one `upsert_containers` call wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContainerUpsert {
    /// Container nodes written.
    pub nodes: u64,
    /// `containedIn` edges written.
    pub edges: u64,
}

/// One container of a subtree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtreeEntry {
    /// The container node id.
    pub node_id: String,
    /// The connector's container id.
    pub container_id: String,
    /// Distance from the root: 0 for the root.
    pub depth: usize,
}

/// The node id of a container: `source-container:` plus the hex of a framed hash over
/// tenant, connector and container id.
pub fn container_node_id(
    tenant: &str,
    source: &str,
    container_id: &str,
) -> Result<String, ContainerError> {
    let digest = Digest256::framed(
        CONTAINER_NODE_DOMAIN,
        &[
            tenant.as_bytes(),
            source.as_bytes(),
            container_id.as_bytes(),
        ],
    )
    .map_err(|e| refuse("CONTAINER_INVALID", e))?;
    Ok(format!("{CONTAINER_NODE_PREFIX}{}", digest.to_hex()))
}

fn blob(value: &Value) -> Vec<u8> {
    rmp_serde::to_vec_named(value).unwrap_or_default()
}

fn props(core: &GraphCore, node_id: &str) -> Option<Value> {
    let bytes = core.get_node_properties(node_id)?;
    rmp_serde::from_slice::<Value>(&bytes).ok()
}

/// One container node a plan writes.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedNode {
    /// The container node id.
    pub node_id: String,
    /// The node's properties.
    pub properties: Value,
}

/// Everything one container batch writes, in write order: nodes first, then the
/// `containedIn` edges a changed parent drops, then the `containedIn` edges to write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerPlan {
    /// Container nodes to upsert.
    pub nodes: Vec<PlannedNode>,
    /// `(child, old parent)` edges to remove.
    pub removed_edges: Vec<(String, String)>,
    /// `(child, parent)` edges to write.
    pub edges: Vec<(String, String)>,
}

/// Validate `containers`, refuse parent cycles and excess depth, and return the node id
/// of every container and of every parent they name. Those are the nodes a plan needs to
/// see the held state of: [`plan_containers`] takes them as its `held` map.
pub fn held_ids(
    tenant: &str,
    containers: &[SourceContainer],
) -> Result<BTreeSet<String>, ContainerError> {
    for container in containers {
        container
            .validate()
            .map_err(|e| refuse("CONTAINER_INVALID", e.to_string()))?;
    }
    check_container_tree(containers).map_err(|e| refuse(&e.code(), e.to_string()))?;
    let mut ids = BTreeSet::new();
    for container in containers {
        ids.insert(container_node_id(
            tenant,
            &container.source,
            &container.container_id,
        )?);
        if let Some(parent) = container.parent_id.as_deref() {
            ids.insert(container_node_id(tenant, &container.source, parent)?);
        }
    }
    Ok(ids)
}

/// Plan the writes of `containers` of `tenant`. `held` maps the node id of every node
/// [`held_ids`] names that already exists to its properties; a missing key is an absent
/// node. Nothing is written here: the caller applies the plan to a [`GraphCore`]
/// ([`upsert_containers`]) or compiles it into the methods of a served batch.
///
/// A parent outside the batch must be a held container node of the same source,
/// otherwise the batch is refused with `CONTAINER_PARENT_UNKNOWN`. A container whose
/// parent changed loses its old `containedIn` edge.
pub fn plan_containers(
    tenant: &str,
    containers: &[SourceContainer],
    held: &BTreeMap<String, Value>,
) -> Result<ContainerPlan, ContainerError> {
    held_ids(tenant, containers)?;
    let in_batch: BTreeSet<(&str, &str)> = containers
        .iter()
        .map(|c| (c.source.as_str(), c.container_id.as_str()))
        .collect();
    for container in containers {
        if let Some(parent) = container.parent_id.as_deref() {
            if !in_batch.contains(&(container.source.as_str(), parent))
                && !held.contains_key(&container_node_id(tenant, &container.source, parent)?)
            {
                return Err(refuse(
                    "CONTAINER_PARENT_UNKNOWN",
                    format!(
                        "container '{}' names parent '{parent}', which is neither in the batch nor held",
                        container.container_id
                    ),
                ));
            }
        }
    }

    let mut plan = ContainerPlan::default();
    for container in containers {
        let id = container_node_id(tenant, &container.source, &container.container_id)?;
        let old_parent = held
            .get(&id)
            .and_then(|p| p.get("parent_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        plan.nodes.push(PlannedNode {
            node_id: id.clone(),
            properties: json!({
                "type": CONTAINER_CLASS,
                "tenant": tenant,
                "source": container.source,
                "container_id": container.container_id,
                "parent_id": container.parent_id,
                "title": container.title,
            }),
        });
        if let Some(old) = old_parent {
            if container.parent_id.as_deref() != Some(old.as_str()) {
                let old_id = container_node_id(tenant, &container.source, &old)?;
                plan.removed_edges.push((id, old_id));
            }
        }
    }
    for container in containers {
        let Some(parent) = container.parent_id.as_deref() else {
            continue;
        };
        plan.edges.push((
            container_node_id(tenant, &container.source, &container.container_id)?,
            container_node_id(tenant, &container.source, parent)?,
        ));
    }
    Ok(plan)
}

/// Upsert `containers` of `tenant` into `core`.
///
/// Every container is validated and the batch is checked for parent cycles and depth
/// before anything is written ([`plan_containers`] owns those rules). Re-sending a
/// container writes the same node and edge again.
pub fn upsert_containers(
    core: &GraphCore,
    tenant: &str,
    containers: &[SourceContainer],
) -> Result<ContainerUpsert, ContainerError> {
    let mut held = BTreeMap::new();
    for id in held_ids(tenant, containers)? {
        if core.has_node(&id) {
            held.insert(id.clone(), props(core, &id).unwrap_or_else(|| json!({})));
        }
    }
    let plan = plan_containers(tenant, containers, &held)?;
    for node in &plan.nodes {
        core.add_node(node.node_id.clone(), blob(&node.properties));
    }
    for (child, old_parent) in &plan.removed_edges {
        core.remove_typed_edge(child.clone(), old_parent.clone(), CONTAINED_IN.to_string());
    }
    for (child, parent) in &plan.edges {
        core.add_typed_edge(
            child.clone(),
            parent.clone(),
            CONTAINED_IN.to_string(),
            blob(&json!({})),
        )
        .map_err(|e| refuse("CONTAINER_EDGE_REFUSED", e))?;
    }
    Ok(ContainerUpsert {
        nodes: plan.nodes.len() as u64,
        edges: plan.edges.len() as u64,
    })
}

/// The root container and its descendants, found by walking `containedIn` edges in
/// reverse breadth first. `max_depth` counts levels below the root and is capped at
/// [`MAX_CONTAINER_DEPTH`]. Ordered by depth, then container id.
pub fn subtree(
    core: &GraphCore,
    root: &str,
    max_depth: usize,
) -> Result<Vec<SubtreeEntry>, ContainerError> {
    let root_props = props(core, root)
        .filter(|p| p.get("type").and_then(Value::as_str) == Some(CONTAINER_CLASS));
    let Some(root_props) = root_props else {
        return Err(refuse(
            "CONTAINER_NOT_FOUND",
            format!("'{root}' is not a container node"),
        ));
    };
    let limit = max_depth.min(MAX_CONTAINER_DEPTH);
    let container_id = |p: &Value| {
        p.get("container_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut seen: BTreeSet<String> = BTreeSet::from([root.to_string()]);
    let mut out = vec![SubtreeEntry {
        node_id: root.to_string(),
        container_id: container_id(&root_props),
        depth: 0,
    }];
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(root.to_string(), 0)]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= limit {
            continue;
        }
        let children = core
            .get_predecessors(&node)
            .map_err(|e| refuse("CONTAINER_READ_FAILED", e))?;
        for child in children {
            if seen.contains(&child) {
                continue;
            }
            if !core
                .typed_edges_between(&child, &node)
                .iter()
                .any(|(relation, _)| relation == CONTAINED_IN)
            {
                continue;
            }
            let Some(child_props) = props(core, &child) else {
                continue;
            };
            seen.insert(child.clone());
            out.push(SubtreeEntry {
                node_id: child.clone(),
                container_id: container_id(&child_props),
                depth: depth + 1,
            });
            queue.push_back((child, depth + 1));
        }
    }
    out.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| a.container_id.cmp(&b.container_id))
    });
    Ok(out)
}
