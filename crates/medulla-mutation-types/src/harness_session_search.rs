//! The harness session search statement (MEDULLA-ORCH-VOCAB-R009.101.2): a read over the
//! index nodes of R009.101.1 (`harnesssession:*`, `harnesssync:*`, `harnesssessionindex:*`)
//! and the committed `RunTrace` nodes they link. The first page begins with one `summary` row
//! (the manifest counts, the watermark and the cost totals), then one `entry` row per session
//! ordered by `(source_sequence desc, source_session_id asc)`. A declared session that has not
//! arrived stays in the denominator as `pending`; an unavailable source stays in `declared`
//! and `received`. A cost total is `unknown` with a reason, never 0.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::benchmark_claim::PriceTable;
use crate::harness_metrics::{
    price_table_from_node, run_trace_usage, RunTraceUsageQuery, COUNTER_UNKNOWN,
};

/// The statement name.
pub const HARNESS_SESSION_SEARCH: &str = "harness_session_search";
/// The refusal of a cursor minted at another index revision, tenant or filter.
pub const SESSION_CURSOR_STALE: &str = "SESSION_CURSOR_STALE";
/// Longest `text` filter.
pub const MAX_TEXT_CHARS: usize = 256;

/// The columns of a row; a row fills the columns of its `kind` and leaves the rest empty.
pub const SESSION_SEARCH_COLUMNS: [&str; 24] = [
    "kind",
    "index_revision",
    "declared",
    "received",
    "indexed",
    "unavailable",
    "pending",
    "watermark",
    "cost_status",
    "cost_reason",
    "uncached_input",
    "cached_input",
    "output",
    "reasoning_output",
    "cost_minor",
    "currency",
    "accounting_revision",
    "harness_id",
    "source_session_id",
    "status",
    "reason",
    "run_trace_id",
    "end_ms",
    "source_sequence",
];

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("PARAMETER_INVALID: {detail}")
}

fn text<'a>(props: &'a Map<String, Value>, key: &str) -> &'a str {
    props.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn number(props: &Map<String, Value>, key: &str) -> Option<u64> {
    match props.get(key)? {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// Where a page resumes: after the entry `(source_sequence, source_session_id)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCursor {
    index_revision: u64,
    filters: String,
    sequence: u64,
    session_id: String,
}

impl SessionCursor {
    fn encode(&self) -> String {
        format!(
            "sessioncursor:v1:{}:{}:{}:{}",
            self.index_revision, self.filters, self.sequence, self.session_id
        )
    }

    fn decode(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("sessioncursor:v1:")?;
        let mut parts = rest.splitn(4, ':');
        Some(Self {
            index_revision: parts.next()?.parse().ok()?,
            filters: parts.next()?.to_string(),
            sequence: parts.next()?.parse().ok()?,
            session_id: parts.next()?.to_string(),
        })
    }
}

/// One search request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSearchQuery {
    text: Option<String>,
    harness_id: Option<String>,
    from_ms: Option<u64>,
    to_ms: Option<u64>,
    page_size: usize,
    cursor: Option<SessionCursor>,
}

impl SessionSearchQuery {
    /// Decode the statement's parameters; `page_size_max` is the knob
    /// `orchestration_limits.page_size_max`. A cursor that does not decode is
    /// `SESSION_CURSOR_STALE`.
    pub fn from_params(
        text_param: impl Fn(&str) -> Option<String>,
        number_param: impl Fn(&str) -> Option<f64>,
        page_size_max: u64,
    ) -> Result<Self, String> {
        let whole = |name: &str| -> Result<Option<u64>, String> {
            match number_param(name) {
                None => Ok(None),
                Some(value) if value.fract() == 0.0 && value >= 0.0 && value < u64::MAX as f64 => {
                    Ok(Some(value as u64))
                }
                Some(_) => Err(invalid(format!("${name} must be a non-negative integer"))),
            }
        };
        let search = text_param("text").map(|text| text.to_lowercase());
        if search
            .as_ref()
            .is_some_and(|text| text.chars().count() > MAX_TEXT_CHARS)
        {
            return Err(invalid(format!(
                "$text holds at most {MAX_TEXT_CHARS} characters"
            )));
        }
        let page_size = match whole("page_size")? {
            None => page_size_max,
            Some(size) => size.clamp(1, page_size_max),
        } as usize;
        let cursor = match text_param("cursor") {
            None => None,
            Some(cursor) => Some(SessionCursor::decode(&cursor).ok_or_else(|| {
                format!("{SESSION_CURSOR_STALE}: the cursor is not a session search cursor")
            })?),
        };
        Ok(Self {
            text: search,
            harness_id: text_param("harness_id"),
            from_ms: whole("from_ms")?,
            to_ms: whole("to_ms")?,
            page_size,
            cursor,
        })
    }

    /// The digest of the filters a cursor binds to.
    fn filters_digest(&self) -> String {
        let text = format!(
            "{}|{}|{:?}|{:?}",
            self.text.as_deref().unwrap_or_default(),
            self.harness_id.as_deref().unwrap_or_default(),
            self.from_ms,
            self.to_ms
        );
        hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
    }
}

/// One row: its id and its columns, each as text (a column that does not apply to the row's
/// kind is absent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSearchRow {
    /// The id of the row.
    pub id: String,
    /// The columns of the row by name, each as text.
    pub columns: BTreeMap<&'static str, String>,
}

/// One page of the search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSearchPage {
    /// The rows of this page.
    pub rows: Vec<SessionSearchRow>,
    /// Notes on the page, such as a filter that could not apply.
    pub warnings: Vec<String>,
    /// The cursor of the next page; `None` on the last.
    pub next_cursor: Option<String>,
}

struct Session<'a> {
    id: &'a str,
    props: &'a Map<String, Value>,
    end_ms: Option<u64>,
}

/// The cost columns of the summary: measured totals, or `unknown` with its reason.
fn cost_columns(
    traces: &[&Map<String, Value>],
    tables: &BTreeMap<String, PriceTable>,
) -> BTreeMap<&'static str, String> {
    let mut columns = BTreeMap::new();
    let unknown = |reason: &str, columns: &mut BTreeMap<&'static str, String>| {
        columns.insert("cost_status", "unknown".to_string());
        columns.insert("cost_reason", reason.to_string());
    };
    let revisions: BTreeSet<&str> = traces
        .iter()
        .filter_map(|trace| trace.get("accounting_revision").and_then(Value::as_str))
        .collect();
    if traces.is_empty()
        || revisions.len() != 1
        || traces
            .iter()
            .any(|trace| trace.get("accounting_revision").is_none())
    {
        unknown("attribution_missing", &mut columns);
        return columns;
    }
    let revision = *revisions.iter().next().expect("one revision");
    let Some(table) = tables.get(revision) else {
        unknown("price_missing", &mut columns);
        return columns;
    };
    let query = RunTraceUsageQuery {
        schema_version: crate::harness_metrics::RUN_TRACE_USAGE_QUERY_SCHEMA_VERSION,
        window_start_ms: 0,
        window_end_ms: 2,
        grouping: Vec::new(),
        currency: table.currency.clone(),
        price_table_revision: revision.to_string(),
        page_size: 1,
        cursor: None,
    };
    // The one aggregation `run_trace_usage` runs: every matching trace joins the one group.
    let timed: Vec<Map<String, Value>> = traces
        .iter()
        .map(|trace| {
            let mut copy = (*trace).clone();
            copy.insert("completedAtMs".to_string(), Value::from(1u64));
            copy
        })
        .collect();
    let Ok(page) = run_trace_usage(&query, &timed, Some(table), 0) else {
        unknown("price_missing", &mut columns);
        return columns;
    };
    let Some(row) = page.rows.first() else {
        unknown("attribution_missing", &mut columns);
        return columns;
    };
    if let Some((_, reason)) = row.unknown.first() {
        let mapped = if *reason == COUNTER_UNKNOWN {
            "attribution_missing"
        } else {
            "price_missing"
        };
        unknown(mapped, &mut columns);
        return columns;
    }
    let value = |index: usize| {
        row.values[index]
            .map(|v| (v as u64).to_string())
            .unwrap_or_default()
    };
    columns.insert("cost_status", "measured".to_string());
    for (name, index) in [
        ("uncached_input", 0),
        ("cached_input", 1),
        ("output", 2),
        ("reasoning_output", 3),
        ("cost_minor", 9),
    ] {
        columns.insert(name, value(index));
    }
    columns.insert("currency", table.currency.clone());
    columns.insert("accounting_revision", revision.to_string());
    columns
}

/// Answer one search over the caller's visible `nodes` (id and properties of every node).
pub fn harness_session_search(
    query: &SessionSearchQuery,
    nodes: &[(String, Map<String, Value>)],
) -> Result<SessionSearchPage, String> {
    let mut sessions: Vec<Session> = Vec::new();
    let mut receipts: Vec<&Map<String, Value>> = Vec::new();
    let mut traces: BTreeMap<&str, &Map<String, Value>> = BTreeMap::new();
    let mut tables: BTreeMap<String, PriceTable> = BTreeMap::new();
    let mut index_revision = 0u64;
    for (id, props) in nodes {
        let class = [text(props, "type"), text(props, "node_type")];
        if class.contains(&"HarnessSession") {
            sessions.push(Session {
                id,
                props,
                end_ms: None,
            });
        } else if class.contains(&"HarnessSync") {
            receipts.push(props);
        } else if class.contains(&"HarnessSessionIndex") {
            index_revision = index_revision.max(number(props, "revision").unwrap_or(0));
        } else if class.contains(&"RunTrace") {
            traces.insert(id, props);
        } else if let Some((revision, table)) = price_table_from_node(props) {
            tables.insert(revision, table);
        }
    }
    let filters = query.filters_digest();
    if let Some(cursor) = &query.cursor {
        if cursor.index_revision != index_revision || cursor.filters != filters {
            return Err(format!(
                "{SESSION_CURSOR_STALE}: the cursor was minted at index revision {} for other filters or the index moved to {index_revision}",
                cursor.index_revision
            ));
        }
    }
    for session in &mut sessions {
        session.end_ms = traces
            .get(text(session.props, "run_trace_id"))
            .and_then(|trace| trace.get("endMs").and_then(Value::as_u64));
    }
    let windowed = query.from_ms.is_some() || query.to_ms.is_some();
    let mut matching: Vec<&Session> = sessions
        .iter()
        .filter(|session| {
            let harness = text(session.props, "harness_id");
            let source = text(session.props, "source_session_id");
            let text_ok = query.text.as_ref().is_none_or(|needle| {
                harness.to_lowercase().contains(needle) || source.to_lowercase().contains(needle)
            });
            let harness_ok = query
                .harness_id
                .as_deref()
                .is_none_or(|wanted| wanted == harness);
            let window_ok = !windowed
                || session.end_ms.is_some_and(|end| {
                    query.from_ms.is_none_or(|from| end >= from)
                        && query.to_ms.is_none_or(|to| end < to)
                });
            text_ok && harness_ok && window_ok
        })
        .collect();
    matching.sort_by(|a, b| {
        number(b.props, "source_sequence")
            .unwrap_or(0)
            .cmp(&number(a.props, "source_sequence").unwrap_or(0))
            .then_with(|| {
                text(a.props, "source_session_id").cmp(text(b.props, "source_session_id"))
            })
    });
    let mut rows = Vec::new();
    if query.cursor.is_none() {
        // The newest receipt of each manifest, by the index revision it was written at.
        let mut newest: BTreeMap<&str, &Map<String, Value>> = BTreeMap::new();
        for receipt in &receipts {
            let manifest = text(receipt, "manifest_id");
            let better = newest.get(manifest).is_none_or(|held| {
                number(receipt, "index_revision") > number(held, "index_revision")
            });
            if better {
                newest.insert(manifest, receipt);
            }
        }
        let declared: u64 = newest
            .values()
            .filter_map(|r| number(r, "declared_total"))
            .sum();
        let watermark = receipts
            .iter()
            .max_by_key(|receipt| number(receipt, "index_revision"))
            .map(|receipt| text(receipt, "source_cursor").to_string())
            .unwrap_or_default();
        let received = matching.len() as u64;
        let indexed = matching
            .iter()
            .filter(|session| text(session.props, "status") == "indexed")
            .count() as u64;
        let unavailable = received - indexed;
        let indexed_traces: Vec<&Map<String, Value>> = matching
            .iter()
            .filter(|session| text(session.props, "status") == "indexed")
            .filter_map(|session| traces.get(text(session.props, "run_trace_id")).copied())
            .collect();
        let mut columns = cost_columns(&indexed_traces, &tables);
        columns.insert("kind", "summary".to_string());
        columns.insert("index_revision", index_revision.to_string());
        columns.insert("declared", declared.to_string());
        columns.insert("received", received.to_string());
        columns.insert("indexed", indexed.to_string());
        columns.insert("unavailable", unavailable.to_string());
        columns.insert("pending", declared.saturating_sub(received).to_string());
        columns.insert("watermark", watermark);
        rows.push(SessionSearchRow {
            id: "summary".to_string(),
            columns,
        });
    }
    let after = query.cursor.as_ref();
    let mut remaining: Vec<&&Session> = matching
        .iter()
        .filter(|session| {
            after.is_none_or(|cursor| {
                let sequence = number(session.props, "source_sequence").unwrap_or(0);
                let id = text(session.props, "source_session_id");
                sequence < cursor.sequence
                    || (sequence == cursor.sequence && id > cursor.session_id.as_str())
            })
        })
        .collect();
    let more = remaining.len() > query.page_size;
    remaining.truncate(query.page_size);
    let mut last = None;
    for session in remaining {
        let props = session.props;
        let sequence = number(props, "source_sequence").unwrap_or(0);
        let mut columns: BTreeMap<&'static str, String> = BTreeMap::new();
        columns.insert("kind", "entry".to_string());
        columns.insert("harness_id", text(props, "harness_id").to_string());
        columns.insert(
            "source_session_id",
            text(props, "source_session_id").to_string(),
        );
        columns.insert("status", text(props, "status").to_string());
        columns.insert("reason", text(props, "reason").to_string());
        columns.insert("run_trace_id", text(props, "run_trace_id").to_string());
        columns.insert(
            "end_ms",
            session
                .end_ms
                .map(|end| end.to_string())
                .unwrap_or_default(),
        );
        columns.insert("source_sequence", sequence.to_string());
        last = Some((sequence, text(props, "source_session_id").to_string()));
        rows.push(SessionSearchRow {
            id: session.id.to_string(),
            columns,
        });
    }
    let next_cursor = match (more, last) {
        (true, Some((sequence, session_id))) => Some(
            SessionCursor {
                index_revision,
                filters,
                sequence,
                session_id,
            }
            .encode(),
        ),
        _ => None,
    };
    Ok(SessionSearchPage {
        rows,
        warnings: Vec::new(),
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    fn session(id: &str, status: &str, sequence: u64, trace: &str) -> (String, Map<String, Value>) {
        (
            format!("harnesssession:t:h:{id}"),
            object(json!({
                "type": "HarnessSession", "status": status, "harness_id": "claude-code",
                "source_session_id": id, "run_trace_id": trace,
                "source_sequence": sequence.to_string(), "reason": if status == "unavailable" { "gone" } else { "" },
            })),
        )
    }

    fn fixture(declared: u64, with_price: bool) -> Vec<(String, Map<String, Value>)> {
        let mut nodes = vec![
            session("s-1", "indexed", 9, "run:1"),
            session("s-2", "indexed", 7, "run:2"),
            session("s-3", "unavailable", 0, ""),
            (
                "harnesssync:t:r1".into(),
                object(
                    json!({"type": "HarnessSync", "manifest_id": "m", "declared_total": declared.to_string(),
                    "source_cursor": "cursor-9", "index_revision": "3"}),
                ),
            ),
            (
                "harnesssessionindex:t".into(),
                object(json!({"type": "HarnessSessionIndex", "revision": "3"})),
            ),
        ];
        for (id, end) in [("run:1", 100u64), ("run:2", 200)] {
            let mut trace = object(json!({
                "node_type": "RunTrace", "endMs": end, "runModel": "m1",
                "uncachedInputTokens": 1000, "cachedInputTokens": 0,
                "outputTokens": 500, "reasoningOutputTokens": 0,
            }));
            if with_price {
                trace.insert("accounting_revision".into(), json!("prices-1"));
            }
            nodes.push((id.to_string(), trace));
        }
        nodes
    }

    /// The `PriceTable` node pinned at revision `prices-1`: `model` costs `rate` minor units
    /// per million tokens in every category (the node JSON `price_table_from_node` reads).
    fn price_table_node(model: &str, rate: u64) -> (String, Map<String, Value>) {
        let table = json!({
            "schema_version": 1, "version": 1, "currency": "USD", "minor_unit_exponent": 2,
            "token_rates": {model: {"uncached_input": rate, "cached_input": rate,
                                    "output": rate, "reasoning_output": rate}},
        });
        (
            "pricetable:t:prices-1".into(),
            object(json!({
                "node_type": "PriceTable", "revision": "prices-1", "table": table.to_string(),
            })),
        )
    }

    fn query(params: &[(&str, &str)], numbers: &[(&str, f64)]) -> SessionSearchQuery {
        SessionSearchQuery::from_params(
            |name| {
                params
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            },
            |name| numbers.iter().find(|(k, _)| *k == name).map(|(_, v)| *v),
            100,
        )
        .unwrap()
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.2
    #[test]
    fn manifest_counts_keep_the_unavailable_source_and_the_pending_remainder() {
        let page = harness_session_search(&query(&[], &[]), &fixture(3, true)).unwrap();
        let summary = &page.rows[0].columns;
        assert_eq!(summary["kind"], "summary");
        for (name, expected) in [
            ("declared", "3"),
            ("received", "3"),
            ("indexed", "2"),
            ("unavailable", "1"),
            ("pending", "0"),
            ("watermark", "cursor-9"),
            ("index_revision", "3"),
        ] {
            assert_eq!(summary[name], expected, "{name}");
        }
        // A manifest declaring 4 leaves one pending.
        let four = harness_session_search(&query(&[], &[]), &fixture(4, true)).unwrap();
        assert_eq!(four.rows[0].columns["pending"], "1");
        // Entries order by sequence descending, then session id.
        let ids: Vec<&str> = page.rows[1..]
            .iter()
            .map(|row| row.columns["source_session_id"].as_str())
            .collect();
        assert_eq!(ids, ["s-1", "s-2", "s-3"]);
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.2
    #[test]
    fn cost_is_unknown_not_zero_and_a_stale_cursor_refuses() {
        let unpriced = harness_session_search(&query(&[], &[]), &fixture(3, false)).unwrap();
        let summary = &unpriced.rows[0].columns;
        assert_eq!(summary["cost_status"], "unknown");
        assert_eq!(summary["cost_reason"], "attribution_missing");
        assert!(!summary.contains_key("cost_minor"));
        // Accounted but no pinned price table: price_missing.
        let no_table = harness_session_search(&query(&[], &[]), &fixture(3, true)).unwrap();
        assert_eq!(no_table.rows[0].columns["cost_reason"], "price_missing");
        // A page cursor binds the index revision: a moved index refuses.
        let paged =
            harness_session_search(&query(&[], &[("page_size", 1.0)]), &fixture(3, true)).unwrap();
        let cursor = paged.next_cursor.clone().expect("more pages");
        let resumed = query(&[("cursor", &cursor)], &[("page_size", 1.0)]);
        assert!(harness_session_search(&resumed, &fixture(3, true)).is_ok());
        let mut moved = fixture(3, true);
        moved
            .iter_mut()
            .find(|(id, _)| id == "harnesssessionindex:t")
            .unwrap()
            .1
            .insert("revision".into(), json!("4"));
        let stale = harness_session_search(&resumed, &moved).unwrap_err();
        assert!(stale.starts_with("SESSION_CURSOR_STALE"), "{stale}");
        // A cursor of other filters refuses too.
        let other = query(&[("cursor", &cursor), ("harness_id", "other")], &[]);
        assert!(harness_session_search(&other, &fixture(3, true))
            .unwrap_err()
            .starts_with("SESSION_CURSOR_STALE"));
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.2
    #[test]
    fn text_harness_and_window_filter_the_entries() {
        let by_text =
            harness_session_search(&query(&[("text", "S-2")], &[]), &fixture(3, true)).unwrap();
        assert_eq!(by_text.rows.len(), 2);
        assert_eq!(by_text.rows[1].columns["source_session_id"], "s-2");
        let by_window = harness_session_search(
            &query(&[], &[("from_ms", 150.0), ("to_ms", 300.0)]),
            &fixture(3, true),
        )
        .unwrap();
        // Only the session whose trace ended in [150, 300) remains; the unavailable one has no trace.
        let ids: Vec<&str> = by_window.rows[1..]
            .iter()
            .map(|r| r.columns["source_session_id"].as_str())
            .collect();
        assert_eq!(ids, ["s-2"]);
        assert_eq!(by_window.rows[0].columns["received"], "1");
        let none =
            harness_session_search(&query(&[("harness_id", "absent")], &[]), &fixture(3, true))
                .unwrap();
        assert_eq!(none.rows.len(), 1);
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.2
    #[test]
    fn measured_cost_prices_the_matching_traces_against_the_pinned_table() {
        // Two traces of 1000 uncached and 500 output tokens each, at 2_000_000 minor units per
        // million tokens: 2000 * 2 + 1000 * 2 = 6000, rounded once for the one group.
        let mut nodes = fixture(3, true);
        nodes.push(price_table_node("m1", 2_000_000));
        let page = harness_session_search(&query(&[], &[]), &nodes).unwrap();
        let summary = &page.rows[0].columns;
        assert_eq!(summary["cost_status"], "measured");
        assert_eq!(summary["uncached_input"], "2000");
        assert_eq!(summary["cached_input"], "0");
        assert_eq!(summary["output"], "1000");
        assert_eq!(summary["reasoning_output"], "0");
        assert_eq!(summary["cost_minor"], "6000");
        assert_eq!(summary["currency"], "USD");
        assert_eq!(summary["accounting_revision"], "prices-1");
        assert!(!summary.contains_key("cost_reason"));

        // A table with no rate for the traces' model is price_missing, never a zero cost.
        let mut other_model = fixture(3, true);
        other_model.push(price_table_node("another-model", 2_000_000));
        let page = harness_session_search(&query(&[], &[]), &other_model).unwrap();
        assert_eq!(page.rows[0].columns["cost_status"], "unknown");
        assert_eq!(page.rows[0].columns["cost_reason"], "price_missing");
        assert!(!page.rows[0].columns.contains_key("cost_minor"));

        // A trace that reports no counter is attribution_missing, even with a table pinned.
        let mut uncounted = fixture(3, true);
        uncounted.push(price_table_node("m1", 2_000_000));
        uncounted
            .iter_mut()
            .find(|(id, _)| id == "run:2")
            .unwrap()
            .1
            .remove("outputTokens");
        let page = harness_session_search(&query(&[], &[]), &uncounted).unwrap();
        assert_eq!(page.rows[0].columns["cost_status"], "unknown");
        assert_eq!(page.rows[0].columns["cost_reason"], "attribution_missing");
    }
}
