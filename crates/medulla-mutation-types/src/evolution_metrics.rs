//! MEDULLA-ORCH-VOCAB-R010.100: join an evolution proposal to its measured source traces as
//! a read view.
//!
//! The statement reads one RLS-filtered snapshot of the caller's graph: the
//! `EvolutionProgress` node the harness wrote, the `ProgramRevision` node of the prompt, and
//! the `RunTrace` nodes the progress record lists. It checks the pinned revisions and digests,
//! never counts a missing trace as zero, and writes nothing.

use std::collections::BTreeMap;

use medulla_query_types::wire::UqlRow;
use medulla_types::contract::Digest256;
use serde_json::{Map, Value};

use crate::harness_metrics::answer_group;

/// The statement's name.
pub const EVOLUTION_TRACE_METRICS: &str = "evolution_trace_metrics";
/// The domain of the metric window digest.
pub const EVOLUTION_WINDOW_DIGEST_DOMAIN: &[u8] = b"eg/evolution-metric-window/v1";
/// The result columns of every row.
pub const EVOLUTION_COLUMNS: [&str; 3] = ["value", "numerator", "denominator"];

/// A pinned digest differs from the stored one.
pub const EVOLUTION_DIGEST_MISMATCH: &str = "EVOLUTION_DIGEST_MISMATCH";
/// A proposal, prompt or window revision is not held.
pub const EVOLUTION_REVISION_UNKNOWN: &str = "EVOLUTION_REVISION_UNKNOWN";
/// The stored metric window is newer than the one asked for.
pub const EVOLUTION_WINDOW_STALE: &str = "EVOLUTION_WINDOW_STALE";

/// The metrics of the answer, in order: name and the statement that computes it.
const METRICS: [(&str, &str); 3] = [
    ("success_rate", "harness_success_rate"),
    ("overflow_rate", "harness_overflow_rate"),
    ("mean_peak_ratio", "harness_mean_peak_ratio"),
];
/// The unit of every metric.
const RATIO: &str = "ratio";

/// The parameters of the statement. The tenant is never one: it is the verified caller's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvolutionTraceQuery {
    /// The proposal.
    pub proposal_id: String,
    /// The proposal revision.
    pub proposal_revision: u64,
    /// The prompt version id (the `program_ref` of its `ProgramRevision`).
    pub prompt_version_id: String,
    /// The prompt revision.
    pub prompt_revision: u64,
    /// The digest of the prompt revision's content.
    pub prompt_digest: Digest256,
    /// The metric window.
    pub metric_window_id: String,
    /// The metric window revision.
    pub metric_window_revision: u64,
    /// The digest of the metric window (see [`window_digest`]).
    pub metric_window_digest: Digest256,
}

fn invalid(name: &str, detail: &str) -> String {
    format!("INVALID_ARGUMENT: ${name} {detail}")
}

impl EvolutionTraceQuery {
    /// Read the statement's parameters. A missing parameter, a revision that is not a
    /// non-negative integer, or a digest that does not parse is refused.
    pub fn from_params(
        text: impl Fn(&str) -> Option<String>,
        number: impl Fn(&str) -> Option<f64>,
    ) -> Result<Self, String> {
        let string = |name: &str| text(name).ok_or_else(|| invalid(name, "is required"));
        let revision = |name: &str| -> Result<u64, String> {
            let value = number(name).ok_or_else(|| invalid(name, "is required"))?;
            if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > 9.0e15 {
                return Err(invalid(name, "must be a non-negative integer"));
            }
            Ok(value as u64)
        };
        let digest = |name: &str| -> Result<Digest256, String> {
            Digest256::parse(&string(name)?).map_err(|_| invalid(name, "is not a digest"))
        };
        Ok(Self {
            proposal_id: string("proposal_id")?,
            proposal_revision: revision("proposal_revision")?,
            prompt_version_id: string("prompt_version_id")?,
            prompt_revision: revision("prompt_revision")?,
            prompt_digest: digest("prompt_digest")?,
            metric_window_id: string("metric_window_id")?,
            metric_window_revision: revision("metric_window_revision")?,
            metric_window_digest: digest("metric_window_digest")?,
        })
    }
}

/// The progress record the harness wrote for one proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvolutionProgress {
    /// The proposal.
    pub proposal_id: String,
    /// The proposal revision.
    pub proposal_revision: u64,
    /// The prompt version id.
    pub prompt_version_id: String,
    /// The prompt revision.
    pub prompt_revision: u64,
    /// The metric window.
    pub metric_window_id: String,
    /// The metric window revision.
    pub metric_window_revision: u64,
    /// The source run ids, in run order.
    pub source_run_ids: Vec<String>,
    /// When the window was observed (Unix ms).
    pub observed_at_ms: u64,
}

impl EvolutionProgress {
    /// Read a progress record from a node's properties; `None` when the node is not an
    /// `EvolutionProgress` or any property is missing or ill-typed.
    pub fn from_node(props: &Map<String, Value>) -> Option<Self> {
        if props.get("node_type").and_then(Value::as_str) != Some("EvolutionProgress") {
            return None;
        }
        let text = |key: &str| props.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| props.get(key).and_then(Value::as_u64);
        let source_run_ids: Vec<String> =
            serde_json::from_str(props.get("sourceRunIds")?.as_str()?).ok()?;
        Some(Self {
            proposal_id: text("proposalId")?,
            proposal_revision: number("proposalRevision")?,
            prompt_version_id: text("promptVersionId")?,
            prompt_revision: number("promptRevision")?,
            metric_window_id: text("metricWindowId")?,
            metric_window_revision: number("metricWindowRevision")?,
            source_run_ids,
            observed_at_ms: number("observedAtMs")?,
        })
    }
}

/// Whether the answer covers every source trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerState {
    /// Every source run id has a trace.
    Complete,
    /// A source run id has no trace, or there are no source run ids.
    Partial,
}

/// Whether a metric could be measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricState {
    /// The value is known.
    Measured,
    /// The value is unknown; its three channels are `None`.
    Unknown,
}

/// One metric over the found traces.
#[derive(Clone, Debug, PartialEq)]
pub struct EvolutionMetric {
    /// The metric's name.
    pub name: &'static str,
    /// The metric's unit.
    pub unit: &'static str,
    /// The ratio, when measured.
    pub value: Option<f64>,
    /// The numerator, when measured.
    pub numerator: Option<f64>,
    /// The denominator, when measured.
    pub denominator: Option<f64>,
    /// Whether the value is known.
    pub state: MetricState,
}

/// The answer to one `evolution_trace_metrics` statement.
#[derive(Clone, Debug, PartialEq)]
pub struct EvolutionTraceAnswer {
    /// The source run ids that are `RunTrace` nodes.
    pub trace_refs: Vec<String>,
    /// The source run ids that are not.
    pub missing_ids: Vec<String>,
    /// The digest of the metric window the answer was read at.
    pub window_digest: Digest256,
    /// Found traces over source run ids; `None` for an empty list.
    pub coverage: Option<f64>,
    /// Whether every source run id has a trace.
    pub state: AnswerState,
    /// When the window was observed (Unix ms).
    pub observed_at_ms: u64,
    /// The metrics over the found traces.
    pub metrics: Vec<EvolutionMetric>,
}

/// The digest of the metric window: the proposal, prompt and window pins, the prompt content
/// digest and the source run ids in stored order, framed under
/// [`EVOLUTION_WINDOW_DIGEST_DOMAIN`].
pub fn window_digest(progress: &EvolutionProgress, prompt_digest: &Digest256) -> Digest256 {
    let proposal_revision = progress.proposal_revision.to_be_bytes();
    let prompt_revision = progress.prompt_revision.to_be_bytes();
    let window_revision = progress.metric_window_revision.to_be_bytes();
    let mut fields: Vec<&[u8]> = vec![
        progress.proposal_id.as_bytes(),
        &proposal_revision,
        progress.prompt_version_id.as_bytes(),
        &prompt_revision,
        prompt_digest.as_bytes(),
        progress.metric_window_id.as_bytes(),
        &window_revision,
    ];
    fields.extend(progress.source_run_ids.iter().map(|id| id.as_bytes()));
    Digest256::framed(EVOLUTION_WINDOW_DIGEST_DOMAIN, &fields)
        .expect("the window digest domain is a non-empty constant")
}

fn revision_of(props: &Map<String, Value>, key: &str) -> Option<u64> {
    props.get(key).and_then(Value::as_u64)
}

/// Join the proposal to its source traces over the snapshot's node id to property map.
/// Each refusal names its code and writes nothing.
pub fn evolution_trace_metrics(
    query: &EvolutionTraceQuery,
    nodes: &BTreeMap<String, Map<String, Value>>,
) -> Result<EvolutionTraceAnswer, String> {
    let unknown_proposal = || {
        format!(
            "{EVOLUTION_REVISION_UNKNOWN}: proposal {} revision {}",
            query.proposal_id, query.proposal_revision
        )
    };
    let unknown_prompt = || {
        format!(
            "{EVOLUTION_REVISION_UNKNOWN}: prompt {} revision {}",
            query.prompt_version_id, query.prompt_revision
        )
    };
    let progress = nodes
        .values()
        .filter_map(EvolutionProgress::from_node)
        .find(|progress| {
            progress.proposal_id == query.proposal_id
                && progress.proposal_revision == query.proposal_revision
        })
        .ok_or_else(unknown_proposal)?;
    if progress.prompt_version_id != query.prompt_version_id
        || progress.prompt_revision != query.prompt_revision
    {
        return Err(unknown_prompt());
    }
    let prompt = nodes
        .values()
        .find(|props| {
            props.get("type").and_then(Value::as_str) == Some("ProgramRevision")
                && props.get("program_ref").and_then(Value::as_str)
                    == Some(query.prompt_version_id.as_str())
                && revision_of(props, "revision") == Some(query.prompt_revision)
        })
        .ok_or_else(unknown_prompt)?;
    if prompt.get("content_digest").and_then(Value::as_str)
        != Some(query.prompt_digest.to_hex().as_str())
    {
        return Err(format!("{EVOLUTION_DIGEST_MISMATCH}: prompt"));
    }
    if progress.metric_window_id != query.metric_window_id
        || progress.metric_window_revision < query.metric_window_revision
    {
        return Err(format!(
            "{EVOLUTION_REVISION_UNKNOWN}: window {} revision {}",
            query.metric_window_id, query.metric_window_revision
        ));
    }
    if progress.metric_window_revision > query.metric_window_revision {
        return Err(format!(
            "{EVOLUTION_WINDOW_STALE}: window {} is at revision {}",
            query.metric_window_id, progress.metric_window_revision
        ));
    }
    let digest = window_digest(&progress, &query.prompt_digest);
    if digest != query.metric_window_digest {
        return Err(format!("{EVOLUTION_DIGEST_MISMATCH}: window"));
    }

    let mut trace_refs = Vec::new();
    let mut missing_ids = Vec::new();
    let mut found: Vec<&Map<String, Value>> = Vec::new();
    for run_id in &progress.source_run_ids {
        match nodes
            .get(run_id)
            .filter(|props| props.get("node_type").and_then(Value::as_str) == Some("RunTrace"))
        {
            Some(props) => {
                trace_refs.push(run_id.clone());
                found.push(props);
            }
            None => missing_ids.push(run_id.clone()),
        }
    }
    let source_count = progress.source_run_ids.len();
    let coverage = (source_count > 0).then(|| trace_refs.len() as f64 / source_count as f64);
    let state = if missing_ids.is_empty() && source_count > 0 {
        AnswerState::Complete
    } else {
        AnswerState::Partial
    };
    let metrics = METRICS
        .iter()
        .map(|&(name, statement)| {
            let row = answer_group(statement, String::new(), &found);
            let value = row.values.first().copied().flatten();
            match value {
                Some(value) => EvolutionMetric {
                    name,
                    unit: RATIO,
                    value: Some(value),
                    numerator: row.values.get(1).copied().flatten(),
                    denominator: Some(found.len() as f64),
                    state: MetricState::Measured,
                },
                None => EvolutionMetric {
                    name,
                    unit: RATIO,
                    value: None,
                    numerator: None,
                    denominator: None,
                    state: MetricState::Unknown,
                },
            }
        })
        .collect();
    Ok(EvolutionTraceAnswer {
        trace_refs,
        missing_ids,
        window_digest: digest,
        coverage,
        state,
        observed_at_ms: progress.observed_at_ms,
        metrics,
    })
}

fn row(id: String, channels: [Option<f64>; 3], provenance: &[(&str, String)]) -> UqlRow {
    UqlRow {
        id,
        score: None,
        channels: channels.to_vec(),
        knowledge: None,
        proof: None,
        provenance: provenance
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    }
}

impl EvolutionTraceAnswer {
    /// The rows of the wire result: one row per source trace (found ones, then missing ones,
    /// each in run order), one row per metric, then the `answer` row.
    pub fn to_rows(&self) -> Vec<UqlRow> {
        let mut rows = Vec::new();
        for (ids, state) in [(&self.trace_refs, "found"), (&self.missing_ids, "missing")] {
            for id in ids {
                rows.push(row(
                    format!("trace:{id}"),
                    [None, None, None],
                    &[("kind", "trace".into()), ("state", state.into())],
                ));
            }
        }
        for metric in &self.metrics {
            let state = match metric.state {
                MetricState::Measured => "measured",
                MetricState::Unknown => "unknown",
            };
            rows.push(row(
                format!("metric:{}", metric.name),
                [metric.value, metric.numerator, metric.denominator],
                &[
                    ("kind", "metric".into()),
                    ("unit", metric.unit.into()),
                    ("state", state.into()),
                ],
            ));
        }
        let state = match self.state {
            AnswerState::Complete => "complete",
            AnswerState::Partial => "partial",
        };
        rows.push(row(
            "answer".to_owned(),
            [
                self.coverage,
                Some(self.trace_refs.len() as f64),
                Some((self.trace_refs.len() + self.missing_ids.len()) as f64),
            ],
            &[
                ("kind", "answer".into()),
                ("state", state.into()),
                ("window_digest", self.window_digest.to_hex()),
                ("observed_at_ms", self.observed_at_ms.to_string()),
            ],
        ));
        rows
    }

    /// Rebuild an answer from the rows [`Self::to_rows`] wrote.
    pub fn from_rows(rows: &[UqlRow]) -> Result<Self, String> {
        let mut trace_refs = Vec::new();
        let mut missing_ids = Vec::new();
        let mut metrics = Vec::new();
        let mut answer: Option<&UqlRow> = None;
        let property = |row: &UqlRow, key: &str| -> Result<String, String> {
            row.provenance
                .get(key)
                .cloned()
                .ok_or_else(|| format!("row {} has no {key}", row.id))
        };
        for row in rows {
            match property(row, "kind")?.as_str() {
                "trace" => {
                    let id = row.id.strip_prefix("trace:").ok_or("trace row id")?.to_owned();
                    match property(row, "state")?.as_str() {
                        "found" => trace_refs.push(id),
                        "missing" => missing_ids.push(id),
                        other => return Err(format!("trace state {other}")),
                    }
                }
                "metric" => {
                    let name = row.id.strip_prefix("metric:").ok_or("metric row id")?;
                    let &(name, _) = METRICS
                        .iter()
                        .find(|(known, _)| *known == name)
                        .ok_or_else(|| format!("metric {name}"))?;
                    if property(row, "unit")? != RATIO {
                        return Err(format!("metric {name} unit"));
                    }
                    let channel = |at: usize| row.channels.get(at).copied().flatten();
                    let state = match property(row, "state")?.as_str() {
                        "measured" => MetricState::Measured,
                        "unknown" => MetricState::Unknown,
                        other => return Err(format!("metric state {other}")),
                    };
                    metrics.push(EvolutionMetric {
                        name,
                        unit: RATIO,
                        value: channel(0),
                        numerator: channel(1),
                        denominator: channel(2),
                        state,
                    });
                }
                "answer" => answer = Some(row),
                other => return Err(format!("row kind {other}")),
            }
        }
        let answer = answer.ok_or("the answer row is missing")?;
        let state = match property(answer, "state")?.as_str() {
            "complete" => AnswerState::Complete,
            "partial" => AnswerState::Partial,
            other => return Err(format!("answer state {other}")),
        };
        let counts = |at: usize| answer.channels.get(at).copied().flatten();
        if counts(1) != Some(trace_refs.len() as f64)
            || counts(2) != Some((trace_refs.len() + missing_ids.len()) as f64)
        {
            return Err("the answer counts do not match the trace rows".to_owned());
        }
        Ok(Self {
            trace_refs,
            missing_ids,
            window_digest: Digest256::parse(&property(answer, "window_digest")?)?,
            coverage: counts(0),
            state,
            observed_at_ms: property(answer, "observed_at_ms")?
                .parse()
                .map_err(|_| "observed_at_ms".to_owned())?,
            metrics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PROMPT_DIGEST_HEX: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("an object")
    }

    fn progress() -> EvolutionProgress {
        EvolutionProgress {
            proposal_id: "proposal-1".into(),
            proposal_revision: 2,
            prompt_version_id: "prompt-1".into(),
            prompt_revision: 3,
            metric_window_id: "window-1".into(),
            metric_window_revision: 4,
            source_run_ids: vec!["run-a".into(), "run-b".into()],
            observed_at_ms: 1_000,
        }
    }

    fn query() -> EvolutionTraceQuery {
        let prompt_digest = Digest256::parse(PROMPT_DIGEST_HEX).unwrap();
        EvolutionTraceQuery {
            proposal_id: "proposal-1".into(),
            proposal_revision: 2,
            prompt_version_id: "prompt-1".into(),
            prompt_revision: 3,
            prompt_digest,
            metric_window_id: "window-1".into(),
            metric_window_revision: 4,
            metric_window_digest: window_digest(&progress(), &prompt_digest),
        }
    }

    fn graph(with_second_trace: bool) -> BTreeMap<String, Map<String, Value>> {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "progress-1".to_owned(),
            object(json!({
                "node_type": "EvolutionProgress",
                "proposalId": "proposal-1",
                "proposalRevision": 2,
                "promptVersionId": "prompt-1",
                "promptRevision": 3,
                "metricWindowId": "window-1",
                "metricWindowRevision": 4,
                "sourceRunIds": "[\"run-a\",\"run-b\"]",
                "observedAtMs": 1000,
            })),
        );
        nodes.insert(
            "revision-1".to_owned(),
            object(json!({
                "type": "ProgramRevision",
                "program_ref": "prompt-1",
                "revision": 3,
                "content_digest": PROMPT_DIGEST_HEX,
            })),
        );
        nodes.insert(
            "run-a".to_owned(),
            object(json!({"node_type": "RunTrace", "status": "completed"})),
        );
        if with_second_trace {
            nodes.insert(
                "run-b".to_owned(),
                object(json!({"node_type": "RunTrace", "status": "failed"})),
            );
        }
        nodes
    }

    // spec: MEDULLA-ORCH-VOCAB-R010.100
    #[test]
    fn answer_round_trips_through_rows() {
        let answer = evolution_trace_metrics(&query(), &graph(true)).unwrap();
        assert_eq!(answer.state, AnswerState::Complete);
        assert_eq!(answer.trace_refs, ["run-a", "run-b"]);
        assert_eq!(answer.coverage, Some(1.0));
        let success = &answer.metrics[0];
        assert_eq!(success.name, "success_rate");
        assert_eq!(
            (success.value, success.numerator, success.denominator),
            (Some(0.5), Some(1.0), Some(2.0))
        );
        let rows = answer.to_rows();
        assert_eq!(rows.last().unwrap().id, "answer");
        assert_eq!(
            EvolutionTraceAnswer::from_rows(&rows).unwrap(),
            answer
        );
        // The window digest is the framed digest of the fields in their stated order.
        let prompt_digest = Digest256::parse(PROMPT_DIGEST_HEX).unwrap();
        let expected = Digest256::framed(
            EVOLUTION_WINDOW_DIGEST_DOMAIN,
            &[
                b"proposal-1",
                &2u64.to_be_bytes(),
                b"prompt-1",
                &3u64.to_be_bytes(),
                prompt_digest.as_bytes(),
                b"window-1",
                &4u64.to_be_bytes(),
                b"run-a",
                b"run-b",
            ],
        )
        .unwrap();
        assert_eq!(answer.window_digest, expected);
        assert_eq!(
            rows.last().unwrap().provenance.get("window_digest"),
            Some(&expected.to_hex())
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R010.100
    #[test]
    fn missing_trace_is_partial_and_each_refusal_names_its_code() {
        let partial = evolution_trace_metrics(&query(), &graph(false)).unwrap();
        assert_eq!(partial.state, AnswerState::Partial);
        assert_eq!(partial.missing_ids, ["run-b"]);
        assert_eq!(partial.coverage, Some(0.5));
        // A missing trace is never counted as zero: the denominator is the found traces.
        assert_eq!(partial.metrics[0].denominator, Some(1.0));
        assert_eq!(
            EvolutionTraceAnswer::from_rows(&partial.to_rows()).unwrap(),
            partial
        );

        let nodes = graph(true);
        let refused = |query: EvolutionTraceQuery, nodes: &BTreeMap<_, _>| {
            evolution_trace_metrics(&query, nodes).unwrap_err()
        };
        let mut unknown = query();
        unknown.proposal_revision = 9;
        assert!(refused(unknown, &nodes)
            .starts_with("EVOLUTION_REVISION_UNKNOWN: proposal proposal-1 revision 9"));
        let mut prompt = query();
        prompt.prompt_revision = 8;
        assert!(refused(prompt, &nodes)
            .starts_with("EVOLUTION_REVISION_UNKNOWN: prompt prompt-1 revision 8"));
        let mut wrong_digest = query();
        wrong_digest.prompt_digest = Digest256::parse(&"b".repeat(64)).unwrap();
        assert_eq!(
            refused(wrong_digest, &nodes),
            "EVOLUTION_DIGEST_MISMATCH: prompt"
        );
        let mut newer = query();
        newer.metric_window_revision = 5;
        assert!(refused(newer, &nodes)
            .starts_with("EVOLUTION_REVISION_UNKNOWN: window window-1 revision 5"));
        let mut stale = query();
        stale.metric_window_revision = 3;
        assert_eq!(
            refused(stale, &nodes),
            "EVOLUTION_WINDOW_STALE: window window-1 is at revision 4"
        );
        let mut tampered = query();
        tampered.metric_window_digest = Digest256::parse(&"c".repeat(64)).unwrap();
        assert_eq!(
            refused(tampered, &nodes),
            "EVOLUTION_DIGEST_MISMATCH: window"
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R010.100
    #[test]
    fn parameters_are_read_and_refused_by_name() {
        let text = |name: &str| {
            Some(match name {
                "proposal_id" => "p".to_owned(),
                "prompt_version_id" => "v".to_owned(),
                "metric_window_id" => "w".to_owned(),
                _ => PROMPT_DIGEST_HEX.to_owned(),
            })
        };
        let number = |_: &str| Some(1.0);
        assert!(EvolutionTraceQuery::from_params(text, number).is_ok());
        let fractional = |name: &str| (name != "prompt_revision").then_some(1.0).or(Some(1.5));
        assert_eq!(
            EvolutionTraceQuery::from_params(text, fractional).unwrap_err(),
            "INVALID_ARGUMENT: $prompt_revision must be a non-negative integer"
        );
        let bad_digest = |name: &str| (name != "prompt_digest").then(|| "x".to_owned());
        assert!(EvolutionTraceQuery::from_params(bad_digest, number).is_err());
        assert!(EvolutionProgress::from_node(&object(json!({"node_type": "RunTrace"}))).is_none());
    }
}
