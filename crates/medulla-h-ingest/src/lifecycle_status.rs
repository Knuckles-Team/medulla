//! The graph read of a served `SpecStatus` (MEDULLA-SDLC-R005.3): the `Requirement` rows of one
//! repository, its default-branch `Changeset` nodes with their `citesRequirement` edges, and
//! its `boundTest` edges to test files, all through the visibility check of the caller. The
//! derivation itself is `medulla_core::lifecycle::status::answer_spec_status`.

use medulla_core::lifecycle::derive::DerivedEdge;
use medulla_core::lifecycle::derive::TestBinding;
use medulla_core::lifecycle::status::{SpecStatusView, StatusChangeset};
use medulla_core::lifecycle::store::LifecycleStore;
use medulla_graph_core::graph::GraphCore;
use medulla_source_types::lifecycle::{LifecycleEdgeKind, LifecycleKind};
use serde_json::Value;

use crate::source_ingestion::spec_projection::{changeset_node_id, requirement_node_id};

/// The most nodes of one type a status read takes.
pub const MAX_STATUS_NODES: usize = 200_000;

fn visible_node(
    core: &GraphCore,
    node_id: &str,
    blob: &[u8],
    tenant: &str,
    repo: &str,
    can_see: &dyn Fn(&[u8], bool) -> bool,
) -> Option<serde_json::Map<String, Value>> {
    if !can_see(blob, core.is_schema_node(node_id)) {
        return None;
    }
    let Ok(Value::Object(node)) = medulla_types::msgpack::decode_property_value(blob) else {
        return None;
    };
    // A record stamped for another tenant is absent to this one.
    if node
        .get("tenant_id")
        .and_then(Value::as_str)
        .is_some_and(|owner| owner != tenant)
    {
        return None;
    }
    if node.get("repo").and_then(Value::as_str) != Some(repo) {
        return None;
    }
    Some(node)
}

/// Read the status view of `repo` for `tenant`. `can_see` is the caller's node visibility
/// (`GraphReadAuthority::can_see_node`); a record it hides, or another tenant stamped, is not
/// in the view.
pub fn spec_status_view(
    core: &GraphCore,
    tenant: &str,
    repo: &str,
    can_see: &dyn Fn(&[u8], bool) -> bool,
) -> SpecStatusView {
    let mut view = SpecStatusView::default();
    for (node_id, blob) in core.get_nodes_by_label("Requirement", MAX_STATUS_NODES) {
        let Some(node) = visible_node(core, &node_id, &blob, tenant, repo, can_see) else {
            continue;
        };
        let Some(row_id) = node.get("rowId").and_then(Value::as_str) else {
            continue;
        };
        if node_id != requirement_node_id(repo, row_id) {
            continue;
        }
        view.rows.push(row_id.to_owned());
        for test_id in core.get_successors(&node_id).unwrap_or_default() {
            let bound = core
                .typed_edges_between(&node_id, &test_id)
                .iter()
                .any(|(relationship, _)| relationship == "boundTest");
            if !bound {
                continue;
            }
            let Some(test_blob) = core.get_node_properties(&test_id) else {
                continue;
            };
            let Some(test) = visible_node(core, &test_id, &test_blob, tenant, repo, can_see) else {
                continue;
            };
            if let Some(path) = test.get("path").and_then(Value::as_str) {
                view.bindings.push(TestBinding {
                    requirement: row_id.to_owned(),
                    test: path.to_owned(),
                });
            }
        }
    }
    let mut cites: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut changesets: Vec<(String, StatusChangeset)> = Vec::new();
    for (node_id, blob) in core.get_nodes_by_label("Changeset", MAX_STATUS_NODES) {
        let Some(node) = visible_node(core, &node_id, &blob, tenant, repo, can_see) else {
            continue;
        };
        let Some(id) = node.get("changeset_id").and_then(Value::as_str) else {
            continue;
        };
        // Only the projected `changeset:` nodes carry `citesRequirement` edges and
        // `Spec-Withdraw` ids; the `commit:` nodes of the repository index are the same
        // changesets and are read through them.
        if node_id != changeset_node_id(repo, id) {
            continue;
        }
        let list = |key: &str| -> Vec<String> {
            node.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .split(',')
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let created_at = node
            .get("created_at")
            .and_then(|value| {
                value
                    .as_str()
                    .and_then(|text| text.parse().ok())
                    .or(value.as_u64())
            })
            .unwrap_or(0);
        for requirement_id in core.get_successors(&node_id).unwrap_or_default() {
            let cites_it = core
                .typed_edges_between(&node_id, &requirement_id)
                .iter()
                .any(|(relationship, _)| relationship == "citesRequirement");
            if cites_it {
                cites
                    .entry(node_id.clone())
                    .or_default()
                    .push(requirement_id);
            }
        }
        changesets.push((
            node_id.clone(),
            StatusChangeset {
                id: id.to_owned(),
                created_at,
                cites: Vec::new(),
                withdraws: list("withdraws"),
            },
        ));
    }
    let row_of: std::collections::HashMap<String, String> = view
        .rows
        .iter()
        .map(|row| (requirement_node_id(repo, row), row.clone()))
        .collect();
    for (node_id, mut changeset) in changesets {
        changeset.cites = cites
            .remove(&node_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|requirement_id| row_of.get(requirement_id).cloned())
            .collect();
        view.changesets.push(changeset);
    }
    view
}

fn lifecycle_edge_kind(relationship: &str) -> Option<LifecycleEdgeKind> {
    LifecycleEdgeKind::ALL
        .into_iter()
        .find(|kind| kind.name() == relationship)
}

/// The lifecycle node `id` stands for, when the caller may see it and it belongs to `tenant`.
fn lifecycle_node(
    core: &GraphCore,
    id: &str,
    tenant: &str,
    can_see: &dyn Fn(&[u8], bool) -> bool,
) -> Option<LifecycleKind> {
    let blob = core.get_node_properties(id)?;
    if !can_see(&blob, core.is_schema_node(id)) {
        return None;
    }
    let Ok(Value::Object(node)) = medulla_types::msgpack::decode_property_value(&blob) else {
        return None;
    };
    if node
        .get("tenant_id")
        .and_then(Value::as_str)
        .is_some_and(|owner| owner != tenant)
    {
        return None;
    }
    let name = node
        .get("type")
        .or_else(|| node.get("kind"))
        .and_then(Value::as_str)?;
    LifecycleKind::from_node_type(name)
}

/// The lifecycle store around `anchor` for a served `LifecycleTrace` (MEDULLA-SDLC-R006.3): the
/// nodes and derived edges the caller can see within `depth` levels of the anchor, at most
/// `node_budget + 1` nodes, so the walk can tell a truncated answer from a complete one. An
/// anchor the caller cannot see, or another tenant stamped, yields an empty store, which the
/// trace answers with no records (no existence oracle).
pub fn lifecycle_store_around(
    core: &GraphCore,
    tenant: &str,
    anchor: &str,
    depth: u8,
    node_budget: usize,
    can_see: &dyn Fn(&[u8], bool) -> bool,
) -> LifecycleStore {
    let mut store = LifecycleStore::default();
    let Some(kind) = lifecycle_node(core, anchor, tenant, can_see) else {
        return store;
    };
    store.add_node(anchor, kind);
    let mut seen: std::collections::BTreeSet<String> = [anchor.to_owned()].into();
    let mut queue = std::collections::VecDeque::from([(anchor.to_owned(), 0u8)]);
    let mut edges: Vec<DerivedEdge> = Vec::new();
    while let Some((id, at_depth)) = queue.pop_front() {
        let mut neighbours: Vec<(String, bool)> = core
            .get_successors(&id)
            .unwrap_or_default()
            .into_iter()
            .map(|other| (other, true))
            .collect();
        neighbours.extend(
            core.get_predecessors(&id)
                .unwrap_or_default()
                .into_iter()
                .map(|other| (other, false)),
        );
        for (other, outgoing) in neighbours {
            let (from, to) = if outgoing {
                (id.as_str(), other.as_str())
            } else {
                (other.as_str(), id.as_str())
            };
            let found: Vec<DerivedEdge> = core
                .typed_edges_between(from, to)
                .into_iter()
                .filter_map(|(relationship, blob)| {
                    let kind = lifecycle_edge_kind(&relationship)?;
                    let properties = medulla_types::msgpack::decode_property_value(&blob).ok()?;
                    let text = |key: &str| {
                        properties
                            .get(key)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    };
                    Some(DerivedEdge {
                        kind,
                        from: from.to_owned(),
                        to: to.to_owned(),
                        rule: text("rule"),
                        rule_version: text("rule_version").parse().unwrap_or(0),
                        input_digests: text("input_digests")
                            .split(',')
                            .filter(|part| !part.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    })
                })
                .collect();
            if found.is_empty() {
                continue;
            }
            // A node at the depth bound adds edges among the nodes already reached, never new
            // nodes.
            if !seen.contains(&other) {
                if at_depth >= depth || seen.len() > node_budget {
                    continue;
                }
                let Some(kind) = lifecycle_node(core, &other, tenant, can_see) else {
                    continue;
                };
                seen.insert(other.clone());
                store.add_node(&other, kind);
                queue.push_back((other.clone(), at_depth + 1));
            }
            edges.extend(found);
        }
    }
    // Every endpoint was added before its edge, so the batch cannot refuse.
    let _ = store.apply_derived(edges);
    store
}
