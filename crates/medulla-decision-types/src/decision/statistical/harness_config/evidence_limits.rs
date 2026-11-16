//! Size limits of the sealed harness evidence (MEDULLA-DECISION-ENGINE-R128.13.1.2).
//!
//! The limits are decision knobs read from the pinned policy. An over-limit
//! record refuses with `HARNESS_EVIDENCE_OVER_LIMIT` before any value is
//! allocated past a limit: the encoded frame length is checked first, then
//! the decoded item and string sizes.

use super::{HarnessConfigAnswer, HarnessConfigEvaluationEvidence, HarnessPart};
use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// Refusal code of an over-limit evidence record.
pub const HARNESS_EVIDENCE_OVER_LIMIT: &str = "HARNESS_EVIDENCE_OVER_LIMIT";

const fn limit_knob(
    name: &'static str,
    unit: &'static str,
    default: i64,
    min: i64,
    max: i64,
) -> KnobSpec {
    KnobSpec {
        name,
        unit,
        default: KnobValue::Integer(default),
        min: KnobValue::Integer(min),
        max: KnobValue::Integer(max),
        safety: KnobSafety::Tunable,
    }
}

/// The `harness.evidence_*` knobs, in the field order of
/// [`HarnessEvidenceLimits`]. The byte upper bound is the wire frame limit.
pub const KNOBS: &[KnobSpec] = &[
    limit_knob("harness.evidence_max_items", "count", 4096, 64, 65_536),
    limit_knob(
        "harness.evidence_max_bytes",
        "bytes",
        16 * 1024 * 1024,
        64 * 1024,
        16 * 1024 * 1024,
    ),
    limit_knob(
        "harness.evidence_max_string_bytes",
        "bytes",
        64 * 1024,
        1024,
        1024 * 1024,
    ),
];

/// The three limits one evidence record is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessEvidenceLimits {
    pub max_items: u64,
    pub max_bytes: u64,
    pub max_string_bytes: u64,
}

fn over(what: &str, got: u64, limit: u64) -> String {
    format!("{HARNESS_EVIDENCE_OVER_LIMIT}: {what} {got} exceeds {limit}")
}

impl HarnessEvidenceLimits {
    /// The limits the pinned policy sets, each knob or its default.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u64::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            max_items: read(0),
            max_bytes: read(1),
            max_string_bytes: read(2),
        }
    }

    /// Refuse an encoded frame past `max_bytes` before it is decoded.
    pub fn check_frame(&self, frame_len: usize) -> Result<(), String> {
        let len = frame_len as u64;
        if len > self.max_bytes {
            return Err(over("evidence bytes", len, self.max_bytes));
        }
        Ok(())
    }

    /// Refuse a decoded record whose item count or any string is past a limit.
    pub fn check_evidence(&self, evidence: &HarnessConfigEvaluationEvidence) -> Result<(), String> {
        let mut items = 0u64;
        let mut strings: Vec<&str> = vec![
            &evidence.model_profile.component_id,
            &evidence.model_profile.definition_digest,
            &evidence.request.model_profile_ref,
            &evidence.request.task_class,
        ];
        let answer: &HarnessConfigAnswer = &evidence.answer;
        macro_rules! facts {
            ($part:expr) => {
                if let HarnessPart::Abstained { missing_fact, .. } = $part {
                    for fact in missing_fact.as_slice() {
                        items += 1;
                        strings.push(fact);
                    }
                }
            };
        }
        facts!(&answer.context_policy);
        facts!(&answer.soft_threshold);
        facts!(&answer.hard_threshold);
        facts!(&answer.recent_window_fraction);
        facts!(&answer.plan_scaffold);
        facts!(&answer.action_space);
        facts!(&answer.tool_subset);
        for tool in answer.tool_subset.selected().as_slice() {
            items += 1;
            strings.push(tool);
        }
        for capability in evidence.request.required_capabilities.as_slice() {
            items += 1;
            strings.push(capability);
        }
        for option in evidence.declared_options.as_slice() {
            items += 1;
            strings.push(&option.option_id);
            for capability in option.classification.as_slice() {
                items += 1;
                strings.push(capability);
            }
            for text in option.texts.as_slice() {
                strings.push(&text.key);
                strings.push(&text.text);
            }
        }
        if items > self.max_items {
            return Err(over("evidence items", items, self.max_items));
        }
        for text in strings {
            let len = text.len() as u64;
            if len > self.max_string_bytes {
                return Err(over("evidence string bytes", len, self.max_string_bytes));
            }
        }
        Ok(())
    }

    /// Decode one evidence frame under the limits: the frame length first, the
    /// decoded sizes second.
    pub fn decode(&self, frame: &[u8]) -> Result<HarnessConfigEvaluationEvidence, String> {
        self.check_frame(frame.len())?;
        let evidence: HarnessConfigEvaluationEvidence = serde_json::from_slice(frame)
            .map_err(|error| format!("harness evidence does not decode: {error}"))?;
        self.check_evidence(&evidence)?;
        Ok(evidence)
    }
}

#[cfg(test)]
mod tests;
