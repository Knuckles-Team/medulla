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
use medulla_ingestion_types::code_health::{
    sweep, CodeGraph, CodeHealthReport, CodeHealthSweepRequest,
};
use medulla_plan::code_context::{
    code_context, diagnose, CodeContextError, CodeEdge, CodeIndex, OpsLink, OpsLinkKind, OpsNode,
    OPS_STAGE_BY_LABEL,
};
use medulla_protocol_types::result_contract::query::{
    CodeContext, CodeHealthSweep, CodeNavigation, Diagnose,
};
use medulla_types::code_context::{
    CodeContextRequest, CodeLocation, CodeNavAction, CodeNavigationAnswer, CodeNavigationRequest,
    DiagnoseRequest,
};
use std::collections::BTreeSet;

pub type Props = serde_json::Map<String, serde_json::Value>;

pub fn props_of(core: &GraphCore, id: &str) -> Option<Props> {
    let blob = core.get_node_properties(id)?;
    Some(medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default())
}

pub fn text(props: &Props, key: &str) -> String {
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
pub fn out_edges(core: &GraphCore, id: &str) -> Vec<(String, Props)> {
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

pub fn relationship(props: &Props) -> String {
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

/// One symbol of the code index with where the index recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeSite {
    id: String,
    location: CodeLocation,
}

/// A numeric property, stored as a number or as its text.
fn line_of(props: &Props, key: &str) -> u32 {
    match props.get(key) {
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        Some(serde_json::Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

/// The symbols of the caller's projected core with their definition sites, and the `calls`
/// edges between them as `(caller id, callee id)`. A symbol's file is the file version whose
/// blob `IMPLEMENTS` it; a symbol no file version reaches falls back to its own `path` or
/// `file_path` property, else an empty path.
fn code_sites(core: &GraphCore) -> (Vec<CodeSite>, Vec<(String, String)>) {
    let mut ids = core.node_ids();
    ids.sort();
    let mut file_of: std::collections::BTreeMap<String, String> = Default::default();
    let mut symbols: std::collections::BTreeMap<String, Props> = Default::default();
    for id in &ids {
        let Some(props) = props_of(core, id) else {
            continue;
        };
        if !text(&props, "symbol_type").is_empty() {
            symbols.insert(id.clone(), props);
        } else if !text(&props, "path").is_empty() {
            let path = text(&props, "path");
            for (blob, edge) in out_edges(core, id) {
                if relationship(&edge) != "hasBlob" {
                    continue;
                }
                for (symbol, edge) in out_edges(core, &blob) {
                    if relationship(&edge) == "IMPLEMENTS" {
                        file_of.entry(symbol).or_insert_with(|| path.clone());
                    }
                }
            }
        }
    }
    let mut sites = Vec::new();
    let mut calls = Vec::new();
    for (id, props) in &symbols {
        let path = file_of.get(id).cloned().unwrap_or_else(|| {
            [text(props, "path"), text(props, "file_path")]
                .into_iter()
                .find(|path| !path.is_empty())
                .unwrap_or_default()
        });
        sites.push(CodeSite {
            id: id.clone(),
            location: CodeLocation {
                symbol: qualified(props, id),
                path,
                line: line_of(props, "line"),
                kind: text(props, "symbol_type"),
            },
        });
        for (target, edge) in out_edges(core, id) {
            if relationship(&edge) == "calls" && symbols.contains_key(&target) {
                calls.push((id.clone(), target));
            }
        }
    }
    (sites, calls)
}

/// Whether `name` names `symbol`: the whole qualified name, or a suffix that starts at a
/// qualifier boundary.
fn names_symbol(symbol: &str, name: &str) -> bool {
    match symbol.strip_suffix(name) {
        Some("") => true,
        Some(head) => head.ends_with([':', '.', '/', '#']),
        None => false,
    }
}

/// The locations a `CodeNavigation` request answers over one code index, at most `limit`, in
/// symbol, path, line order. An unknown symbol answers none: the answer never tells a symbol
/// that is absent from one that the caller cannot read.
fn navigate(
    sites: &[CodeSite],
    calls: &[(String, String)],
    request: &CodeNavigationRequest,
    limit: usize,
) -> Result<Vec<CodeLocation>, String> {
    let symbol = request.symbol.as_deref().map(str::trim);
    if symbol == Some("") {
        return Err("INVALID_ARGUMENT: a code navigation symbol must not be empty".into());
    }
    let mut found: Vec<CodeLocation> = match (request.action, symbol) {
        (CodeNavAction::Instances, None) => {
            return Err("INVALID_ARGUMENT: instances needs a symbol".into());
        }
        (CodeNavAction::Nav, None) => sites.iter().map(|site| site.location.clone()).collect(),
        (CodeNavAction::Nav, Some(name)) => sites
            .iter()
            .filter(|site| names_symbol(&site.location.symbol, name))
            .map(|site| site.location.clone())
            .collect(),
        (CodeNavAction::Instances, Some(name)) => {
            let targets: BTreeSet<&str> = sites
                .iter()
                .filter(|site| names_symbol(&site.location.symbol, name))
                .map(|site| site.id.as_str())
                .collect();
            let callers: BTreeSet<&str> = calls
                .iter()
                .filter(|(_, callee)| targets.contains(callee.as_str()))
                .map(|(caller, _)| caller.as_str())
                .collect();
            sites
                .iter()
                .filter(|site| callers.contains(site.id.as_str()))
                .map(|site| site.location.clone())
                .collect()
        }
    };
    found.sort_by(|a, b| (&a.symbol, &a.path, a.line).cmp(&(&b.symbol, &b.path, b.line)));
    found.dedup();
    found.truncate(limit);
    Ok(found)
}

/// Serve one `CodeNavigation` (MEDULLA-FEDERATED-QUERY-R654.8) over the code index
/// `CodeContext` reads. Rows the caller cannot read are not in the projected core, so the
/// answer never reaches them. Nothing is stored.
pub async fn handle_code_navigation(
    ctx: &QueryHandlerCtx<'_>,
    request: CodeNavigationRequest,
) -> Response {
    let req_id = ctx.req_id;
    let cap = medulla_server_api::knob_config::knobs().code_max_nodes;
    let limit = match request.limit {
        Some(limit) if limit == 0 || u64::from(limit) > cap => {
            return Response::err(
                req_id,
                format!("INVALID_ARGUMENT: limit must be within [1, {cap}]"),
            );
        }
        Some(limit) => limit as usize,
        None => usize::try_from(cap).unwrap_or(usize::MAX),
    };
    let core = Arc::clone(ctx.core);
    let outcome = compute_off_lock(req_id, move || {
        let (sites, calls) = code_sites(&core);
        navigate(&sites, &calls, &request, limit)
    })
    .await;
    match outcome {
        Err(response) => response,
        Ok(Err(error)) => Response::err(req_id, error),
        Ok(Ok(items)) => served_response::<CodeNavigation>(
            req_id,
            Ok(Ok(CodeNavigationAnswer { items })),
            "CodeNavigation",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
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
pub async fn handle_code_context(
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
pub async fn handle_code_health_sweep(
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
pub async fn handle_diagnose(ctx: &QueryHandlerCtx<'_>, request: DiagnoseRequest) -> Response {
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

#[cfg(test)]
mod navigation_tests {
    use super::*;

    fn site(id: &str, symbol: &str, path: &str, line: u32) -> CodeSite {
        CodeSite {
            id: id.into(),
            location: CodeLocation {
                symbol: symbol.into(),
                path: path.into(),
                line,
                kind: "function".into(),
            },
        }
    }

    fn request(action: CodeNavAction, symbol: Option<&str>) -> CodeNavigationRequest {
        CodeNavigationRequest {
            action,
            symbol: symbol.map(str::to_string),
            limit: None,
        }
    }

    fn index() -> (Vec<CodeSite>, Vec<(String, String)>) {
        let sites = vec![
            site("run", "repo::a::run", "a.rs", 3),
            site("main", "repo::a::main", "a.rs", 9),
            site("parse", "repo::b::parse", "b.rs", 1),
            site("xparse", "repo::b::xparse", "b.rs", 20),
        ];
        let calls = vec![
            ("run".to_string(), "main".to_string()),
            ("main".to_string(), "parse".to_string()),
            ("run".to_string(), "parse".to_string()),
        ];
        (sites, calls)
    }

    // spec: MEDULLA-FEDERATED-QUERY-R654.8
    #[test]
    fn nav_answers_the_definition_of_a_suffix_at_a_qualifier_boundary() {
        let (sites, calls) = index();
        let items = navigate(
            &sites,
            &calls,
            &request(CodeNavAction::Nav, Some("parse")),
            10,
        )
        .unwrap();
        assert_eq!(
            items,
            vec![CodeLocation {
                symbol: "repo::b::parse".into(),
                path: "b.rs".into(),
                line: 1,
                kind: "function".into()
            }]
        );
        let all = navigate(&sites, &calls, &request(CodeNavAction::Nav, None), 3).unwrap();
        assert_eq!(all.len(), 3, "the limit caps the listing");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R654.8
    #[test]
    fn instances_answer_the_callers_and_an_unknown_symbol_answers_nothing() {
        let (sites, calls) = index();
        let items = navigate(
            &sites,
            &calls,
            &request(CodeNavAction::Instances, Some("repo::b::parse")),
            10,
        )
        .unwrap();
        let names: Vec<&str> = items.iter().map(|item| item.symbol.as_str()).collect();
        assert_eq!(names, ["repo::a::main", "repo::a::run"]);
        let none = navigate(
            &sites,
            &calls,
            &request(CodeNavAction::Instances, Some("nope")),
            10,
        )
        .unwrap();
        assert!(none.is_empty());
        assert!(navigate(&sites, &calls, &request(CodeNavAction::Instances, None), 10).is_err());
        assert!(navigate(&sites, &calls, &request(CodeNavAction::Nav, Some(" ")), 10).is_err());
    }
}
