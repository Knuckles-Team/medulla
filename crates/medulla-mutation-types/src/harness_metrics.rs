//! Harness metric statements (MEDULLA-ORCH-VOCAB-R010.1 to R010.5, R010.101): the
//! closed `HARNESS_QUERIES` table served through `Method::Uql`, and the pure
//! aggregation each statement runs over the `RunTrace` nodes the caller can see.
//!
//! A statement divides counts per group and never invents a number: a group with
//! no solved trace abstains instead of reporting zero cost, and a trace with no
//! usable measurement is excluded from its mean and counted.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::benchmark_claim::{round_half_even_div, PriceTable};

/// Refusal reason of a cost group without a solved trace.
pub const NO_SOLVED_TRACE: &str = "NO_SOLVED_TRACE";

/// The property a `RunTrace` group key reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKey {
    /// The trace's model (`:runModel`).
    Model,
    /// The trace's task class (`:runTaskClass`).
    TaskClass,
    /// The harness configuration the trace ran with (`:appliedHarnessConfig`).
    Config,
}

impl GroupKey {
    fn property(self) -> &'static str {
        match self {
            Self::Model => "runModel",
            Self::TaskClass => "runTaskClass",
            Self::Config => "appliedHarnessConfig",
        }
    }
}

/// One statement of the table: its name, its group keys and its result columns.
#[derive(Clone, Copy, Debug)]
pub struct HarnessQuery {
    /// The UQL statement name a caller sends.
    pub name: &'static str,
    /// The keys each result row is grouped by, in order.
    pub groups: &'static [GroupKey],
    /// The result columns, one value per column in each row.
    pub columns: &'static [&'static str],
}

/// The closed statement table; a text outside it is not a harness statement.
pub const HARNESS_QUERIES: &[HarnessQuery] = &[
    HarnessQuery {
        name: "harness_success_rate",
        groups: &[GroupKey::Model, GroupKey::TaskClass],
        columns: &["success_rate", "solved", "total"],
    },
    HarnessQuery {
        name: "harness_cost_per_solved_task",
        groups: &[GroupKey::Model, GroupKey::TaskClass, GroupKey::Config],
        columns: &["cost_per_solved_task", "solved"],
    },
    HarnessQuery {
        name: "harness_overflow_rate",
        groups: &[GroupKey::Model, GroupKey::TaskClass],
        columns: &["overflow_rate", "overflow", "total"],
    },
    HarnessQuery {
        name: "harness_mean_peak_ratio",
        groups: &[GroupKey::Model, GroupKey::TaskClass],
        columns: &["mean_peak_ratio", "included", "excluded"],
    },
    HarnessQuery {
        name: "harness_mechanism_counts",
        groups: &[GroupKey::Model, GroupKey::Config],
        columns: &[
            "mean_elision_count",
            "mean_summarization_count",
            "mean_turn_count",
            "mean_tool_call_count",
        ],
    },
    // MEDULLA-ORCH-VOCAB-R010.101: grouped by the request's `grouping`, so it
    // declares no fixed group keys; see [`RunTraceUsageQuery`].
    HarnessQuery {
        name: RUN_TRACE_USAGE,
        groups: &[],
        columns: &USAGE_COLUMNS,
    },
];

/// The statement a request text names, if any (surrounding blanks and one
/// trailing semicolon are ignored).
#[must_use]
pub fn harness_query(text: &str) -> Option<&'static HarnessQuery> {
    let name = text.trim().trim_end_matches(';').trim();
    HARNESS_QUERIES.iter().find(|query| query.name == name)
}

/// One answered group: its key, one value per column, and the abstention reason
/// when the group has nothing to divide by.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricRow {
    /// The group key values joined into the row id.
    pub group: String,
    /// One value per statement column; `None` when the column has no measurement.
    pub values: Vec<Option<f64>>,
    /// The abstention reason when the group has nothing to divide by.
    pub abstained: Option<&'static str>,
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::String(text) => text.parse::<f64>().ok(),
        other => other.as_f64(),
    }
    .filter(|v| v.is_finite())
}

fn text<'a>(props: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn ratio(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

fn solved(props: &Map<String, Value>) -> bool {
    text(props, "status") == Some("completed")
}

/// Run one statement over the `RunTrace` property objects. Rows are ordered by
/// group key; a trace missing a group key is not part of any group.
#[must_use]
pub fn run_harness_query(query: &HarnessQuery, traces: &[Map<String, Value>]) -> Vec<MetricRow> {
    let mut groups: BTreeMap<String, Vec<&Map<String, Value>>> = BTreeMap::new();
    for trace in traces {
        let keys: Option<Vec<&str>> = query
            .groups
            .iter()
            .map(|key| text(trace, key.property()))
            .collect();
        if let Some(keys) = keys {
            groups.entry(keys.join("|")).or_default().push(trace);
        }
    }
    groups
        .into_iter()
        .map(|(group, members)| answer_group(query.name, group, &members))
        .collect()
}

fn answer_group(name: &str, group: String, members: &[&Map<String, Value>]) -> MetricRow {
    let total = members.len();
    let wins = members.iter().filter(|t| solved(t)).count();
    let counted = |key: &str| -> Vec<f64> {
        members
            .iter()
            .filter_map(|t| number(t.get(key)))
            .collect::<Vec<_>>()
    };
    let (values, abstained) = match name {
        "harness_success_rate" => (
            vec![ratio(wins, total), Some(wins as f64), Some(total as f64)],
            None,
        ),
        "harness_cost_per_solved_task" => {
            if wins == 0 {
                (vec![None, Some(0.0)], Some(NO_SOLVED_TRACE))
            } else {
                let cost: f64 = counted("costUsd").iter().sum();
                (vec![Some(cost / wins as f64), Some(wins as f64)], None)
            }
        }
        "harness_overflow_rate" => {
            let overflow = members
                .iter()
                .filter(|t| text(t, "terminationReason") == Some("context_overflow"))
                .count();
            (
                vec![
                    ratio(overflow, total),
                    Some(overflow as f64),
                    Some(total as f64),
                ],
                None,
            )
        }
        "harness_mean_peak_ratio" => {
            let ratios: Vec<f64> = members
                .iter()
                .filter_map(|t| {
                    let budget = number(t.get("contextBudgetTokens"))?;
                    let peak = number(t.get("peakContextTokens"))?;
                    (budget > 0.0).then(|| peak / budget)
                })
                .collect();
            (
                vec![
                    mean(&ratios),
                    Some(ratios.len() as f64),
                    Some((total - ratios.len()) as f64),
                ],
                None,
            )
        }
        _ => (
            [
                "elisionCount",
                "summarizationCount",
                "turnCount",
                "toolCallCount",
            ]
            .iter()
            .map(|key| mean(&counted(key)))
            .collect(),
            None,
        ),
    };
    MetricRow {
        group,
        values,
        abstained,
    }
}

// ── MEDULLA-ORCH-VOCAB-R010.101: project and agent token and cost totals ──────

/// The usage statement's name.
pub const RUN_TRACE_USAGE: &str = "run_trace_usage";

/// A token rate is a minor-unit numerator per this many tokens (the unit of the
/// rate itself, not a tunable).
const TOKENS_PER_RATE: u128 = 1_000_000;

/// The schema version of [`RunTraceUsageQuery`].
pub const RUN_TRACE_USAGE_QUERY_SCHEMA_VERSION: u16 = 1;

/// The columns of a usage row: the tokens of each category and their total,
/// then the cost of each category and their total, in minor currency units.
pub const USAGE_COLUMNS: [&str; 10] = [
    "uncached_input_tokens",
    "cached_input_tokens",
    "output_tokens",
    "reasoning_output_tokens",
    "total_tokens",
    "uncached_input_cost_minor",
    "cached_input_cost_minor",
    "output_cost_minor",
    "reasoning_output_cost_minor",
    "total_cost_minor",
];

/// Why a usage value is unknown; an unknown value is never reported as zero.
pub const COUNTER_UNKNOWN: &str = "COUNTER_UNKNOWN";
/// A member's model has no rate for the category in the pinned table.
pub const PRICE_UNKNOWN: &str = "PRICE_UNKNOWN";
/// The pinned price table revision is not visible or does not decode.
pub const PRICE_TABLE_UNKNOWN: &str = "PRICE_TABLE_UNKNOWN";
/// The requested currency differs from the pinned table's and no FX revision is pinned.
pub const CURRENCY_MISMATCH: &str = "CURRENCY_MISMATCH";
/// A trace lacks an attribute the grouping needs.
pub const ATTRIBUTION_UNKNOWN: &str = "ATTRIBUTION_UNKNOWN";
/// A trace lacks its completion time, so no window holds it.
pub const COMPLETION_TIME_UNKNOWN: &str = "COMPLETION_TIME_UNKNOWN";

/// The four disjoint token categories of a run. Totals add each once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCategory {
    /// Input tokens read without a cache hit.
    UncachedInput,
    /// Input tokens served from the provider's cache.
    CachedInput,
    /// Visible output tokens.
    Output,
    /// Output tokens spent on reasoning.
    ReasoningOutput,
}

impl UsageCategory {
    /// Every category, in column order.
    pub const ALL: [Self; 4] = [
        Self::UncachedInput,
        Self::CachedInput,
        Self::Output,
        Self::ReasoningOutput,
    ];

    /// The `RunTrace` property holding the category's token count.
    pub fn trace_property(self) -> &'static str {
        match self {
            Self::UncachedInput => "uncachedInputTokens",
            Self::CachedInput => "cachedInputTokens",
            Self::Output => "outputTokens",
            Self::ReasoningOutput => "reasoningOutputTokens",
        }
    }
}

/// A usage grouping dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageGroupKey {
    /// Group by the trace's model.
    Model,
    /// Group by the trace's project id.
    Project,
    /// Group by the trace's agent id.
    Agent,
}

impl UsageGroupKey {
    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "model" => Some(Self::Model),
            "project" => Some(Self::Project),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }

    fn property(self) -> &'static str {
        match self {
            Self::Model => "runModel",
            Self::Project => "projectId",
            Self::Agent => "agentId",
        }
    }
}

/// Where a page resumes: the snapshot the previous page was read at and the
/// last group it held. Its text is `<snapshot_revision>:<group>`, composed from
/// the previous answer's `snapshot_revision` and its last row id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCursor {
    /// The snapshot revision the previous page was read at.
    pub snapshot_revision: u64,
    /// The last group of the previous page; the next page starts after it.
    pub after_group: String,
}

impl UsageCursor {
    /// The cursor text.
    pub fn encode(&self) -> String {
        format!("{}:{}", self.snapshot_revision, self.after_group)
    }

    fn decode(text: &str) -> Option<Self> {
        let (revision, group) = text.split_once(':')?;
        if group.is_empty() {
            return None;
        }
        Some(Self {
            snapshot_revision: revision.parse().ok()?,
            after_group: group.to_string(),
        })
    }
}

/// One `run_trace_usage` request. The tenant is the verified caller's: the
/// statement reads only the caller's visible snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTraceUsageQuery {
    /// The request schema version.
    pub schema_version: u16,
    /// Inclusive start of the `completedAtMs` window.
    pub window_start_ms: u64,
    /// Exclusive end of the `completedAtMs` window.
    pub window_end_ms: u64,
    /// The grouping dimensions, in order.
    pub grouping: Vec<UsageGroupKey>,
    /// The currency costs are reported in.
    pub currency: String,
    /// The pinned `PriceTable` revision the costs use.
    pub price_table_revision: String,
    /// The most groups one page returns.
    pub page_size: usize,
    /// Where this page resumes; `None` for the first page.
    pub cursor: Option<UsageCursor>,
}

fn invalid(detail: &str) -> String {
    format!("USAGE_QUERY_INVALID: {detail}")
}

impl RunTraceUsageQuery {
    /// Decode the statement's parameters. `text` reads a text parameter and
    /// `number` a numeric one; `page_size_max` is the registry ceiling a
    /// requested page size is held to.
    pub fn from_params(
        text: impl Fn(&str) -> Option<String>,
        number: impl Fn(&str) -> Option<f64>,
        page_size_max: u64,
    ) -> Result<Self, String> {
        let whole = |name: &str| -> Result<u64, String> {
            let value = number(name).ok_or_else(|| invalid(&format!("${name} is required")))?;
            if value.fract() != 0.0 || value < 0.0 || value > u64::MAX as f64 {
                return Err(invalid(&format!("${name} must be a non-negative integer")));
            }
            Ok(value as u64)
        };
        let window_start_ms = whole("window_start_ms")?;
        let window_end_ms = whole("window_end_ms")?;
        if window_end_ms <= window_start_ms {
            return Err(invalid("$window_end_ms must be after $window_start_ms"));
        }
        let grouping_text = text("grouping").ok_or_else(|| invalid("$grouping is required"))?;
        let mut grouping = Vec::new();
        for part in grouping_text.split(',') {
            let key = UsageGroupKey::parse(part).ok_or_else(|| {
                invalid(&format!(
                    "grouping `{}` is not model, project or agent",
                    part.trim()
                ))
            })?;
            if grouping.contains(&key) {
                return Err(invalid("a grouping key is named twice"));
            }
            grouping.push(key);
        }
        let currency = text("currency").ok_or_else(|| invalid("$currency is required"))?;
        let price_table_revision = text("price_table_revision")
            .ok_or_else(|| invalid("$price_table_revision is required"))?;
        let page_size = match number("page_size") {
            None => page_size_max,
            Some(_) => whole("page_size")?.clamp(1, page_size_max),
        } as usize;
        let cursor = match text("cursor") {
            None => None,
            Some(cursor) => Some(UsageCursor::decode(&cursor).ok_or_else(|| {
                "USAGE_CURSOR_INVALID: the cursor is not a usage page cursor".to_string()
            })?),
        };
        Ok(Self {
            schema_version: RUN_TRACE_USAGE_QUERY_SCHEMA_VERSION,
            window_start_ms,
            window_end_ms,
            grouping,
            currency,
            price_table_revision,
            page_size,
            cursor,
        })
    }
}

/// The pinned price table revision, read from a `PriceTable` node: its
/// `revision` text and its `table` JSON.
pub fn price_table_from_node(props: &Map<String, Value>) -> Option<(String, PriceTable)> {
    if text(props, "node_type") != Some("PriceTable") {
        return None;
    }
    let revision = text(props, "revision")?.to_string();
    let (table, _) = PriceTable::load(text(props, "table")?).ok()?;
    Some((revision, table))
}

/// One answered usage group: a value per [`USAGE_COLUMNS`] column (`None` is
/// unknown) and the reason of each unknown column.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageRow {
    /// The group key values joined into the row id.
    pub group: String,
    /// One value per [`USAGE_COLUMNS`] column; `None` is unknown.
    pub values: Vec<Option<f64>>,
    /// Each unknown column with the reason it is unknown.
    pub unknown: Vec<(&'static str, &'static str)>,
}

/// One page of usage groups (at most `page_size`, in group order) and the
/// warnings of unknown values and of traces that could not be attributed.
#[derive(Clone, Debug, PartialEq)]
pub struct UsagePage {
    /// The groups of this page, in group order.
    pub rows: Vec<UsageRow>,
    /// Warnings for unknown values and for traces that could not be attributed.
    pub warnings: Vec<String>,
}

/// The usage of one category over a group: the exact token sum and the exact
/// cost numerator, each unknown with a reason.
#[derive(Default)]
struct CategorySum {
    tokens: u128,
    tokens_unknown: bool,
    numerator: u128,
    price_unknown: Option<&'static str>,
}

/// Run `run_trace_usage` over the caller's `RunTrace` nodes at
/// `snapshot_revision`, pricing with `table` (the pinned revision, or `None`
/// when it is not visible). Exact numerators are summed per group and category
/// and rounded once, half to even, per final group and category.
pub fn run_trace_usage(
    query: &RunTraceUsageQuery,
    traces: &[Map<String, Value>],
    table: Option<&PriceTable>,
    snapshot_revision: u64,
) -> Result<UsagePage, String> {
    if let Some(cursor) = &query.cursor {
        if cursor.snapshot_revision != snapshot_revision {
            return Err(format!(
                "USAGE_CURSOR_STALE: the cursor was read at snapshot {} and the graph is at {snapshot_revision}",
                cursor.snapshot_revision
            ));
        }
    }
    let mut warnings = Vec::new();
    let mut unattributed = 0usize;
    let mut untimed = 0usize;
    let mut groups: BTreeMap<String, Vec<&Map<String, Value>>> = BTreeMap::new();
    for trace in traces {
        let Some(completed) = trace.get("completedAtMs").and_then(Value::as_u64) else {
            untimed += 1;
            continue;
        };
        if completed < query.window_start_ms || completed >= query.window_end_ms {
            continue;
        }
        let keys: Option<Vec<&str>> = query
            .grouping
            .iter()
            .map(|key| text(trace, key.property()))
            .collect();
        match keys {
            Some(keys) => groups.entry(keys.join("|")).or_default().push(trace),
            None => unattributed += 1,
        }
    }
    if untimed > 0 {
        warnings.push(format!(
            "unknown {COMPLETION_TIME_UNKNOWN}: {untimed} traces have no completion time"
        ));
    }
    if unattributed > 0 {
        warnings.push(format!(
            "unknown {ATTRIBUTION_UNKNOWN}: {unattributed} traces lack a grouping attribute"
        ));
    }
    let price = match table {
        None => Err(PRICE_TABLE_UNKNOWN),
        Some(table) if table.currency != query.currency => Err(CURRENCY_MISMATCH),
        Some(table) => Ok(table),
    };
    let after = query
        .cursor
        .as_ref()
        .map(|cursor| cursor.after_group.as_str());
    let rows: Vec<UsageRow> = groups
        .iter()
        .filter(|(group, _)| after.is_none_or(|after| group.as_str() > after))
        .take(query.page_size)
        .map(|(group, members)| usage_row(group, members, price))
        .collect();
    for row in &rows {
        for (column, reason) in &row.unknown {
            warnings.push(format!("unknown {reason}: {}|{column}", row.group));
        }
    }
    Ok(UsagePage { rows, warnings })
}

fn usage_row(
    group: &str,
    members: &[&Map<String, Value>],
    price: Result<&PriceTable, &'static str>,
) -> UsageRow {
    let mut sums: [CategorySum; 4] = Default::default();
    for trace in members {
        let model = text(trace, "runModel");
        for (index, category) in UsageCategory::ALL.iter().enumerate() {
            let sum = &mut sums[index];
            let Some(tokens) = trace.get(category.trace_property()).and_then(Value::as_u64) else {
                sum.tokens_unknown = true;
                continue;
            };
            sum.tokens += u128::from(tokens);
            let rate = match price {
                Err(reason) => Err(reason),
                Ok(table) => model
                    .and_then(|model| table.token_rates.get(model))
                    .and_then(|rates| rates.get(category))
                    .copied()
                    .ok_or(PRICE_UNKNOWN),
            };
            match rate {
                Ok(rate) => sum.numerator += u128::from(tokens) * u128::from(rate),
                Err(reason) => {
                    sum.price_unknown.get_or_insert(reason);
                }
            }
        }
    }
    let mut values = Vec::with_capacity(USAGE_COLUMNS.len());
    let mut unknown = Vec::new();
    let categories = UsageCategory::ALL.len();
    let mut total_tokens = Some(0u128);
    for (index, sum) in sums.iter().enumerate() {
        if sum.tokens_unknown {
            unknown.push((USAGE_COLUMNS[index], COUNTER_UNKNOWN));
            values.push(None);
            total_tokens = None;
        } else {
            values.push(Some(sum.tokens as f64));
            total_tokens = total_tokens.map(|total| total + sum.tokens);
        }
    }
    if total_tokens.is_none() {
        unknown.push((USAGE_COLUMNS[categories], COUNTER_UNKNOWN));
    }
    values.push(total_tokens.map(|total| total as f64));
    let mut total_cost = Some(0u128);
    let mut total_reason = None;
    for (index, sum) in sums.iter().enumerate() {
        let column = USAGE_COLUMNS[categories + 1 + index];
        let reason = if sum.tokens_unknown {
            Some(COUNTER_UNKNOWN)
        } else {
            sum.price_unknown
        };
        match reason {
            Some(reason) => {
                unknown.push((column, reason));
                values.push(None);
                total_cost = None;
                total_reason.get_or_insert(reason);
            }
            None => {
                // Rounded once, per final group and category.
                let cost = round_half_even_div(sum.numerator, TOKENS_PER_RATE);
                values.push(Some(cost as f64));
                total_cost = total_cost.map(|total| total + cost);
            }
        }
    }
    if let Some(reason) = total_reason {
        unknown.push((USAGE_COLUMNS[USAGE_COLUMNS.len() - 1], reason));
    }
    values.push(total_cost.map(|total| total as f64));
    UsageRow {
        group: group.to_string(),
        values,
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(model: &str, task: &str, status: &str, cost: &str) -> Map<String, Value> {
        let value = serde_json::json!({
            "status": status, "terminationReason": "final_answer", "costUsd": cost,
            "runModel": model, "runTaskClass": task, "appliedHarnessConfig": "c1",
            "peakContextTokens": 50, "contextBudgetTokens": 100,
        });
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn table_is_closed_and_names_resolve() {
        for query in HARNESS_QUERIES {
            assert_eq!(harness_query(query.name).map(|q| q.name), Some(query.name));
        }
        assert!(harness_query(" harness_success_rate; ").is_some());
        assert!(harness_query("MATCH (n)").is_none());
    }

    #[test]
    fn success_rate_groups_and_cost_abstains() {
        let traces = vec![
            trace("a", "t", "completed", "0.10"),
            trace("a", "t", "failed", "0.30"),
            trace("b", "t", "failed", "0.20"),
        ];
        let rate = run_harness_query(harness_query("harness_success_rate").unwrap(), &traces);
        assert_eq!(rate[0].group, "a|t");
        assert_eq!(rate[0].values[0], Some(0.5));
        let cost = run_harness_query(
            harness_query("harness_cost_per_solved_task").unwrap(),
            &traces,
        );
        assert_eq!(cost[0].values[0], Some(0.4));
        assert_eq!(cost[1].abstained, Some(NO_SOLVED_TRACE));
        assert_eq!(cost[1].values[0], None);
    }

    fn usage_trace(model: &str, project: &str, tokens: [u64; 4], at: u64) -> Map<String, Value> {
        let value = serde_json::json!({
            "runModel": model, "projectId": project, "completedAtMs": at,
            "uncachedInputTokens": tokens[0], "cachedInputTokens": tokens[1],
            "outputTokens": tokens[2], "reasoningOutputTokens": tokens[3],
        });
        value.as_object().cloned().unwrap_or_default()
    }

    fn usage_query(cursor: Option<&str>, page_size: f64) -> RunTraceUsageQuery {
        let texts = |name: &str| -> Option<String> {
            match name {
                "grouping" => Some("project".into()),
                "currency" => Some("USD".into()),
                "price_table_revision" => Some("prices-1".into()),
                "cursor" => cursor.map(str::to_string),
                _ => None,
            }
        };
        let numbers = |name: &str| -> Option<f64> {
            match name {
                "window_start_ms" => Some(0.0),
                "window_end_ms" => Some(1_000.0),
                "page_size" => Some(page_size),
                _ => None,
            }
        };
        RunTraceUsageQuery::from_params(texts, numbers, 100).unwrap()
    }

    fn table(rate: u64) -> PriceTable {
        let config = serde_json::json!({
            "schema_version": 1, "version": 1, "currency": "USD", "minor_unit_exponent": 2,
            "token_rates": {"m": {"uncached_input": rate, "cached_input": rate,
                                  "output": rate, "reasoning_output": rate}},
        });
        PriceTable::load(&config.to_string()).unwrap().0
    }

    // spec: MEDULLA-ORCH-VOCAB-R010.101
    #[test]
    fn usage_rounds_once_per_group_half_even_and_unknown_is_never_zero() {
        // Two traces of 250_000 tokens at 2 minor units per million: each alone
        // is 0.5 (rounds to 0), together 1.0 exactly; rounding per trace would say 0.
        let traces = vec![
            usage_trace("m", "p1", [250_000, 0, 0, 0], 10),
            usage_trace("m", "p1", [250_000, 0, 0, 0], 20),
            usage_trace("other", "p2", [1, 1, 1, 1], 30),
            usage_trace("m", "p3", [1, 1, 1, 1], 5_000),
        ];
        let priced = table(2);
        let page = run_trace_usage(&usage_query(None, 10.0), &traces, Some(&priced), 7).unwrap();
        let groups: Vec<&str> = page.rows.iter().map(|row| row.group.as_str()).collect();
        // The trace outside the window is in no group.
        assert_eq!(groups, ["p1", "p2"]);
        let p1 = &page.rows[0];
        assert_eq!(p1.values[0], Some(500_000.0));
        assert_eq!(p1.values[5], Some(1.0));
        assert_eq!(p1.values[9], Some(1.0));
        // A model without a rate: tokens known, cost unknown with a reason.
        let p2 = &page.rows[1];
        assert_eq!(p2.values[4], Some(4.0));
        assert_eq!(p2.values[9], None);
        assert!(p2.unknown.contains(&("total_cost_minor", PRICE_UNKNOWN)));
        assert!(page
            .warnings
            .iter()
            .any(|w| w.contains("PRICE_UNKNOWN: p2|")));

        // Half to even at the boundary: 2.5 rounds to 2, 3.5 to 4.
        let half = vec![usage_trace("m", "p", [2_500_000, 0, 0, 0], 1)];
        let page = run_trace_usage(&usage_query(None, 10.0), &half, Some(&table(1)), 7).unwrap();
        assert_eq!(page.rows[0].values[5], Some(2.0));
        let half = vec![usage_trace("m", "p", [3_500_000, 0, 0, 0], 1)];
        let page = run_trace_usage(&usage_query(None, 10.0), &half, Some(&table(1)), 7).unwrap();
        assert_eq!(page.rows[0].values[5], Some(4.0));

        // No visible table: every cost is unknown, tokens stay known.
        let page = run_trace_usage(&usage_query(None, 10.0), &traces, None, 7).unwrap();
        assert!(page.rows.iter().all(|row| row.values[9].is_none()));
        assert!(page.rows[0]
            .unknown
            .contains(&("total_cost_minor", PRICE_TABLE_UNKNOWN)));
    }

    // spec: MEDULLA-ORCH-VOCAB-R010.101
    #[test]
    fn usage_pages_by_snapshot_cursor_and_refuses_a_stale_one() {
        let traces = vec![
            usage_trace("m", "a", [1, 1, 1, 1], 1),
            usage_trace("m", "b", [1, 1, 1, 1], 1),
            usage_trace("m", "c", [1, 1, 1, 1], 1),
        ];
        let first = run_trace_usage(&usage_query(None, 2.0), &traces, None, 9).unwrap();
        assert_eq!(first.rows.len(), 2);
        let cursor = UsageCursor {
            snapshot_revision: 9,
            after_group: first.rows[1].group.clone(),
        }
        .encode();
        let rest = run_trace_usage(&usage_query(Some(&cursor), 2.0), &traces, None, 9).unwrap();
        assert_eq!(rest.rows.len(), 1);
        assert_eq!(rest.rows[0].group, "c");
        let stale =
            run_trace_usage(&usage_query(Some(&cursor), 2.0), &traces, None, 10).unwrap_err();
        assert!(stale.starts_with("USAGE_CURSOR_STALE"), "{stale}");
    }
}
