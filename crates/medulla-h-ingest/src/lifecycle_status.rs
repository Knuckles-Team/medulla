//! The graph read of a served `SpecStatus` (MEDULLA-SDLC-R005.3): the `Requirement` rows of one
//! repository, its default-branch `Changeset` nodes with their `citesRequirement` edges, and
//! its `boundTest` edges to test files, all through the visibility check of the caller. The
//! derivation itself is `medulla_core::lifecycle::status::answer_spec_status`.

use medulla_core::lifecycle::derive::TestBinding;
use medulla_core::lifecycle::status::{SpecStatusView, StatusChangeset};
use medulla_graph_core::graph::GraphCore;
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
            .and_then(|value| value.as_str().and_then(|text| text.parse().ok()).or(value.as_u64()))
            .unwrap_or(0);
        for requirement_id in core.get_successors(&node_id).unwrap_or_default() {
            let cites_it = core
                .typed_edges_between(&node_id, &requirement_id)
                .iter()
                .any(|(relationship, _)| relationship == "citesRequirement");
            if cites_it {
                cites.entry(node_id.clone()).or_default().push(requirement_id);
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
