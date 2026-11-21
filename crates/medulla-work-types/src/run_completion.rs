//! Ordinary admitted-run measurements, independent of delegated provenance.
use serde::{Deserialize, Serialize};

pub const RUN_COMPLETION_TOPIC: &str = "work_item.run_completion.v1";
pub const RUN_COMPLETION_BATCH: &str = "run_completion_batch";
pub const RUN_COMPLETION_DIGEST: &str = "run_completion_digest";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}
fn validate_token_totals(usage: &RunUsage) -> Result<(), String> {
    if let Some(total) = usage.total_tokens {
        if usage.input_tokens.is_some_and(|n| n > total)
            || usage.output_tokens.is_some_and(|n| n > total)
            || matches!((usage.input_tokens, usage.output_tokens), (Some(i), Some(o)) if i + o != total)
        {
            return Err("incoherent run usage counters".into());
        }
    }
    Ok(())
}

impl RunUsage {
    pub fn validate(&self) -> Result<(), String> {
        if [
            self.requests,
            self.tool_calls,
            self.input_tokens,
            self.output_tokens,
            self.total_tokens,
        ]
        .into_iter()
        .flatten()
        .any(|n| n > 9_007_199_254_740_991)
        {
            return Err("run usage exceeds exact JSON integer range".into());
        }
        validate_token_totals(self)
    }
}

/// The phases of one turn a completion reports a duration for
/// (MEDULLA-ORCH-VOCAB-R814). A label set at construction, not a lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TimingPhase {
    Request,
    Run,
    Queue,
    Tool,
}

impl TimingPhase {
    /// The wire label of the phase.
    pub fn label(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Run => "run",
            Self::Queue => "queue",
            Self::Tool => "tool",
        }
    }
}

/// One phase's duration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PhaseTiming {
    pub phase: TimingPhase,
    pub duration_ms: u64,
}

/// The longest duration one phase may report: one day.
pub const MAX_PHASE_DURATION_MS: u64 = 86_400_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunCompletionInput {
    pub schema_version: u32,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RunUsage>,
    /// Per-phase timings of the turn; absent when none were measured, so a
    /// completion without timings keeps its bytes and its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timings: Vec<PhaseTiming>,
}
impl RunCompletionInput {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.run_id.trim().is_empty() || self.run_id.len() > 512 {
            return Err("invalid ordinary run completion version or run id".into());
        }
        self.usage.as_ref().map_or(Ok(()), RunUsage::validate)?;
        self.validate_timings()
    }

    /// At most one entry per phase, none above [`MAX_PHASE_DURATION_MS`].
    fn validate_timings(&self) -> Result<(), String> {
        let mut seen = Vec::new();
        for timing in &self.timings {
            let label = timing.phase.label();
            if seen.contains(&timing.phase.label()) {
                return Err(format!(
                    "INVALID_ARGUMENT: invalid run completion timing: {label} reported more than once"
                ));
            }
            if timing.duration_ms > MAX_PHASE_DURATION_MS {
                return Err(format!(
                    "INVALID_ARGUMENT: invalid run completion timing: {label} duration exceeds {MAX_PHASE_DURATION_MS} ms"
                ));
            }
            seen.push(label);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunCompletionPayload {
    pub completion: RunCompletionInput,
    pub tenant: String,
    pub work_item_id: String,
    pub batch_id: String,
    pub outcome: String,
    pub result_ref: Option<String>,
    pub error_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CommittedRunCompletion {
    pub payload: RunCompletionPayload,
    pub scope: medulla_mutation_types::mutation_scope::MutationScopeIdentity,
    pub commit_sequence: u64,
    pub ordinal: u32,
    pub payload_digest: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_usage_is_not_zero() {
        let value: RunCompletionInput =
            serde_json::from_str(r#"{"schema_version":1,"run_id":"run","usage":{}}"#).unwrap();
        assert_eq!(
            serde_json::to_value(value).unwrap()["usage"],
            serde_json::json!({})
        );
    }
    #[test]
    fn reject_invalid_measurements() {
        for raw in [
            r#"{"requests":true}"#,
            r#"{"requests":-1}"#,
            r#"{"requests":1.5}"#,
            r#"{"cost":1}"#,
        ] {
            assert!(serde_json::from_str::<RunUsage>(raw).is_err());
        }
        for raw in [
            r#"{"requests":9007199254740992}"#,
            r#"{"input_tokens":5,"output_tokens":2,"total_tokens":6}"#,
            r#"{"input_tokens":5,"total_tokens":4}"#,
        ] {
            assert!(serde_json::from_str::<RunUsage>(raw)
                .unwrap()
                .validate()
                .is_err());
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R814
    #[test]
    fn timings_validate_and_keep_the_empty_digest() {
        let without = RunCompletionInput {
            schema_version: 1,
            run_id: "run".into(),
            usage: None,
            timings: Vec::new(),
        };
        // A completion without timings encodes to the bytes it had before the field.
        assert_eq!(
            serde_json::to_string(&without).unwrap(),
            r#"{"schema_version":1,"run_id":"run"}"#
        );
        assert_eq!(
            rmp_serde::to_vec_named(&without).unwrap(),
            rmp_serde::to_vec_named(&serde_json::json!({"schema_version": 1, "run_id": "run"}))
                .unwrap()
        );
        assert!(without.validate().is_ok());

        let timed = |timings: Vec<PhaseTiming>| RunCompletionInput {
            timings,
            ..without.clone()
        };
        let phase = |phase, duration_ms| PhaseTiming { phase, duration_ms };

        // Four distinct phases pass, including the largest allowed duration.
        let four = timed(vec![
            phase(TimingPhase::Request, 5),
            phase(TimingPhase::Run, MAX_PHASE_DURATION_MS),
            phase(TimingPhase::Queue, 0),
            phase(TimingPhase::Tool, 9),
        ]);
        assert!(four.validate().is_ok());
        let round: RunCompletionInput =
            serde_json::from_str(&serde_json::to_string(&four).unwrap()).unwrap();
        assert_eq!(round, four);

        // A duplicate phase and an over-day duration refuse.
        let duplicate = timed(vec![phase(TimingPhase::Run, 1), phase(TimingPhase::Run, 2)]);
        assert_eq!(
            duplicate.validate().unwrap_err(),
            "INVALID_ARGUMENT: invalid run completion timing: run reported more than once"
        );
        let over = timed(vec![phase(TimingPhase::Tool, MAX_PHASE_DURATION_MS + 1)]);
        assert!(over
            .validate()
            .unwrap_err()
            .starts_with("INVALID_ARGUMENT: invalid run completion timing: tool"));
    }
}
