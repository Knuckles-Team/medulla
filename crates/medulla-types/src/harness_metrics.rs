//! Harness metric statements (MEDULLA-ORCH-VOCAB-R010.1 to R010.5): the closed
//! `HARNESS_QUERIES` table served through `Method::Uql`, and the pure aggregation
//! each statement runs over the `RunTrace` nodes the caller can see.
//!
//! A statement divides counts per group and never invents a number: a group with
//! no solved trace abstains instead of reporting zero cost, and a trace with no
//! usable measurement is excluded from its mean and counted.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Refusal reason of a cost group without a solved trace.
pub const NO_SOLVED_TRACE: &str = "NO_SOLVED_TRACE";

/// The property a `RunTrace` group key reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKey {
    Model,
    TaskClass,
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
    pub name: &'static str,
    pub groups: &'static [GroupKey],
    pub columns: &'static [&'static str],
}

/// The closed statement table; a text outside it is not a harness statement.
pub const HARNESS_QUERIES: [HarnessQuery; 5] = [
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
    pub group: String,
    pub values: Vec<Option<f64>>,
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
        assert_eq!(HARNESS_QUERIES.len(), 5);
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
}
