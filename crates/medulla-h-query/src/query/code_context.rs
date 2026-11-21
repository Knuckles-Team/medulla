//! `Method::CodeContext` and `Method::Diagnose` (MEDULLA-FEDERATED-QUERY-R654.3):
//! code context and symptom diagnosis over the caller's row-projected core.
//!
//! The handlers only read the projected core into the plain index records of
//! `medulla_plan::code_context` and call its one `code_context` / `diagnose`
//! implementation. A symbol is a node the repository index wrote with a
//! `symbol_type`; a call is a `calls` edge between two symbols; a repository whose
//! every `Branch` node is no longer `live` is reported as stale. An operations node
//! is a node whose `type` is one of the stage labels of `OPS_STAGE_BY_LABEL`; a
//! `dependsOn` edge from `a` to `b` reads as the link `b -> a`, and a `changes` or
//! `deployedTo` edge is a change link from its source to its target. Rows the
//! caller cannot read are not in the projected core, so neither read reaches them.
//! Nothing here writes.

use super::*;
use medulla_plan::code_context::{
    code_context, diagnose, CodeContextError, CodeEdge, CodeIndex, OpsLink, OpsLinkKind, OpsNode,
    OPS_STAGE_BY_LABEL,
};
use medulla_ingestion_types::code_health::{
    sweep, CodeGraph, CodeHealthReport, CodeHealthSweepRequest,
};
use medulla_protocol_types::result_contract::query::{CodeContext, CodeHealthSweep, Diagnose};
use medulla_types::code_context::{CodeContextRequest, DiagnoseRequest};
use std::collections::BTreeSet;

pub(super) type Props = serde_json::Map<String, serde_json::Value>;

pub(super) fn props_of(core: &GraphCore, id: &str) -> Option<Props> {
    let blob = core.get_node_properties(id)?;
    Some(medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default())
}

pub(super) fn text(props: &Props, key: &str) -> String {
    props
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn label_of(props: &Props) -> String {
    match text(props, "type") {
        t if t.is_empty() => text(props, "node_type"),
        t => t,
    }
}

/// `(target, edge properties)` of every out-edge of `id`.
pub(super) fn out_edges(core: &GraphCore, id: &str) -> Vec<(String, Props)> {
    let mut found = Vec::new();
    for (from, neighbours) in core.get_neighbors_batch(vec![id.to_string()]) {
        for to in neighbours {
            for blob in core.get_edge_properties(&from, &to) {
                let props =
                    medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default();
                found.push((to.clone(), props));
            }
        }
    }
    found
}

pub(super) fn relationship(props: &Props) -> String {
    ["relationship", "type", "edge_type"]
        .iter()
        .map(|key| text(props, key))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn qualified(props: &Props, id: &str) -> String {
    match text(props, "qualified_name") {
        name if name.is_empty() => id.to_string(),
        name => name,
    }
}

/// The symbol index of every repository in the caller's projected core.
fn code_index(core: &GraphCore) -> CodeIndex {
    let mut ids = core.node_ids();
    ids.sort();
    let mut names: std::collections::BTreeMap<String, String> = Default::default();
    let mut live: BTreeSet<String> = BTreeSet::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    for id in &ids {
        let Some(props) = props_of(core, id) else {
            continue;
        };
        if !text(&props, "symbol_type").is_empty() {
            names.insert(id.clone(), qualified(&props, id));
        } else if label_of(&props) == "Branch" {
            let repository = text(&props, "repository_id");
            if text(&props, "status") == "live" {
                live.insert(repository.clone());
            }
            known.insert(repository);
        }
    }
    let mut edges = Vec::new();
    for (id, caller) in &names {
        for (target, props) in out_edges(core, id) {
            if relationship(&props) == "calls" {
                if let Some(callee) = names.get(&target) {
                    edges.push(CodeEdge::new(caller.clone(), callee.clone()));
                }
            }
        }
    }
    CodeIndex {
        symbols: names.into_values().collect(),
        edges,
        missing_repositories: known.difference(&live).cloned().collect(),
    }
}

/// The operations nodes and links of the caller's projected core.
fn ops_graph(core: &GraphCore) -> (Vec<OpsNode>, Vec<OpsLink>) {
    let mut ids = core.node_ids();
    ids.sort();
    let mut nodes = Vec::new();
    for id in &ids {
        let Some(props) = props_of(core, id) else {
            continue;
        };
        let label = label_of(&props);
        if OPS_STAGE_BY_LABEL.iter().any(|(name, _)| *name == label) {
            let at_ms = props
                .get("at_ms")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            nodes.push(OpsNode {
                id: id.clone(),
                label,
                at_ms,
            });
        }
    }
    let known: BTreeSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let mut links = Vec::new();
    for node in &nodes {
        for (target, props) in out_edges(core, &node.id) {
            if !known.contains(target.as_str()) {
                continue;
            }
            let evidence_ids = match props
                .get("evidence_ids")
                .and_then(serde_json::Value::as_array)
            {
                Some(items) => items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect(),
                None => Some(text(&props, "evidence_id"))
                    .filter(|id| !id.is_empty())
                    .into_iter()
                    .collect(),
            };
            match relationship(&props).as_str() {
                "dependsOn" => links.push(OpsLink {
                    from: target,
                    to: node.id.clone(),
                    kind: OpsLinkKind::DependsOn,
                    evidence_ids,
                }),
                "changes" | "deployedTo" => links.push(OpsLink {
                    from: node.id.clone(),
                    to: target,
                    kind: OpsLinkKind::Change,
                    evidence_ids,
                }),
                _ => {}
            }
        }
    }
    (nodes, links)
}

fn refusal(error: CodeContextError) -> String {
    error.to_string()
}

/// Serve one `CodeContext`.
pub(crate) async fn handle_code_context(
    ctx: &QueryHandlerCtx<'_>,
    mut request: CodeContextRequest,
) -> Response {
    let req_id = ctx.req_id;
    let knobs = medulla_server_api::knob_config::knobs();
    request.depth = request
        .depth
        .or_else(|| u32::try_from(knobs.code_context_depth).ok());
    request.limit = request
        .limit
        .or_else(|| u32::try_from(knobs.code_max_nodes).ok());
    let core = Arc::clone(ctx.core);
    let outcome =
        compute_off_lock(req_id, move || code_context(&code_index(&core), &request)).await;
    match outcome {
        Err(response) => response,
        Ok(Err(error)) => Response::err(req_id, refusal(error)),
        Ok(Ok(answer)) => served_response::<CodeContext>(
            req_id,
            Ok(Ok(answer)),
            "CodeContext",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}

/// Serve one `CodeHealthSweep` (MEDULLA-REPO-INGEST-R700.3): the symbols the declared entry
/// points do not reach, over the same code index `CodeContext` reads. Rows the caller cannot
/// read are not in the projected core, so the sweep never reports them. Nothing is stored.
pub(crate) async fn handle_code_health_sweep(
    ctx: &QueryHandlerCtx<'_>,
    request: CodeHealthSweepRequest,
) -> Response {
    let req_id = ctx.req_id;
    let knobs = medulla_server_api::knob_config::knobs();
    let budget = match request.step_budget {
        Some(budget) if !(10_000..=1_000_000_000).contains(&budget) => {
            return Response::err(
                req_id,
                "INVALID_ARGUMENT: step_budget must be within [10000, 1000000000]",
            );
        }
        Some(budget) => budget,
        None => knobs.code_health_step_budget,
    };
    let core = Arc::clone(ctx.core);
    let outcome = compute_off_lock(req_id, move || {
        let index = code_index(&core);
        let symbols: BTreeSet<String> = index.symbols.iter().cloned().collect();
        let mut references: std::collections::BTreeMap<String, BTreeSet<String>> =
            Default::default();
        for edge in &index.edges {
            references
                .entry(edge.caller.clone())
                .or_default()
                .insert(edge.callee.clone());
        }
        // An entry point that is not a symbol of the index is dropped.
        let entry_points = request
            .entry_points
            .iter()
            .filter(|name| symbols.contains(name.as_str()))
            .cloned()
            .collect();
        let graph = CodeGraph {
            repo: request.repo.clone(),
            symbols,
            references,
            entry_points,
        };
        CodeHealthReport {
            repo: request.repo,
            findings: sweep(&graph, budget),
        }
    })
    .await;
    match outcome {
        Err(response) => response,
        Ok(report) => served_response::<CodeHealthSweep>(
            req_id,
            Ok(Ok(report)),
            "CodeHealthSweep",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}

/// Serve one `Diagnose`.
pub(crate) async fn handle_diagnose(
    ctx: &QueryHandlerCtx<'_>,
    request: DiagnoseRequest,
) -> Response {
    let req_id = ctx.req_id;
    let core = Arc::clone(ctx.core);
    let outcome = compute_off_lock(req_id, move || {
        let (nodes, links) = ops_graph(&core);
        diagnose(&nodes, &links, &request.symptom)
    })
    .await;
    match outcome {
        Err(response) => response,
        Ok(Err(error)) => Response::err(req_id, refusal(error)),
        Ok(Ok(diagnosis)) => served_response::<Diagnose>(
            req_id,
            Ok(Ok(diagnosis)),
            "Diagnose",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}
