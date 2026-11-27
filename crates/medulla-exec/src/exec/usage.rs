//! `USAGE QUERIES` / `USAGE ASSETS` (MEDULLA-TYPED-PACKS-R108.8): operational metadata read
//! from provenance, as a UQL source.
//!
//! No counter is added here. The served path binds a [`UsageSource`] over the usage series the
//! server's own query dispatch writes (`asset_usage.queries` carries the question digest,
//! `asset_usage.reads` the asset) in the caller's tenant and graph, trimmed to the trailing
//! window `knob catalog.top_queries_window_days`. `USAGE ASSETS` also counts the `ToolCall`
//! nodes of the (already narrowed) graph view whose `valid_from` falls in that window: each is
//! one run of the tool its `tool_name` names, and every `RunTrace` with an edge to it is one
//! run that used the tool. Rows are ranked by uses (most first, then by id); a row never
//! carries a result row of the question it counts.

use std::collections::{BTreeMap, BTreeSet};

use super::PlanCtx;
use crate::rowset::{RowSet, ValueChannels};
use medulla_query_types::wire::UsageView;
use {
    medulla_catalog_source_types::asset_usage::usage_series_tags,
    medulla_catalog_source_types::asset_usage::ASSET_USAGE_QUERIES_SERIES,
    medulla_catalog_source_types::asset_usage::ASSET_USAGE_READS_SERIES,
    medulla_catalog_source_types::asset_usage::USAGE_VIEW_CHANNELS,
};

/// The node label of one tool run in the provenance graph.
pub const TOOL_CALL_LABEL: &str = "ToolCall";
/// The node label of one agent run in the provenance graph.
pub const RUN_TRACE_LABEL: &str = "RunTrace";
/// The `ToolCall` property naming the tool that ran.
pub const TOOL_NAME_PROPERTY: &str = "tool_name";
/// The `ToolCall` property holding when it ran (unix seconds).
pub const TOOL_CALL_TIME_PROPERTY: &str = "valid_from";
/// The tag of an `asset_usage.queries` series that holds the question digest.
const QUERY_TAG: &str = "query";

/// The trailing window a usage view ranks over, in unix seconds, both ends inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageWindow {
    pub from_s: u64,
    pub to_s: u64,
}

impl UsageWindow {
    /// Whether the instant `at_s` lies inside the window.
    pub fn contains(&self, at_s: u64) -> bool {
        (self.from_s..=self.to_s).contains(&at_s)
    }
}

/// One usage series of the caller's tenant and graph, summed over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageSeries {
    /// The series name (`<base>|principal=…|asset=…[|tag=…]`).
    pub name: String,
    /// The uses recorded inside the window.
    pub uses: u64,
    /// The last use inside the window (unix seconds).
    pub last_used_s: u64,
}

/// The caller's usage log, bound per request by the served path.
pub trait UsageSource: Send + Sync {
    /// The window the views rank over (`knob catalog.top_queries_window_days`).
    fn window(&self) -> UsageWindow;

    /// Every series of `base` with at least one use inside the window, or why the log
    /// cannot be read.
    fn usage_series(&self, base: &str) -> Result<Vec<UsageSeries>, String>;
}

/// What one row of a usage view adds up.
#[derive(Default)]
struct Tally {
    uses: u64,
    last_used_s: u64,
    users: BTreeSet<String>,
    runs: BTreeSet<String>,
}

impl Tally {
    fn seen(&mut self, uses: u64, at_s: u64) {
        self.uses = self.uses.saturating_add(uses);
        self.last_used_s = self.last_used_s.max(at_s);
    }
}

/// Run one `USAGE` stage (a source: its input is ignored, like `MATCH`).
pub(super) fn usage_scan(ctx: &PlanCtx, view: UsageView) -> Result<RowSet, String> {
    let Some(source) = ctx.usage else {
        return Err(
            "USAGE needs the caller's usage log, and none is bound to this plan".to_string(),
        );
    };
    let tallies = match view {
        UsageView::Queries => query_tallies(&source.usage_series(ASSET_USAGE_QUERIES_SERIES)?),
        UsageView::Assets => {
            let mut tallies = asset_tallies(&source.usage_series(ASSET_USAGE_READS_SERIES)?);
            tool_call_tallies(ctx.view, source.window(), &mut tallies);
            tallies
        }
    };
    Ok(ranked(tallies))
}

/// One row per question digest. A question that read several assets wrote one point per
/// asset, so its runs by one principal are the most any one asset recorded.
fn query_tallies(series: &[UsageSeries]) -> BTreeMap<String, Tally> {
    let mut runs: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for entry in series {
        let Some(tags) = usage_series_tags(&entry.name, ASSET_USAGE_QUERIES_SERIES) else {
            continue;
        };
        let Some((QUERY_TAG, digest)) = tags.extra else {
            continue;
        };
        let by_principal = runs.entry(digest.to_string()).or_default();
        let most = by_principal.entry(tags.principal.to_string()).or_insert(0);
        *most = (*most).max(entry.uses);
        let tally = tallies.entry(digest.to_string()).or_default();
        tally.seen(0, entry.last_used_s);
        tally.users.insert(tags.principal.to_string());
    }
    for (digest, by_principal) in runs {
        if let Some(tally) = tallies.get_mut(&digest) {
            tally.uses = by_principal.values().sum();
        }
    }
    tallies
}

/// One row per asset read through the server's query dispatch.
fn asset_tallies(series: &[UsageSeries]) -> BTreeMap<String, Tally> {
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for entry in series {
        let Some(tags) = usage_series_tags(&entry.name, ASSET_USAGE_READS_SERIES) else {
            continue;
        };
        if tags.extra.is_some() {
            continue;
        }
        let tally = tallies.entry(tags.asset.to_string()).or_default();
        tally.seen(entry.uses, entry.last_used_s);
        tally.users.insert(tags.principal.to_string());
    }
    tallies
}

/// Add each `ToolCall` node of the view that ran inside `window` as one use of its tool, and
/// each `RunTrace` with an edge to it as one run of that tool.
fn tool_call_tallies(
    view: &medulla_graph_core::graph::GraphView,
    window: UsageWindow,
    tallies: &mut BTreeMap<String, Tally>,
) {
    for (id, blob) in &view.node_properties {
        let Ok(node) = medulla_types::msgpack::decode_property_value(blob.as_slice()) else {
            continue;
        };
        if !medulla_graph_core::graph::node_carries_label(&node, TOOL_CALL_LABEL) {
            continue;
        }
        let Some(tool) = node.get(TOOL_NAME_PROPERTY).and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(at_s) = node.get(TOOL_CALL_TIME_PROPERTY).and_then(|v| v.as_u64()) else {
            continue;
        };
        if !window.contains(at_s) {
            continue;
        }
        let tally = tallies.entry(tool.to_string()).or_default();
        tally.seen(1, at_s);
        tally.runs.extend(runs_of(view, id));
    }
}

/// The `RunTrace` nodes with an edge to `tool_call`.
fn runs_of(view: &medulla_graph_core::graph::GraphView, tool_call: &str) -> Vec<String> {
    let Some(&index) = view.node_map.get(tool_call) else {
        return Vec::new();
    };
    view.graph
        .neighbors_directed(index, petgraph::Direction::Incoming)
        .filter_map(|source| view.graph.node_weight(source))
        .filter(|run| {
            view.node_properties
                .get(run.as_str())
                .and_then(|blob| {
                    medulla_types::msgpack::decode_property_value(blob.as_slice()).ok()
                })
                .is_some_and(|node| {
                    medulla_graph_core::graph::node_carries_label(&node, RUN_TRACE_LABEL)
                })
        })
        .cloned()
        .collect()
}

/// Rank the tallies by uses (most first, then by id) with their value channels.
fn ranked(tallies: BTreeMap<String, Tally>) -> RowSet {
    let mut rows: Vec<(String, Tally)> = tallies
        .into_iter()
        .filter(|(_, tally)| tally.uses > 0)
        .collect();
    rows.sort_by(|a, b| b.1.uses.cmp(&a.1.uses).then_with(|| a.0.cmp(&b.0)));
    let mut values = ValueChannels::new();
    let scored: Vec<(String, f32)> = rows
        .into_iter()
        .map(|(id, tally)| {
            let [uses, last_used, users, runs] = USAGE_VIEW_CHANNELS;
            values.insert(
                id.clone(),
                BTreeMap::from([
                    (uses.to_string(), tally.uses as f64),
                    (last_used.to_string(), tally.last_used_s as f64),
                    (users.to_string(), tally.users.len() as f64),
                    (runs.to_string(), tally.runs.len() as f64),
                ]),
            );
            (id, tally.uses as f32)
        })
        .collect();
    RowSet::from_scored(scored).with_values(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use medulla_semantic_index::semantic::SemanticStore;
    use serde_json::json;
    use {
        medulla_catalog_source_types::asset_usage::usage_query_series_name,
        medulla_catalog_source_types::asset_usage::usage_series_name,
    };

    const WINDOW: UsageWindow = UsageWindow {
        from_s: 1_000,
        to_s: 2_000,
    };

    struct Log(Vec<UsageSeries>);

    impl UsageSource for Log {
        fn window(&self) -> UsageWindow {
            WINDOW
        }
        fn usage_series(&self, base: &str) -> Result<Vec<UsageSeries>, String> {
            Ok(self
                .0
                .iter()
                .filter(|s| usage_series_tags(&s.name, base).is_some())
                .cloned()
                .collect())
        }
    }

    fn series(name: String, uses: u64, last_used_s: u64) -> UsageSeries {
        UsageSeries {
            name,
            uses,
            last_used_s,
        }
    }

    fn log() -> Log {
        Log(vec![
            // q1 read two assets three times as alice: three runs, not six.
            series(usage_query_series_name("alice", "Doc", "q1"), 3, 1_500),
            series(usage_query_series_name("alice", "Person", "q1"), 3, 1_500),
            series(usage_query_series_name("bob", "Doc", "q1"), 1, 1_700),
            series(usage_query_series_name("bob", "Doc", "q2"), 2, 1_200),
            series(
                usage_series_name(ASSET_USAGE_READS_SERIES, "alice", "Doc"),
                3,
                1_500,
            ),
            series(
                usage_series_name(ASSET_USAGE_READS_SERIES, "bob", "Doc"),
                3,
                1_700,
            ),
        ])
    }

    fn props(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    fn graph() -> GraphCore {
        let graph = GraphCore::new();
        graph.add_node("run-1".into(), props(json!({"node_type": RUN_TRACE_LABEL})));
        for (id, at) in [("tc-1", 1_100), ("tc-2", 1_900), ("tc-old", 10)] {
            graph.add_node(
                id.into(),
                props(json!({"node_type": TOOL_CALL_LABEL, "tool_name": "graph_query", "valid_from": at})),
            );
        }
        for call in ["tc-1", "tc-2"] {
            graph
                .add_edge(
                    "run-1".into(),
                    call.into(),
                    props(json!({"relationship": "USED_TOOL"})),
                )
                .unwrap();
        }
        graph
    }

    fn value(rows: &RowSet, id: &str, channel: &str) -> f64 {
        rows.value(id, channel).unwrap()
    }

    // spec: MEDULLA-TYPED-PACKS-R108.8
    #[test]
    fn queries_rank_by_runs_per_digest_inside_the_window() {
        let graph = graph();
        let view = graph.analysis_snapshot();
        let semantic = SemanticStore::new();
        let log = log();
        let ctx = PlanCtx::new(&view, &semantic).with_usage(&log);
        let rows = usage_scan(&ctx, UsageView::Queries).unwrap();
        assert_eq!(rows.ids(), vec!["q1".to_string(), "q2".to_string()]);
        assert_eq!(value(&rows, "q1", "uses"), 4.0);
        assert_eq!(value(&rows, "q1", "users"), 2.0);
        assert_eq!(value(&rows, "q1", "last_used"), 1_700.0);
        assert_eq!(value(&rows, "q2", "uses"), 2.0);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.8
    #[test]
    fn assets_add_reads_and_tool_calls_with_their_runs() {
        let graph = graph();
        let view = graph.analysis_snapshot();
        let semantic = SemanticStore::new();
        let log = log();
        let ctx = PlanCtx::new(&view, &semantic).with_usage(&log);
        let rows = usage_scan(&ctx, UsageView::Assets).unwrap();
        assert_eq!(
            rows.ids(),
            vec!["Doc".to_string(), "graph_query".to_string()]
        );
        assert_eq!(value(&rows, "Doc", "uses"), 6.0);
        assert_eq!(value(&rows, "Doc", "users"), 2.0);
        // The call from before the window is not counted.
        assert_eq!(value(&rows, "graph_query", "uses"), 2.0);
        assert_eq!(value(&rows, "graph_query", "runs"), 1.0);
        assert_eq!(value(&rows, "graph_query", "last_used"), 1_900.0);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.8
    #[test]
    fn an_unbound_usage_log_is_a_typed_error() {
        let graph = graph();
        let view = graph.analysis_snapshot();
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(&view, &semantic);
        let error = usage_scan(&ctx, UsageView::Queries).unwrap_err();
        assert!(error.contains("USAGE"), "{error}");
    }
}
