//! Fail closed when the behavior-preserving dispatch module cut drifts.
//!
//! The dispatch subsystem is cut over four crates (router, graph, consensus, identity). This gate reads each
//! compiler-declared dispatch family (nothing is pinned by file list; orphan files fail the walk,
//! and a `#[path]` escape out of a crate's `src/dispatch` splits the cut and fails) and asserts:
//! the retired one-arm wrappers stay deleted, every route function is owned by exactly the module
//! that owns it and is still called from its router, the graph gateway/terminal and post-lock route
//! orders hold, the legacy write coalescer stays unreachable, and the native consensus route stays
//! exhaustive with no fail-open catch-all.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::analysis::mask::{code_mask, comments_mask};
use crate::analysis::module_tree;

pub const NAME: &str = "dispatch-decomposition";
pub const SUMMARY: &str =
    "the dispatch module cut, route ownership and route order have not drifted";

const REDUNDANT_WRAPPERS: [&str; 60] = [
    "authorize_and_route_knowledge_stream",
    "authorize_and_route_served_modality",
    "dispatch_analytics_job",
    "dispatch_apply_change_envelope",
    "dispatch_apply_change_envelopes",
    "dispatch_apply_multisig_mutation",
    "dispatch_begin_txn",
    "dispatch_blob_begin",
    "dispatch_cdc_read",
    "dispatch_cep_subscribe",
    "dispatch_change_env_apply_change_envelope",
    "dispatch_change_env_apply_change_envelopes",
    "dispatch_change_env_get_change_cursor",
    "dispatch_change_env_get_change_envelope",
    "dispatch_change_env_get_content_version",
    "dispatch_close_channel",
    "dispatch_cluster_members",
    "dispatch_create_channel",
    "dispatch_create_graph",
    "dispatch_delete_graph",
    "dispatch_get_change_cursor",
    "dispatch_get_change_envelope",
    "dispatch_get_channel_members",
    "dispatch_get_channel_messages",
    "dispatch_get_content_version",
    "dispatch_get_identity_from_store",
    "dispatch_health",
    "dispatch_import_sqlite_file",
    "dispatch_index_repository",
    "dispatch_join_channel",
    "dispatch_kv_get",
    "dispatch_leave_channel",
    "dispatch_list_channels",
    "dispatch_list_graphs",
    "dispatch_multi_graph_batch_update",
    "dispatch_nl_query",
    "dispatch_observe_screen",
    "dispatch_op_audit_prove_inclusion",
    "dispatch_op_served_modality",
    "dispatch_owl_reason_distributed",
    "dispatch_parse_file",
    "dispatch_parse_files",
    "dispatch_placement_route",
    "dispatch_policy_export",
    "dispatch_raft_add_learner",
    "dispatch_rbac_admin",
    "dispatch_register_identity",
    "dispatch_register_server",
    "dispatch_reshard",
    "dispatch_resource_stats_page",
    "dispatch_send_message",
    "dispatch_shutdown",
    "dispatch_statechart",
    "dispatch_txn_add_measurement",
    "dispatch_txn_axiom",
    "dispatch_txn_construct",
    "dispatch_txn_materialize_belief",
    "dispatch_txn_plan_writeback",
    "dispatch_unpaged_resource_stats",
    "dispatch_viz",
];

const ROUTE_OWNERS: [(&str, &str); 16] = [
    (
        "dispatch_service_control_methods",
        "crates/medulla-server-router/src/dispatch/router/service_control.rs",
    ),
    (
        "dispatch_source_ingest_methods",
        "crates/medulla-server-router/src/dispatch/router/source_ingest.rs",
    ),
    (
        "dispatch_resource_cost_methods",
        "crates/medulla-server-router/src/dispatch/router/resource_cost.rs",
    ),
    (
        "dispatch_graph_lifecycle_methods",
        "crates/medulla-server-router/src/dispatch/router/graph_lifecycle.rs",
    ),
    (
        "dispatch_channel_methods",
        "crates/medulla-server-router/src/dispatch/router/channels.rs",
    ),
    (
        "dispatch_identity_and_access_methods",
        "crates/medulla-server-router/src/dispatch/router/identity_access.rs",
    ),
    (
        "dispatch_decision_plane_methods",
        "crates/medulla-server-router/src/dispatch/router/decision_plane.rs",
    ),
    (
        "dispatch_decision_methods",
        "crates/medulla-server-router/src/dispatch/router/decision_plane.rs",
    ),
    (
        "dispatch_catalog_admin_methods",
        "crates/medulla-server-router/src/dispatch/router/decision_plane.rs",
    ),
    (
        "route_change_envelope_ops",
        "crates/medulla-server-graph/src/dispatch/change_envelope.rs",
    ),
    (
        "route_graph_op_method",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs",
    ),
    (
        "dispatch_op_workitem_claim_capability",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/work_governance.rs",
    ),
    (
        "dispatch_op_workitem_submission_or_resources",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/work_governance.rs",
    ),
    (
        "dispatch_rbac_elevation",
        "crates/medulla-server-identity/src/dispatch/elevation.rs",
    ),
    (
        "handle_policy_evolution",
        "crates/medulla-server-graph/src/dispatch/policy_evolution.rs",
    ),
    (
        "dispatch_telemetry_methods",
        "crates/medulla-server-router/src/dispatch/router/telemetry.rs",
    ),
];

const ROUTE_CALLERS: [(&str, &str); 16] = [
    (
        "dispatch_service_control_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_source_ingest_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_resource_cost_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_graph_lifecycle_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_channel_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_identity_and_access_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_decision_plane_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
    (
        "dispatch_decision_methods",
        "crates/medulla-server-router/src/dispatch/router/decision_plane.rs",
    ),
    (
        "dispatch_catalog_admin_methods",
        "crates/medulla-server-router/src/dispatch/router/decision_plane.rs",
    ),
    (
        "route_change_envelope_ops",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs",
    ),
    (
        "route_graph_op_method",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/graph_dispatch.rs",
    ),
    (
        "dispatch_op_workitem_claim_capability",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs",
    ),
    (
        "dispatch_op_workitem_submission_or_resources",
        "crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs",
    ),
    (
        "dispatch_rbac_elevation",
        "crates/medulla-server-router/src/dispatch/router/identity_access.rs",
    ),
    (
        "handle_policy_evolution",
        "crates/medulla-server-router/src/dispatch/router/control_plane.rs",
    ),
    (
        "dispatch_telemetry_methods",
        "crates/medulla-server-router/src/dispatch/router.rs",
    ),
];

const LEGACY_COALESCER_METHODS: [&str; 5] = [
    "AddNode",
    "RemoveNode",
    "AddEdge",
    "RemoveEdge",
    "CompareAndSetNodeFields",
];

const DISPATCH_ROOTS: [&str; 4] = [
    "crates/medulla-server-router/src/dispatch",
    "crates/medulla-server-graph/src/dispatch",
    "crates/medulla-server-consensus/src/dispatch",
    "crates/medulla-server-identity/src/dispatch",
];

const GRAPH_PIPELINE: &str = "crates/medulla-server-graph/src/dispatch/graph_pipeline/pipeline.rs";
const NATIVE_ROUTES: &str =
    "crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs";
const GATEWAY_GRAPH: &str = "crates/medulla-h-graph/src/graph_ops/gateway_graph.rs";
const MUTATION: &str = "crates/medulla-server-mutation/src/mutation.rs";
const CONSENSUS_ROUTING: &str = "crates/medulla-server-consensus/src/dispatch/consensus/routing.rs";

type Parts = BTreeMap<String, String>;

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    check(root).map_err(|message| format!("dispatch decomposition gate failed: {message}"))?;
    println!("dispatch decomposition gate: OK");
    Ok(())
}

fn check(root: &Path) -> Result<(), String> {
    let parts = sources(root)?;
    check_inventory(&parts)?;
    check_routing_and_coalescing(root, &parts)?;
    check_consensus_exhaustiveness(&parts)
}

/// The compiler-declared dispatch families, by repository path.
fn sources(root: &Path) -> Result<Parts, String> {
    let mut parts = Parts::new();
    for dispatch_root in DISPATCH_ROOTS {
        let family = module_tree::compiler_family(root, &format!("{dispatch_root}.rs"))?;
        let facade = format!("{dispatch_root}.rs");
        let escaped: Vec<&String> = family
            .files
            .keys()
            .filter(|path| **path != facade && !path.starts_with(&format!("{dispatch_root}/")))
            .collect();
        if !escaped.is_empty() {
            return Err(format!(
                "dispatch module declared outside {dispatch_root}: {escaped:?}"
            ));
        }
        parts.extend(family.files);
    }
    Ok(parts)
}

/// Named functions in Rust code, excluding comments and every literal kind.
fn function_names(source: &str) -> Result<Vec<String>, String> {
    let re = Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)")
        .expect("static regex");
    Ok(re
        .captures_iter(&code_mask(source)?)
        .map(|c| c[1].to_string())
        .collect())
}

fn part<'a>(parts: &'a Parts, path: &str) -> Result<&'a String, String> {
    parts
        .get(path)
        .ok_or_else(|| format!("dispatch source is absent from the declared families: {path}"))
}

fn check_route_calls(parts: &Parts) -> Result<(), String> {
    for (function, caller) in ROUTE_CALLERS {
        // A pinned caller that is no longer in the declared families is a stale pin: the
        // check runs against it and fails with the missing-call verdict instead of the
        // gate dying on the lookup.
        let Some(source) = parts.get(caller) else {
            return Err(format!("{function} call missing from {caller}"));
        };
        let code = code_mask(source)?;
        let call =
            Regex::new(&format!(r"\b{}\s*\(", regex::escape(function))).expect("escaped regex");
        if !call.is_match(&code) {
            return Err(format!("{function} call missing from {caller}"));
        }
    }
    Ok(())
}

fn check_inventory(parts: &Parts) -> Result<(), String> {
    let joined = code_mask(&parts.values().cloned().collect::<Vec<_>>().join("\n"))?;
    let names = function_names(&joined)?;
    let distinct: std::collections::BTreeSet<_> = REDUNDANT_WRAPPERS.iter().collect();
    if distinct.len() != 60 {
        return Err("wrapper deletion inventory is not exhaustive".into());
    }
    if REDUNDANT_WRAPPERS
        .iter()
        .any(|w| names.iter().any(|n| n == w))
    {
        return Err("redundant one-arm wrapper returned".into());
    }
    if joined.contains("classifier/handler diverged") {
        return Err("wrapper sentinel returned".into());
    }
    for (function, owner) in ROUTE_OWNERS {
        let mut owners = Vec::new();
        for (path, source) in parts {
            if function_names(source)?.iter().any(|n| n == function) {
                owners.push(path.as_str());
            }
        }
        if owners != [owner] {
            return Err(format!("{function} ownership changed: {owners:?}"));
        }
    }
    check_route_calls(parts)
}

fn family_source(parts: &Parts, root: &str) -> String {
    let prefix = root.strip_suffix(".rs").unwrap_or(root);
    parts
        .iter()
        .filter(|(path, _)| path.as_str() == root || path.starts_with(&format!("{prefix}/")))
        .map(|(_, source)| source.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The markers must appear in this order, by their LAST occurrence in the masked source.
fn require_order(source: &str, markers: &[&str], message: &str) -> Result<(), String> {
    let mut offsets = Vec::new();
    for marker in markers {
        offsets.push(
            source
                .rfind(marker)
                .ok_or_else(|| format!("{message}: marker absent: {marker}"))?,
        );
    }
    if offsets.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(message.to_string());
    }
    Ok(())
}

fn check_route_order(graph: &str) -> Result<(), String> {
    let graph = code_mask(graph)?;
    let message = "graph gateway/terminal route order changed";
    let gateway_order = [
        "medulla_h_graph::graph_ops::try_handle_gateway(",
        "medulla_h_compute::finance::try_handle(",
    ];
    let graph_order = [
        "medulla_h_compute::datascience::try_handle(",
        "medulla_h_mining::mining::try_handle(",
        "medulla_server_api::handlers::graphlearn::try_handle(",
        "medulla_h_graph::pipeline::try_handle(",
    ];
    let query_order = ["route_query_gateway(", "route_rdf_gateway("];
    let global_order = [
        "medulla_h_compute::wasm_udf::try_handle(",
        "medulla_h_sparql::foreign_source_handlers::try_handle(",
        "medulla_h_txn::dist_compute::try_handle(",
    ];
    for markers in [
        &gateway_order[..],
        &graph_order[..],
        &query_order[..],
        &global_order[..],
    ] {
        require_order(&graph, markers, message)?;
    }
    require_order(
        &graph,
        &[
            "route_pipeline_compute(&ctx, method)",
            "route_pipeline_surfaces(&ctx, method)",
            "medulla_h_graph::graph_ops::try_handle(",
        ],
        message,
    )
}

fn check_post_lock_route_order(native_routes: &str) -> Result<(), String> {
    require_order(
        &code_mask(native_routes)?,
        &[
            "route_change_envelope_ops(",
            "route_native_store_ops(",
            "route_graph_authority_surfaces(",
        ],
        "post-lock graph router order changed",
    )
}

fn check_legacy_coalescer(root: &Path, parts: &Parts) -> Result<(), String> {
    let graph_family = family_source(
        parts,
        "crates/medulla-server-graph/src/dispatch/graph_pipeline.rs",
    );
    if graph_family.contains("try_coalesce_write") {
        return Err("unreachable legacy coalescer fallback returned".into());
    }
    let gateway = match parts.get(GATEWAY_GRAPH) {
        Some(source) => source.clone(),
        None => module_tree::compiler_family(root, GATEWAY_GRAPH)?.production,
    };
    let gateway = code_mask(&gateway)?;
    let mutation = comments_mask(&module_tree::compiler_family(root, MUTATION)?.production)?;
    let routed = mutation
        .split_once("pub const GATEWAY_ROUTED: &[&str] = &[")
        .and_then(|(_, rest)| rest.split_once("\n];"))
        .map(|(body, _)| body)
        .ok_or("GATEWAY_ROUTED table is absent from the mutation module")?;
    for method in LEGACY_COALESCER_METHODS {
        if !gateway.contains(&format!("Method::{method}")) {
            return Err(format!("legacy coalescer proof lost gateway arm {method}"));
        }
        if !routed.contains(&format!("\"{method}\"")) {
            return Err(format!(
                "legacy coalescer method escaped GATEWAY_ROUTED: {method}"
            ));
        }
    }
    Ok(())
}

fn check_routing_and_coalescing(root: &Path, parts: &Parts) -> Result<(), String> {
    check_route_order(part(parts, GRAPH_PIPELINE)?)?;
    check_post_lock_route_order(part(parts, NATIVE_ROUTES)?)?;
    check_legacy_coalescer(root, parts)
}

fn check_consensus_exhaustiveness(parts: &Parts) -> Result<(), String> {
    let consensus = part(parts, CONSENSUS_ROUTING)?;
    let start = consensus
        .find("fn native_route_target(")
        .ok_or("fn native_route_target( is absent")?;
    let end = consensus[start..]
        .find("\n}\n")
        .ok_or("native_route_target has no closing brace")?
        + start
        + 2;
    let route = code_mask(&consensus[start..end])?;
    if route.contains("_ =>") {
        return Err("native consensus route regained a wildcard fallback".into());
    }
    let catch_all = Regex::new(r"Some\(\s*other\s*\)\s*=>\s*request_graph\.to_string\(\)")
        .expect("static regex");
    if catch_all.is_match(&route) {
        return Err("native consensus route regained a fail-open catch-all".into());
    }
    if parts.values().any(|source| source.contains("domain_of")) {
        return Err("a second dispatch domain registry appeared".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use crate::workspace_root;

    fn parts() -> Parts {
        sources(&workspace_root()).unwrap()
    }

    fn owner_of(function: &str) -> &'static str {
        ROUTE_OWNERS.iter().find(|(f, _)| *f == function).unwrap().1
    }

    /// Swap two call expressions in `source`.
    fn swapped(source: &str, a: &str, b: &str) -> String {
        assert!(
            source.contains(a) && source.contains(b),
            "fixture targets absent"
        );
        source
            .replacen(a, "__swap__", 1)
            .replacen(b, a, 1)
            .replacen("__swap__", b, 1)
    }

    #[test]
    fn the_gate_passes_on_the_current_tree() {
        // spec: MEDULLA-CONTRACT-R017
        check(&workspace_root()).unwrap();
    }

    #[test]
    fn elevation_route_ownership_drift_fails_closed() {
        let mut parts = parts();
        let owner = owner_of("dispatch_rbac_elevation");
        let text =
            parts[owner].replacen("fn dispatch_rbac_elevation", "fn removed_rbac_elevation", 1);
        parts.insert(owner.to_string(), text);
        // spec: MEDULLA-CONTRACT-R017
        assert!(check_inventory(&parts)
            .unwrap_err()
            .contains("dispatch_rbac_elevation ownership changed"));
    }

    #[test]
    fn policy_evolution_route_ownership_drift_fails_closed() {
        let mut parts = parts();
        let owner = owner_of("handle_policy_evolution");
        let text = parts[owner].replacen(
            "pub async fn handle_policy_evolution",
            "pub async fn removed_handle_policy_evolution",
            1,
        );
        parts.insert(owner.to_string(), text);
        // spec: MEDULLA-CONTRACT-R017
        assert!(check_inventory(&parts)
            .unwrap_err()
            .contains("handle_policy_evolution ownership changed"));
    }

    #[test]
    fn telemetry_route_call_removal_fails_closed() {
        let mut parts = parts();
        let target = "crates/medulla-server-router/src/dispatch/router.rs";
        let call = "Box::pin(dispatch_telemetry_methods(ctx, method))";
        assert!(parts[target].contains(call));
        let text = parts[target].replacen(
            call,
            "Box::pin(removed_dispatch_telemetry_methods(ctx, method))",
            1,
        );
        parts.insert(target.to_string(), text);
        // spec: MEDULLA-CONTRACT-R017
        assert!(check_inventory(&parts)
            .unwrap_err()
            .contains("dispatch_telemetry_methods call missing"));
    }

    #[test]
    fn a_stale_caller_pin_runs_the_check_instead_of_aborting() {
        let mut parts = parts();
        parts.remove("crates/medulla-server-router/src/dispatch/router.rs");
        // spec: MEDULLA-CONTRACT-R017
        let err = check_inventory(&parts).unwrap_err();
        assert!(
            err.contains(
                "dispatch_service_control_methods call missing from crates/medulla-server-router/src/dispatch/router.rs"
            ) && !err.contains("absent from the declared families")
        );
    }

    #[test]
    fn the_wrapper_inventory_is_exhaustive_and_unique() {
        assert_eq!(
            REDUNDANT_WRAPPERS
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            60
        );
    }

    #[test]
    fn route_ownership_drift_fails_closed() {
        let mut parts = parts();
        let owner = owner_of("dispatch_channel_methods");
        let text = parts[owner].replacen("dispatch_channel_methods", "removed_channel_methods", 1);
        parts.insert(owner.to_string(), text);
        assert!(check_inventory(&parts)
            .unwrap_err()
            .contains("dispatch_channel_methods ownership changed"));
    }

    #[test]
    fn route_call_removal_fails_closed() {
        let mut parts = parts();
        let target = "crates/medulla-server-router/src/dispatch/router.rs";
        let call = "Box::pin(dispatch_service_control_methods(ctx, method))";
        assert!(parts[target].contains(call));
        let text = parts[target].replacen(
            call,
            "Box::pin(removed_dispatch_service_control_methods(ctx, method))",
            1,
        );
        parts.insert(target.to_string(), text);
        assert!(check_inventory(&parts)
            .unwrap_err()
            .contains("dispatch_service_control_methods call missing"));
    }

    #[test]
    fn graph_router_call_removal_fails_closed() {
        let mut parts = parts();
        let target = "crates/medulla-server-graph/src/dispatch/graph_pipeline/graph_dispatch.rs";
        let text = parts[target].replacen(
            "route_graph_op_method(routing, method).await",
            "removed_route_graph_op_method(routing, method).await",
            1,
        );
        parts.insert(target.to_string(), text);
        assert!(check_inventory(&parts)
            .unwrap_err()
            .contains("route_graph_op_method call missing"));
    }

    #[test]
    fn legacy_coalescer_subset_proof_fails_closed() {
        let root = workspace_root();
        let mut parts = parts();
        let text = std::fs::read_to_string(root.join(GATEWAY_GRAPH))
            .unwrap()
            .replace("Method::AddNode", "Method::RemovedAddNode");
        parts.insert(GATEWAY_GRAPH.to_string(), text);
        assert!(check_routing_and_coalescing(&root, &parts)
            .unwrap_err()
            .contains("gateway arm AddNode"));
    }

    #[test]
    fn route_order_ignores_comments_and_detects_reversed_calls() {
        let root = workspace_root();
        let mut parts = parts();
        let compute = "route_pipeline_compute(&ctx, method)";
        let surfaces = "route_pipeline_surfaces(&ctx, method)";
        let text = swapped(&parts[GRAPH_PIPELINE], compute, surfaces)
            + &format!("\n// {compute} must precede {surfaces}\n");
        parts.insert(GRAPH_PIPELINE.to_string(), text);
        assert!(check_routing_and_coalescing(&root, &parts)
            .unwrap_err()
            .contains("graph gateway/terminal route order changed"));
    }

    #[test]
    fn post_lock_router_order_swap_fails_closed() {
        let root = workspace_root();
        let mut parts = parts();
        let text = swapped(
            &parts[NATIVE_ROUTES],
            "route_change_envelope_ops(ctx, method).await",
            "route_native_store_ops(ctx, method).await",
        );
        parts.insert(NATIVE_ROUTES.to_string(), text);
        assert!(check_routing_and_coalescing(&root, &parts)
            .unwrap_err()
            .contains("post-lock graph router order changed"));
    }

    #[test]
    fn consensus_fail_open_catch_all_fails_closed() {
        let mut parts = parts();
        let text = parts[CONSENSUS_ROUTING].replacen(
            "Some(other) => unreachable!(\"unclassified native consensus domain: {other}\")",
            "Some(other) => request_graph.to_string()",
            1,
        );
        parts.insert(CONSENSUS_ROUTING.to_string(), text);
        assert!(check_consensus_exhaustiveness(&parts)
            .unwrap_err()
            .contains("fail-open catch-all"));
    }

    #[test]
    fn an_undeclared_dispatch_module_is_an_orphan() {
        let tree = Tree::new(&[
            ("crates/medulla-server-router/src/dispatch.rs", "mod a;\n"),
            ("crates/medulla-server-router/src/dispatch/a.rs", ""),
            (
                "crates/medulla-server-router/src/dispatch/forgotten.rs",
                "fn dead() {}\n",
            ),
        ]);
        assert!(sources(tree.path())
            .unwrap_err()
            .contains("orphan Rust module files"));
    }

    #[test]
    fn a_path_escape_out_of_the_dispatch_root_is_rejected() {
        let tree = Tree::new(&[
            (
                "crates/medulla-server-router/src/dispatch.rs",
                "#[path = \"elsewhere.rs\"]\nmod a;\n",
            ),
            ("crates/medulla-server-router/src/elsewhere.rs", ""),
        ]);
        assert!(sources(tree.path())
            .unwrap_err()
            .contains("declared outside"));
    }
}
