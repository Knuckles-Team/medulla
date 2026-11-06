//! MEDULLA-REPO-INGEST-R601.3 / R602.3 — the commit-history and service-link
//! hooks of [`super::run_after_commit`].
//!
//! Both read the typed nodes the index envelope committed
//! ([`crate::repository_index::scope_facts`]), run the `medulla-enrich` pass
//! and lower its output as methods of the enrichment envelope.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_outcome_types::decision::ingestion_ladder::ProvenanceRung;
use medulla_enrich::history::{
    ChangeKind, CommitRecord, FileChange, HistoryConfig, HistoryGraph, HistoryRunReport,
};
use medulla_enrich::links::{
    link_pass, HttpRoute, IacResource, LinkRunReport, ServiceLink, ServiceOwnership,
};
use medulla_enrich::Id;
use medulla_protocol_types::protocol::Method;

use super::{add_edge, add_node, Row};
use crate::ingestion_rung::{rung_label, PRODUCING_RUNG};
use crate::repository_index::scope_facts::{
    COMMIT_TYPE, IAC_TYPE, ROUTE_TYPE, SERVICE_PREFIX_TYPE,
};

/// Node type of one commit applied to the history graph.
pub const COMMIT_HISTORY_TYPE: &str = "CommitHistory";
/// The enrichment node id of an applied commit.
pub const COMMIT_HISTORY_PREFIX: &str = "commit-history:";
/// The rung history facts are produced under.
const HISTORY_RUNG: ProvenanceRung = ProvenanceRung::StatisticalOrCommunity;

/// The typed nodes the index envelope committed for the two hooks.
#[derive(Default)]
pub struct HookInputs {
    pub commits: Vec<CommitRecord>,
    pub routes: Vec<HttpRoute>,
    pub resources: Vec<IacResource>,
    pub services: Vec<ServiceOwnership>,
}

fn commit_of(properties: &BTreeMap<String, String>, digest: &str) -> Option<CommitRecord> {
    let changes: Vec<FileChange> =
        serde_json::from_str::<Vec<serde_json::Value>>(properties.get("changes")?)
            .ok()?
            .into_iter()
            .map(|change| {
                Some(FileChange {
                    path: Id::new(change.get("path")?.as_str()?).ok()?,
                    kind: serde_json::from_value::<ChangeKind>(change.get("kind")?.clone()).ok()?,
                    added_lines: u32::try_from(change.get("added_lines")?.as_u64()?).ok()?,
                    removed_lines: u32::try_from(change.get("removed_lines")?.as_u64()?).ok()?,
                })
            })
            .collect::<Option<_>>()?;
    let parents = properties
        .get("parents")
        .map(|joined| joined.split(',').filter(|p| !p.is_empty()))
        .into_iter()
        .flatten()
        .map(str::to_string)
        .collect();
    let author = Id::new(properties.get("author")?.as_str())
        .or_else(|_| Id::new("unknown-author"))
        .ok()?;
    let timestamp_ms = properties.get("timestamp_ms")?.parse().ok()?;
    CommitRecord::new(digest, author, timestamp_ms, parents, changes).ok()
}

fn route_of(properties: &BTreeMap<String, String>, id: &str) -> Option<HttpRoute> {
    Some(HttpRoute {
        id: Id::new(id).ok()?,
        method: properties.get("method")?.clone(),
        path: properties.get("path")?.clone(),
        file: properties.get("file")?.clone(),
    })
}

fn resource_of(properties: &BTreeMap<String, String>, id: &str) -> Option<IacResource> {
    Some(IacResource {
        id: Id::new(id).ok()?,
        kind: properties.get("kind")?.clone(),
        file: properties.get("file")?.clone(),
    })
}

/// Read the hook inputs from the committed rows.
pub(super) fn hook_inputs(rows: &[Row]) -> HookInputs {
    let mut inputs = HookInputs::default();
    let mut prefixes: BTreeMap<Id, Vec<String>> = BTreeMap::new();
    for row in rows {
        let Row::Node { id, properties } = row else {
            continue;
        };
        match properties.get("type").map(String::as_str) {
            Some(COMMIT_TYPE) => inputs.commits.extend(
                id.strip_prefix("commit:")
                    .and_then(|digest| commit_of(properties, digest)),
            ),
            Some(ROUTE_TYPE) => inputs.routes.extend(route_of(properties, id)),
            Some(IAC_TYPE) => inputs.resources.extend(resource_of(properties, id)),
            Some(SERVICE_PREFIX_TYPE) => {
                let service = properties
                    .get("service")
                    .and_then(|s| Id::new(s.as_str()).ok());
                if let (Some(service), Some(prefix)) = (service, properties.get("path_prefix")) {
                    prefixes.entry(service).or_default().push(prefix.clone());
                }
            }
            _ => {}
        }
    }
    inputs.services = prefixes
        .into_iter()
        .map(|(service, path_prefixes)| ServiceOwnership {
            service,
            path_prefixes,
        })
        .collect();
    inputs
}

fn commit_methods(graph: &HistoryGraph, added: &[&CommitRecord]) -> Result<Vec<Method>, String> {
    let mut methods = Vec::new();
    for commit in added {
        methods.push(add_node(
            format!("{COMMIT_HISTORY_PREFIX}{}", commit.digest()),
            &[
                ("type", COMMIT_HISTORY_TYPE.to_string()),
                ("author", commit.author.as_str().to_string()),
                ("timestamp_ms", commit.timestamp_ms.to_string()),
                (PRODUCING_RUNG, rung_label(HISTORY_RUNG)),
            ],
        )?);
    }
    for (file, churn) in &graph.file_churn {
        methods.push(add_node(
            format!("history-file:{}", file.as_str()),
            &[
                ("type", "HistoryFile".to_string()),
                ("commits", churn.commits.to_string()),
                ("added_lines", churn.added_lines.to_string()),
                ("removed_lines", churn.removed_lines.to_string()),
            ],
        )?);
    }
    Ok(methods)
}

fn coupling_methods(graph: &HistoryGraph, config: &HistoryConfig) -> Result<Vec<Method>, String> {
    let mut methods = Vec::new();
    for edge in graph.coupling(config) {
        let (a, b) = (edge.a.as_str(), edge.b.as_str());
        let id = format!("history-coupling:{a}|{b}");
        methods.push(add_node(
            id.clone(),
            &[
                ("type", "HistoryCoupling".to_string()),
                ("support", edge.support.to_string()),
                ("confidence_milli", edge.confidence_milli.to_string()),
            ],
        )?);
        methods.push(add_edge(
            format!("history-file:{a}"),
            id.clone(),
            "coupledFrom",
        )?);
        methods.push(add_edge(id, format!("history-file:{b}"), "coupledTo")?);
    }
    Ok(methods)
}

/// R601.3: apply the commits not stored yet, then lower the commits, file
/// churn and coupling. `seen` holds the digests already stored.
pub fn history_methods(
    commits: &[CommitRecord],
    seen: BTreeSet<String>,
    config: &HistoryConfig,
) -> Result<(Vec<Method>, HistoryRunReport), String> {
    let mut graph = HistoryGraph::default();
    graph.commits.clone_from(&seen);
    let report = graph.ingest_history(commits, config);
    let added: Vec<&CommitRecord> = commits
        .iter()
        .filter(|c| graph.commits.contains(c.digest()) && !seen.contains(c.digest()))
        .collect();
    let mut methods = commit_methods(&graph, &added)?;
    methods.extend(coupling_methods(&graph, config)?);
    Ok((methods, report))
}

fn link_edges(links: &[ServiceLink]) -> Result<Vec<Method>, String> {
    links
        .iter()
        .map(|link| {
            add_edge(
                link.from.as_str().to_string(),
                link.service.as_str().to_string(),
                "ownedByService",
            )
        })
        .collect()
}

/// R602.3: link every route and IaC resource to its owning service.
pub fn link_methods(inputs: &HookInputs) -> Result<(Vec<Method>, LinkRunReport), String> {
    let (links, _ties, report) = link_pass(&inputs.resources, &inputs.routes, &inputs.services);
    Ok((link_edges(&links)?, report))
}

/// Both hooks over the committed rows, as methods of the enrichment envelope
/// and a run node recording both reports.
pub(super) fn run_hooks(
    rows: &[Row],
    run_id: &str,
    stored: &dyn Fn(&str) -> Option<ProvenanceRung>,
    config: &HistoryConfig,
) -> Result<Vec<Method>, String> {
    let inputs = hook_inputs(rows);
    let seen = inputs
        .commits
        .iter()
        .map(|c| c.digest().to_string())
        .filter(|digest| stored(&format!("{COMMIT_HISTORY_PREFIX}{digest}")).is_some())
        .collect();
    let (mut methods, history) = history_methods(&inputs.commits, seen, config)?;
    let (links, link_report) = link_methods(&inputs)?;
    methods.extend(links);
    let report_id = format!("{run_id}:hooks");
    methods.push(add_node(
        report_id.clone(),
        &[
            ("type", "RepositoryHookRun".to_string()),
            ("commits_added", history.commits_added.to_string()),
            ("commits_skipped", history.commits_skipped.to_string()),
            ("coupling_edges", history.coupling_edges.to_string()),
            ("history_truncated", history.truncated.unwrap_or_default()),
            ("routes", link_report.routes.to_string()),
            ("resources", link_report.resources.to_string()),
            ("links_written", link_report.links_written.to_string()),
            ("ambiguous_ties", link_report.ambiguous_ties.to_string()),
        ],
    )?);
    methods.push(add_edge(run_id.to_string(), report_id, "producedHookRun")?);
    Ok(methods)
}
