//! Tool necessity records (MEDULLA-DECISION-ENGINE-R165.1).
//!
//! How much a tool matters to a model's success is a Q32 difference between
//! runs with and without it. The records here carry that difference, compare it
//! with what the model did and name the mismatches.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::run_adjustment::INSUFFICIENT_SAMPLES;
use super::run_governance::{RunOutcome, RunTraceRecord, StepKind};
use medulla_types::refusal_code::RefusalCode;

const DAY_MS: u64 = 86_400_000;
/// Domain of the necessity provenance digest.
pub const NECESSITY_PROVENANCE_DOMAIN: &[u8] = b"eg/tool-necessity/v1";

/// Request refusal (wire error `PARAMETER_INVALID`): `tool_id` is not a published Tool
/// component of the tenant (MEDULLA-DECISION-ENGINE-R165.4).
pub const TOOL_UNKNOWN: RefusalCode = RefusalCode::new("TOOL_UNKNOWN");
/// Request refusal (wire error `PARAMETER_INVALID`): `model_id` is not a published
/// ModelProfile component of the tenant (MEDULLA-DECISION-ENGINE-R165.4).
pub const MODEL_UNKNOWN: RefusalCode = RefusalCode::new("MODEL_UNKNOWN");
/// Abstention: the newest counted run is older than `tool_necessity.max_age`
/// (MEDULLA-DECISION-ENGINE-R165.4).
pub const STALE_MEASUREMENT: RefusalCode = RefusalCode::new("STALE_MEASUREMENT");

/// The `tool_necessity` family table: its own codes. `INSUFFICIENT_SAMPLES` is the shared
/// abstention registered with the run-adjustment table.
pub const REFUSALS: &[RefusalCode] = &[TOOL_UNKNOWN, MODEL_UNKNOWN, STALE_MEASUREMENT];

/// The `tool_necessity.*` knobs (MEDULLA-DECISION-ENGINE-R165.2).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "tool_necessity.min_samples",
        unit: "runs per arm",
        default: KnobValue::Integer(30),
        min: KnobValue::Integer(5),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "tool_necessity.necessary_min",
        unit: "fraction",
        default: KnobValue::DecimalMicros(100_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "tool_necessity.window",
        unit: "ms",
        default: KnobValue::DurationMs(30 * DAY_MS),
        min: KnobValue::DurationMs(DAY_MS),
        max: KnobValue::DurationMs(365 * DAY_MS),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "tool_necessity.max_age",
        unit: "ms",
        default: KnobValue::DurationMs(DAY_MS),
        min: KnobValue::DurationMs(3_600_000),
        max: KnobValue::DurationMs(30 * DAY_MS),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `tool_necessity.*` values one decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolNecessityKnobs {
    /// Runs each arm needs.
    pub min_samples: u32,
    /// `tool_necessity.necessary_min` on the Q32 scale.
    pub necessary_min_q32: i64,
    /// How far back runs count, in milliseconds.
    pub window_ms: u64,
    /// How old a measured necessity may be, in milliseconds.
    pub max_age_ms: u64,
}

impl ToolNecessityKnobs {
    /// The values the policy pins, or the defaults.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let integer = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        let micros = match policy.knob(&KNOBS[1]) {
            KnobValue::DecimalMicros(value) => value,
            _ => 0,
        };
        let duration = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::DurationMs(value) => value,
            _ => u64::MAX,
        };
        Self {
            min_samples: integer(0),
            necessary_min_q32: i64::try_from((i128::from(micros) << 32) / 1_000_000)
                .unwrap_or(Q32_ONE),
            window_ms: duration(2),
            max_age_ms: duration(3),
        }
    }
}

/// Format identity of [`ToolNecessity`].
pub const TOOL_NECESSITY_SCHEMA_VERSION: u16 = 1;
/// `2^32`, the bound of a Q32 difference.
pub const Q32_ONE: i64 = 1 << 32;

/// The measured necessity of one tool for one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolNecessity {
    pub schema_version: u16,
    pub tool_id: String,
    pub model_id: String,
    pub necessity_q32: i64,
    pub samples_with: u32,
    pub samples_without: u32,
    pub newest_run_ms: u64,
    pub provenance_digest: Digest256,
}

impl ToolNecessity {
    /// `necessity_q32` lies in `[-2^32, 2^32]`.
    pub fn checked(self) -> Result<Self, String> {
        if !(-Q32_ONE..=Q32_ONE).contains(&self.necessity_q32) {
            return Err("necessity_q32 is outside [-2^32, 2^32]".into());
        }
        Ok(self)
    }
}

/// A tool's necessity against what the model did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolComparison {
    pub tool_id: String,
    pub necessity_q32: i64,
    pub model_called: bool,
    pub agreement: bool,
}

/// Why a trace disagrees with the measured necessity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MismatchReason {
    MissedTool,
    UnneededTool,
}

/// One disagreement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceMismatch {
    pub tool_id: String,
    pub necessity_q32: i64,
    pub model_call_id: String,
    pub reason: MismatchReason,
}

/// Why a necessity could not be measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NecessityRefusal {
    /// An arm holds fewer runs than `tool_necessity.min_samples`
    /// (`INSUFFICIENT_SAMPLES`).
    InsufficientSamples {
        /// Runs that used the tool.
        with: u32,
        /// Runs that did not.
        without: u32,
        /// The floor each arm needs.
        min_samples: u32,
    },
    /// The provenance digest could not be framed.
    Malformed(String),
}

impl NecessityRefusal {
    /// The registered code of the refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InsufficientSamples { .. } => INSUFFICIENT_SAMPLES,
            Self::Malformed(_) => "PARAMETER_INVALID",
        }
    }
}

impl std::fmt::Display for NecessityRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientSamples {
                with,
                without,
                min_samples,
            } => write!(
                f,
                "{INSUFFICIENT_SAMPLES}: {with} runs with and {without} without the tool, \
                 {min_samples} needed in each arm"
            ),
            Self::Malformed(detail) => write!(f, "tool necessity is malformed: {detail}"),
        }
    }
}

impl std::error::Error for NecessityRefusal {}

/// A success rate on the Q32 scale: `succeeded / total`, rounded down.
fn rate_q32(succeeded: u32, total: u32) -> i64 {
    ((i128::from(succeeded) << 32) / i128::from(total)) as i64
}

/// Measure how much `tool_id` matters to `model_id`: the success rate of the
/// runs that called it minus the success rate of those that did not, as a Q32
/// difference. Only runs of `model_id` count. A run used the tool when a step
/// is a `tool_call` whose `action_ref` equals `tool_id`; it succeeded when its
/// outcome is `succeeded`. An arm below `min_samples` refuses.
pub fn measure_tool_necessity(
    runs: &[RunTraceRecord],
    tool_id: &str,
    model_id: &str,
    min_samples: u32,
) -> Result<ToolNecessity, NecessityRefusal> {
    let (mut with, mut with_ok, mut without, mut without_ok) = (0u32, 0u32, 0u32, 0u32);
    let mut newest_run_ms = 0u64;
    let mut run_ids: Vec<&str> = Vec::new();
    for run in runs.iter().filter(|run| run.model_id == model_id) {
        let used = run
            .steps
            .iter()
            .any(|step| step.kind == StepKind::ToolCall && step.action_ref == tool_id);
        let succeeded = run.outcome == RunOutcome::Succeeded;
        let (count, wins) = if used {
            (&mut with, &mut with_ok)
        } else {
            (&mut without, &mut without_ok)
        };
        *count = count.saturating_add(1);
        if succeeded {
            *wins = wins.saturating_add(1);
        }
        newest_run_ms = newest_run_ms.max(run.started_at_ms);
        run_ids.push(run.run_id.as_str());
    }
    if with < min_samples || without < min_samples || with == 0 || without == 0 {
        return Err(NecessityRefusal::InsufficientSamples {
            with,
            without,
            min_samples,
        });
    }
    run_ids.sort_unstable();
    let mut fields: Vec<&[u8]> = vec![tool_id.as_bytes(), model_id.as_bytes()];
    fields.extend(run_ids.iter().map(|id| id.as_bytes()));
    let provenance_digest = Digest256::framed(NECESSITY_PROVENANCE_DOMAIN, &fields)
        .map_err(NecessityRefusal::Malformed)?;
    Ok(ToolNecessity {
        schema_version: TOOL_NECESSITY_SCHEMA_VERSION,
        tool_id: tool_id.to_string(),
        model_id: model_id.to_string(),
        necessity_q32: rate_q32(with_ok, with) - rate_q32(without_ok, without),
        samples_with: with,
        samples_without: without,
        newest_run_ms,
        provenance_digest,
    })
}

/// Compare the measured necessity with what the model did. The tool is
/// necessary when its necessity is at least `necessary_min_q32`; the two agree
/// when necessary equals `model_called`.
pub fn compare_tool_decision(
    necessity: &ToolNecessity,
    model_called: bool,
    necessary_min_q32: i64,
) -> ToolComparison {
    let necessary = necessity.necessity_q32 >= necessary_min_q32;
    ToolComparison {
        tool_id: necessity.tool_id.clone(),
        necessity_q32: necessity.necessity_q32,
        model_called,
        agreement: necessary == model_called,
    }
}

impl ToolComparison {
    /// The disagreement this comparison records, or none when they agree. A
    /// necessary tool the model did not call is `missed_tool`; an unnecessary
    /// tool it called is `unneeded_tool`.
    pub fn mismatch(&self, model_call_id: &str) -> Option<TraceMismatch> {
        (!self.agreement).then(|| TraceMismatch {
            tool_id: self.tool_id.clone(),
            necessity_q32: self.necessity_q32,
            model_call_id: model_call_id.to_string(),
            reason: if self.model_called {
                MismatchReason::UnneededTool
            } else {
                MismatchReason::MissedTool
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::run_governance::{StepOutcome, TraceStep};
    use medulla_types::contract::BoundedVec;

    fn run(id: &str, model: &str, uses_tool: bool, outcome: RunOutcome) -> RunTraceRecord {
        let steps = if uses_tool {
            vec![TraceStep {
                index: 0,
                kind: StepKind::ToolCall,
                action_ref: "tool:search".into(),
                outcome: StepOutcome::Ok,
                arguments: BoundedVec::default(),
            }]
        } else {
            Vec::new()
        };
        RunTraceRecord {
            schema_version: 1,
            run_id: id.into(),
            task_signature: "task".into(),
            model_id: model.into(),
            steps: BoundedVec::new(steps).unwrap(),
            outcome,
            started_at_ms: 1_000,
        }
    }

    fn runs(
        model: &str,
        with_ok: u32,
        with_bad: u32,
        without_ok: u32,
        without_bad: u32,
    ) -> Vec<RunTraceRecord> {
        let mut all = Vec::new();
        let mut push = |count: u32, tool: bool, outcome: RunOutcome| {
            for _ in 0..count {
                let id = format!("run-{}", all.len());
                all.push(run(&id, model, tool, outcome));
            }
        };
        push(with_ok, true, RunOutcome::Succeeded);
        push(with_bad, true, RunOutcome::Failed);
        push(without_ok, false, RunOutcome::Succeeded);
        push(without_bad, false, RunOutcome::Failed);
        all
    }

    // spec: MEDULLA-DECISION-ENGINE-R165.2
    #[test]
    fn necessity_comparison_detects_mismatch() {
        let min = Q32_ONE / 10;
        // With the tool 9 of 10 succeed, without it 3 of 10: necessity 0.6.
        let mut all = runs("model:m", 9, 1, 3, 7);
        // Another model's runs never count.
        all.extend(runs("model:other", 0, 20, 20, 0));
        let necessity = measure_tool_necessity(&all, "tool:search", "model:m", 10).unwrap();
        assert_eq!(necessity.samples_with, 10);
        assert_eq!(necessity.samples_without, 10);
        let expected = rate_q32(9, 10) - rate_q32(3, 10);
        assert_eq!(necessity.necessity_q32, expected);
        assert!(necessity.clone().checked().is_ok());
        assert_eq!(necessity.newest_run_ms, 1_000);

        // A necessary tool the model skipped is a missed tool.
        let missed = compare_tool_decision(&necessity, false, min);
        assert!(!missed.agreement);
        assert_eq!(
            missed.mismatch("call:1").unwrap().reason,
            MismatchReason::MissedTool
        );
        // A necessary tool the model called agrees.
        let agreed = compare_tool_decision(&necessity, true, min);
        assert!(agreed.agreement);
        assert!(agreed.mismatch("call:1").is_none());

        // An unnecessary tool the model called is an unneeded tool.
        let flat =
            measure_tool_necessity(&runs("model:m", 5, 5, 5, 5), "tool:search", "model:m", 10)
                .unwrap();
        assert_eq!(flat.necessity_q32, 0);
        let unneeded = compare_tool_decision(&flat, true, min);
        assert_eq!(
            unneeded.mismatch("call:2").unwrap().reason,
            MismatchReason::UnneededTool
        );

        // The same runs give the same provenance; an arm below the floor abstains.
        let again = measure_tool_necessity(&all, "tool:search", "model:m", 10).unwrap();
        assert_eq!(again.provenance_digest, necessity.provenance_digest);
        let thin =
            measure_tool_necessity(&runs("model:m", 9, 1, 2, 2), "tool:search", "model:m", 10);
        assert_eq!(
            thin,
            Err(NecessityRefusal::InsufficientSamples {
                with: 10,
                without: 4,
                min_samples: 10
            })
        );
        assert_eq!(thin.unwrap_err().code(), "INSUFFICIENT_SAMPLES");
    }

    fn necessity(value: i64) -> ToolNecessity {
        ToolNecessity {
            schema_version: TOOL_NECESSITY_SCHEMA_VERSION,
            tool_id: "tool:search".into(),
            model_id: "model:m".into(),
            necessity_q32: value,
            samples_with: 30,
            samples_without: 28,
            newest_run_ms: 1_700_000_000_000,
            provenance_digest: Digest256::sha256(b"runs"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R165.1
    #[test]
    fn necessity_types_round_trip() {
        let record = necessity(Q32_ONE / 2).checked().unwrap();
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<ToolNecessity>(&text).unwrap(),
            record
        );
        let comparison = ToolComparison {
            tool_id: "tool:search".into(),
            necessity_q32: record.necessity_q32,
            model_called: false,
            agreement: false,
        };
        let text = serde_json::to_string(&comparison).unwrap();
        assert_eq!(
            serde_json::from_str::<ToolComparison>(&text).unwrap(),
            comparison
        );
        let mismatch = TraceMismatch {
            tool_id: "tool:search".into(),
            necessity_q32: record.necessity_q32,
            model_call_id: "call:1".into(),
            reason: MismatchReason::MissedTool,
        };
        let text = serde_json::to_string(&mismatch).unwrap();
        assert!(text.contains("\"reason\":\"missed_tool\""));
        assert_eq!(
            serde_json::from_str::<TraceMismatch>(&text).unwrap(),
            mismatch
        );
        assert!(necessity(Q32_ONE).checked().is_ok());
        assert!(necessity(-Q32_ONE).checked().is_ok());
        assert!(necessity(Q32_ONE + 1).checked().is_err());
        assert!(necessity(-Q32_ONE - 1).checked().is_err());
    }
}
