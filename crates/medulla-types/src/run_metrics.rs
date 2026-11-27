//! The run metrics read (MEDULLA-ORCH-VOCAB-R811.1).
//!
//! `RunMetrics` aggregates the measurements the engine already holds on the `:RunTrace` nodes of
//! the caller's tenant: cost, tokens and solved count. It is a read of the caller's projected
//! graph; it writes nothing and keeps no store of its own. Cost and tokens are reported per
//! solved run, so a population that solved nothing has no cost to report: it abstains rather
//! than answering zero.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A run whose `terminationReason` is this one is solved.
pub const SOLVED_TERMINATION: &str = "final_answer";

/// The abstention of a population in which no run is solved.
pub const ABSTAIN_NO_SOLVED_RUNS: &str = "NO_SOLVED_RUNS";
/// The abstention of a population in which no solved run reports the measured counter.
pub const ABSTAIN_NO_MEASUREMENTS: &str = "NO_MEASUREMENTS";

/// What the read measures. The set is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunMetricsQuery {
    /// Cost in USD per solved run.
    Cost,
    /// Tokens (uncached input, cached input and output) per solved run.
    Tokens,
    /// The count of solved runs.
    Solved,
    /// Mean wall-clock latency in milliseconds of the solved runs that report one
    /// (`latencyMs`).
    Latency,
    /// Mean benchmark score of the solved runs that report one (`benchmarkScore`).
    Score,
}

/// How the population is grouped. The set is closed; with none, one group `all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunMetricsGroupBy {
    /// By `agentId`.
    Agent,
    /// By `projectId`.
    Project,
    /// By `terminationReason`.
    Termination,
    /// By `techniqueId`.
    Technique,
    /// By `armId`.
    Arm,
    /// By `configDigest`.
    Config,
}

/// A half-open range `[start_ms, end_ms)` over `completedAtMs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunMetricsWindow {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// A served `RunMetrics` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunMetricsRequest {
    /// The verified tenant; any other tenant refuses `ACCESS_DENIED`.
    pub tenant: String,
    pub query: RunMetricsQuery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<RunMetricsGroupBy>,
    /// Only runs completed inside the window count; with none, every run does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<RunMetricsWindow>,
    /// Only runs of this technique (`techniqueId`) count. A technique measure is
    /// the three filters together: the technique, its arm and the digest of the
    /// configuration the arm ran under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technique: Option<String>,
    /// Only runs of this arm (`armId`) count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    /// Only runs under this configuration digest (`configDigest`) count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_digest: Option<String>,
}

/// The longest technique, arm or configuration digest a filter may name.
pub const MAX_FILTER_BYTES: usize = 256;

impl RunMetricsRequest {
    /// Refuse a request outside its bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.tenant.is_empty() {
            return Err("INVALID_ARGUMENT: a run metrics request names its tenant".into());
        }
        if let Some(window) = self.window {
            if window.start_ms >= window.end_ms {
                return Err("INVALID_ARGUMENT: a run metrics window must not be empty".into());
            }
        }
        for (name, value) in [
            ("technique", &self.technique),
            ("arm", &self.arm),
            ("config_digest", &self.config_digest),
        ] {
            if let Some(value) = value {
                if value.is_empty() || value.len() > MAX_FILTER_BYTES {
                    return Err(format!(
                        "INVALID_ARGUMENT: run metrics {name} is 1 to {MAX_FILTER_BYTES} bytes"
                    ));
                }
            }
        }
        Ok(())
    }

    fn admits(&self, sample: &RunSample) -> bool {
        let matches = |wanted: &Option<String>, actual: &str| {
            wanted.as_deref().map_or(true, |wanted| wanted == actual)
        };
        matches(&self.technique, &sample.technique)
            && matches(&self.arm, &sample.arm)
            && matches(&self.config_digest, &sample.config_digest)
    }
}

/// One group of the answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunMetricsGroup {
    pub key: String,
    /// Never negative.
    pub value: f64,
    /// The runs of the group, solved or not.
    pub trace_count: u64,
}

/// The served `RunMetrics` answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunMetricsAnswer {
    pub query: RunMetricsQuery,
    pub groups: Vec<RunMetricsGroup>,
    /// Present, with no groups, when the population cannot answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abstained: Option<String>,
}

/// The measurements of one `:RunTrace`, read from its node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunSample {
    pub agent: String,
    pub project: String,
    pub termination: String,
    pub completed_at_ms: Option<u64>,
    pub cost_usd: Option<f64>,
    /// The sum of the token counters the run reports, `None` when it reports none.
    pub tokens: Option<u64>,
    /// The technique, arm and configuration digest the run measured, empty when unlabelled.
    pub technique: String,
    pub arm: String,
    pub config_digest: String,
    /// The run's wall-clock latency, `None` when it reports none.
    pub latency_ms: Option<f64>,
    /// The run's benchmark score, `None` when it reports none.
    pub score: Option<f64>,
}

impl RunSample {
    fn solved(&self) -> bool {
        self.termination == SOLVED_TERMINATION
    }

    fn key(&self, group_by: Option<RunMetricsGroupBy>) -> String {
        let key = match group_by {
            None => "all",
            Some(RunMetricsGroupBy::Agent) => self.agent.as_str(),
            Some(RunMetricsGroupBy::Project) => self.project.as_str(),
            Some(RunMetricsGroupBy::Termination) => self.termination.as_str(),
            Some(RunMetricsGroupBy::Technique) => self.technique.as_str(),
            Some(RunMetricsGroupBy::Arm) => self.arm.as_str(),
            Some(RunMetricsGroupBy::Config) => self.config_digest.as_str(),
        };
        let key = if key.is_empty() { "unknown" } else { key };
        key.to_string()
    }
}

#[derive(Default)]
struct Tally {
    runs: u64,
    solved: u64,
    cost: f64,
    cost_measured: u64,
    tokens: u64,
    tokens_measured: u64,
    latency: f64,
    latency_measured: u64,
    score: f64,
    score_measured: u64,
}

/// Aggregate `samples` for `request`. The one implementation: the handler only reads the nodes
/// into samples. A group with no solved run (or no measurement of the asked counter on a solved
/// run) is left out; a population with none answers `abstained`.
pub fn aggregate(
    request: &RunMetricsRequest,
    samples: impl IntoIterator<Item = RunSample>,
) -> RunMetricsAnswer {
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for sample in samples {
        if !request.admits(&sample) {
            continue;
        }
        if let Some(window) = request.window {
            match sample.completed_at_ms {
                Some(at) if (window.start_ms..window.end_ms).contains(&at) => {}
                _ => continue,
            }
        }
        let solved = sample.solved();
        let tally = tallies.entry(sample.key(request.group_by)).or_default();
        tally.runs += 1;
        if solved {
            tally.solved += 1;
            if let Some(cost) = sample.cost_usd {
                tally.cost += cost.max(0.0);
                tally.cost_measured += 1;
            }
            if let Some(tokens) = sample.tokens {
                tally.tokens = tally.tokens.saturating_add(tokens);
                tally.tokens_measured += 1;
            }
            if let Some(latency) = sample.latency_ms {
                tally.latency += latency.max(0.0);
                tally.latency_measured += 1;
            }
            if let Some(score) = sample.score {
                tally.score += score;
                tally.score_measured += 1;
            }
        }
    }
    let answer = |groups: Vec<RunMetricsGroup>, abstained: Option<&str>| RunMetricsAnswer {
        query: request.query,
        groups,
        abstained: abstained.map(str::to_string),
    };
    if tallies.values().all(|tally| tally.solved == 0) {
        return answer(Vec::new(), Some(ABSTAIN_NO_SOLVED_RUNS));
    }
    let groups: Vec<RunMetricsGroup> = tallies
        .into_iter()
        .filter(|(_, tally)| tally.solved > 0)
        .filter_map(|(key, tally)| {
            let value = match request.query {
                RunMetricsQuery::Solved => tally.solved as f64,
                RunMetricsQuery::Cost => {
                    (tally.cost_measured > 0).then(|| tally.cost / tally.solved as f64)?
                }
                RunMetricsQuery::Tokens => (tally.tokens_measured > 0)
                    .then(|| tally.tokens as f64 / tally.solved as f64)?,
                RunMetricsQuery::Latency => (tally.latency_measured > 0)
                    .then(|| tally.latency / tally.latency_measured as f64)?,
                RunMetricsQuery::Score => {
                    (tally.score_measured > 0).then(|| tally.score / tally.score_measured as f64)?
                }
            };
            Some(RunMetricsGroup {
                key,
                value,
                trace_count: tally.runs,
            })
        })
        .collect();
    if groups.is_empty() {
        return answer(groups, Some(ABSTAIN_NO_MEASUREMENTS));
    }
    answer(groups, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        agent: &str,
        termination: &str,
        at: u64,
        cost: Option<f64>,
        tokens: Option<u64>,
    ) -> RunSample {
        RunSample {
            agent: agent.into(),
            project: "p".into(),
            termination: termination.into(),
            completed_at_ms: Some(at),
            cost_usd: cost,
            tokens,
            ..RunSample::default()
        }
    }

    fn request(query: RunMetricsQuery) -> RunMetricsRequest {
        RunMetricsRequest {
            tenant: "tenant-a".into(),
            query,
            group_by: Some(RunMetricsGroupBy::Agent),
            window: None,
            technique: None,
            arm: None,
            config_digest: None,
        }
    }

    fn population() -> Vec<RunSample> {
        vec![
            sample("a", "final_answer", 10, Some(1.0), Some(100)),
            sample("a", "final_answer", 20, Some(3.0), Some(300)),
            sample("a", "error", 30, Some(5.0), Some(500)),
            sample("b", "final_answer", 40, None, Some(50)),
            sample("c", "stuck", 50, Some(9.0), Some(900)),
        ]
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.1
    #[test]
    fn cost_and_tokens_are_per_solved_run_and_a_group_without_a_solve_is_left_out() {
        let cost = aggregate(&request(RunMetricsQuery::Cost), population());
        assert_eq!(cost.abstained, None);
        // Agent a: (1 + 3) / 2 solved; its failed run's cost is not charged to a solve.
        assert_eq!(
            cost.groups,
            [RunMetricsGroup {
                key: "a".into(),
                value: 2.0,
                trace_count: 3
            }]
        );
        let tokens = aggregate(&request(RunMetricsQuery::Tokens), population());
        assert_eq!(
            tokens.groups,
            [
                RunMetricsGroup {
                    key: "a".into(),
                    value: 200.0,
                    trace_count: 3
                },
                RunMetricsGroup {
                    key: "b".into(),
                    value: 50.0,
                    trace_count: 1
                },
            ]
        );
        let solved = aggregate(&request(RunMetricsQuery::Solved), population());
        assert_eq!(
            solved.groups.iter().map(|g| g.value).collect::<Vec<_>>(),
            [2.0, 1.0]
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.1
    #[test]
    fn a_population_without_a_solved_run_abstains_and_never_reports_a_zero_cost() {
        let only_failures = vec![sample("a", "error", 10, Some(5.0), Some(5))];
        for query in [
            RunMetricsQuery::Cost,
            RunMetricsQuery::Tokens,
            RunMetricsQuery::Solved,
        ] {
            let answer = aggregate(&request(query), only_failures.clone());
            assert!(answer.groups.is_empty());
            assert_eq!(answer.abstained.as_deref(), Some(ABSTAIN_NO_SOLVED_RUNS));
        }
        assert_eq!(
            aggregate(&request(RunMetricsQuery::Solved), Vec::new())
                .abstained
                .as_deref(),
            Some(ABSTAIN_NO_SOLVED_RUNS)
        );
        // Solved runs that report no cost abstain rather than answer zero.
        let unmeasured = vec![sample("a", "final_answer", 10, None, None)];
        let answer = aggregate(&request(RunMetricsQuery::Cost), unmeasured);
        assert!(answer.groups.is_empty());
        assert_eq!(answer.abstained.as_deref(), Some(ABSTAIN_NO_MEASUREMENTS));
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.1
    #[test]
    fn the_window_keeps_only_runs_completed_inside_it_and_an_empty_one_is_refused() {
        let mut windowed = request(RunMetricsQuery::Solved);
        windowed.window = Some(RunMetricsWindow {
            start_ms: 15,
            end_ms: 45,
        });
        let answer = aggregate(&windowed, population());
        assert_eq!(
            answer
                .groups
                .iter()
                .map(|g| (g.key.as_str(), g.trace_count))
                .collect::<Vec<_>>(),
            [("a", 2), ("b", 1)]
        );
        windowed.window = Some(RunMetricsWindow {
            start_ms: 5,
            end_ms: 5,
        });
        assert!(windowed
            .validate()
            .unwrap_err()
            .starts_with("INVALID_ARGUMENT"));
        let wire = serde_json::json!({"tenant": "t", "query": "cost", "group_by": "agent"});
        let parsed: RunMetricsRequest = serde_json::from_value(wire).unwrap();
        assert_eq!(parsed.query, RunMetricsQuery::Cost);
        assert!(serde_json::from_value::<RunMetricsRequest>(
            serde_json::json!({"tenant": "t", "query": "p99"})
        )
        .is_err());
    }

    fn measured(technique: &str, arm: &str, config: &str, latency: f64, score: f64) -> RunSample {
        RunSample {
            termination: "final_answer".into(),
            technique: technique.into(),
            arm: arm.into(),
            config_digest: config.into(),
            latency_ms: Some(latency),
            score: Some(score),
            cost_usd: Some(0.5),
            ..RunSample::default()
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.1.1
    #[test]
    fn a_technique_measure_is_latency_cost_and_score_of_one_technique_arm_and_config() {
        let runs = || {
            vec![
                measured("rerank", "a", "sha256:1", 100.0, 0.5),
                measured("rerank", "a", "sha256:1", 300.0, 0.9),
                measured("rerank", "b", "sha256:1", 999.0, 0.1),
                measured("rerank", "a", "sha256:2", 999.0, 0.1),
                measured("other", "a", "sha256:1", 999.0, 0.1),
            ]
        };
        let ask = |query| RunMetricsRequest {
            tenant: "tenant-a".into(),
            query,
            group_by: None,
            window: None,
            technique: Some("rerank".into()),
            arm: Some("a".into()),
            config_digest: Some("sha256:1".into()),
        };
        let value = |query| {
            let answer = aggregate(&ask(query), runs());
            assert_eq!(answer.abstained, None);
            (answer.groups[0].value, answer.groups[0].trace_count)
        };
        assert_eq!(value(RunMetricsQuery::Latency), (200.0, 2));
        assert_eq!(value(RunMetricsQuery::Score), (0.7, 2));
        assert_eq!(value(RunMetricsQuery::Cost), (0.5, 2));
        // A measure nothing matches abstains, and a technique grouping splits it.
        let mut none = ask(RunMetricsQuery::Latency);
        none.arm = Some("zzz".into());
        assert_eq!(
            aggregate(&none, runs()).abstained.as_deref(),
            Some(ABSTAIN_NO_SOLVED_RUNS)
        );
        let mut grouped = ask(RunMetricsQuery::Score);
        grouped.technique = None;
        grouped.arm = None;
        grouped.group_by = Some(RunMetricsGroupBy::Technique);
        let keys: Vec<_> = aggregate(&grouped, runs())
            .groups
            .into_iter()
            .map(|group| group.key)
            .collect();
        assert_eq!(keys, ["other", "rerank"]);
        // A filter outside its bounds is refused.
        let mut empty = ask(RunMetricsQuery::Latency);
        empty.technique = Some(String::new());
        assert!(empty
            .validate()
            .unwrap_err()
            .starts_with("INVALID_ARGUMENT"));
    }
}
