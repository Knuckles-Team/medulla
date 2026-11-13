//! Trajectory join, step verification and adversarial admission: the wire
//! model (MEDULLA-DECISION-ENGINE-R753.750).
//!
//! `Decide` joins competing trajectories into the strongest one, verifies a
//! trace step by step and chooses a recovery for the first failed step, and
//! admits a candidate against a bounded set of red-team probes. Digests are
//! SHA-256, 64 lowercase hex characters. The verdict carries no uncertainty
//! bound; the checks and `trace_digest` are the proof binding. Nothing here
//! reads a clock or a store.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::numeric::QuantisedValue;
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;
use super::record::EvidenceClass;
use medulla_types::contract::BoundedVec;

/// Most trajectories one join may take.
pub const MAX_TRAJECTORIES: usize = 256;
/// Most steps one trace may carry.
pub const MAX_TRACE_STEPS: usize = 1024;
/// Most probes one admission may carry.
pub const MAX_PROBES: usize = 64;

/// Whether `digest` is 64 lowercase hex characters.
pub fn is_sha256_hex(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The verdict of one red-team probe. It lives here, below the outcome types
/// that score probes, so the admission types can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProbeVerdict {
    Resisted,
    Compromised,
    Inconclusive,
}

/// One trajectory offered to a join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryEntry {
    pub trajectory_id: String,
    pub evidence: EvidenceClass,
    /// The verifier's score on `Q32`.
    pub verifier_score: QuantisedValue,
    pub trace_digest: String,
}

impl TrajectoryEntry {
    /// The validating constructor: the trace digest is 64 lowercase hex.
    pub fn new(
        trajectory_id: String,
        evidence: EvidenceClass,
        verifier_score: QuantisedValue,
        trace_digest: String,
    ) -> Result<Self, String> {
        if !is_sha256_hex(&trace_digest) {
            return Err("trace_digest is 64 lowercase hex characters".to_string());
        }
        Ok(Self {
            trajectory_id,
            evidence,
            verifier_score,
            trace_digest,
        })
    }
}

/// The result of a join: the winner and every source it was distilled from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DistilledTrajectory {
    pub trajectory_id: String,
    pub winner_id: String,
    /// Every source id, sorted ascending.
    pub source_ids: BoundedVec<String, MAX_TRAJECTORIES>,
    pub derivation_digest: String,
}

impl DistilledTrajectory {
    /// The validating constructor: sorted source ids, a hex digest.
    pub fn new(
        trajectory_id: String,
        winner_id: String,
        source_ids: Vec<String>,
        derivation_digest: String,
    ) -> Result<Self, String> {
        if source_ids.windows(2).any(|pair| pair[0] > pair[1]) {
            return Err("source_ids must be sorted".to_string());
        }
        if !is_sha256_hex(&derivation_digest) {
            return Err("derivation_digest is 64 lowercase hex characters".to_string());
        }
        Ok(Self {
            trajectory_id,
            winner_id,
            source_ids: BoundedVec::new(source_ids)?,
            derivation_digest,
        })
    }
}

/// One step of a trace: its id and the digest of its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceStep {
    pub step_id: String,
    pub digest: String,
}

/// One check run against a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepCheck {
    pub step_id: String,
    pub passed: bool,
    pub check_digest: String,
}

/// The verdict on one step, bound to the whole trace by `trace_digest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepVerdict {
    pub step_id: String,
    pub passed: bool,
    pub trace_digest: String,
    pub check_digests: BoundedVec<String, MAX_TRACE_STEPS>,
}

impl StepVerdict {
    /// The validating constructor: every digest is 64 lowercase hex.
    pub fn new(
        step_id: String,
        passed: bool,
        trace_digest: String,
        check_digests: Vec<String>,
    ) -> Result<Self, String> {
        if !is_sha256_hex(&trace_digest) || !check_digests.iter().all(|d| is_sha256_hex(d)) {
            return Err("every digest is 64 lowercase hex characters".to_string());
        }
        Ok(Self {
            step_id,
            passed,
            trace_digest,
            check_digests: BoundedVec::new(check_digests)?,
        })
    }
}

/// What to do after the first failed step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RecoveryAction {
    /// Run the failed step again.
    Retry { step_id: String },
    /// Return to the last step that passed.
    Rollback { step_id: String },
    /// Give up: no step passed.
    Abandon,
}

/// One probe and its verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProbeResult {
    pub probe_id: String,
    pub verdict: ProbeVerdict,
}

/// Whether a candidate is admitted after its probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AdmissionVerdict {
    Admit,
    Reject,
}

/// The refusal code an admission that is not admitted abstains with
/// (MEDULLA-DECISION-ENGINE-R753.755).
pub const PROBE_COMPROMISED: RefusalCode = RefusalCode::new("PROBE_COMPROMISED");

/// The `AdversarialAdmission` family's refusal table.
pub const REFUSALS: &[RefusalCode] = &[PROBE_COMPROMISED];

/// The `verify.*` knobs (MEDULLA-DECISION-ENGINE-R753.751): retries before a
/// rollback, probes per admission and inconclusive probes an admission allows.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "verify.max_retries",
        unit: "attempts",
        default: KnobValue::Integer(2),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(5),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "verify.probe_budget",
        unit: "probes",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "verify.max_inconclusive",
        unit: "count",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(0),
        safety: KnobSafety::Fixed,
    },
];

/// The pinned `verify.*` knob values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyKnobs {
    /// `knob verify.max_retries`, attempts.
    pub max_retries: u32,
    /// `knob verify.probe_budget`, probes.
    pub probe_budget: u32,
    /// `knob verify.max_inconclusive`, count (fixed at 0).
    pub max_inconclusive: u32,
}

impl VerifyKnobs {
    /// The knob values `policy` pins, each defaulting to its spec.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            max_retries: read(0),
            probe_budget: read(1),
            max_inconclusive: read(2),
        }
    }
}

/// The join question: the competing trajectories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JoinRequest {
    pub entries: BoundedVec<TrajectoryEntry, MAX_TRAJECTORIES>,
}

/// The verification question: a trace, one check per step, and the attempts
/// already spent on the step that fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VerifyRequest {
    pub steps: BoundedVec<TraceStep, MAX_TRACE_STEPS>,
    pub checks: BoundedVec<StepCheck, MAX_TRACE_STEPS>,
    #[serde(default)]
    pub attempts: u32,
}

/// The admission question: the probe results of one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdmissionRequest {
    pub probes: BoundedVec<ProbeResult, MAX_PROBES>,
}

/// The answer to a verification: the verdict and, for a failed step, the
/// recovery it selects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepVerification {
    pub verdict: StepVerdict,
    #[serde(default)]
    pub recovery: Option<RecoveryAction>,
}

#[cfg(test)]
mod tests {
    use super::super::numeric::QuantScaleTag;
    use super::*;

    fn hex(n: u8) -> String {
        format!("{n:064x}")
    }

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let text = serde_json::to_string(v).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&text).unwrap(), v);
    }

    // spec: MEDULLA-DECISION-ENGINE-R753.750
    #[test]
    fn verdict_types_round_trip() {
        let entry = TrajectoryEntry::new(
            "t1".into(),
            EvidenceClass::Observation,
            QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 1 << 31,
            },
            hex(1),
        )
        .unwrap();
        round_trip(&entry);
        let distilled = DistilledTrajectory::new(
            "d1".into(),
            "t1".into(),
            vec!["t1".into(), "t2".into()],
            hex(2),
        )
        .unwrap();
        round_trip(&distilled);
        round_trip(&TraceStep {
            step_id: "s1".into(),
            digest: hex(3),
        });
        round_trip(&StepCheck {
            step_id: "s1".into(),
            passed: true,
            check_digest: hex(4),
        });
        let verdict = StepVerdict::new("s1".into(), false, hex(5), vec![hex(4)]).unwrap();
        round_trip(&verdict);
        for action in [
            RecoveryAction::Retry {
                step_id: "s1".into(),
            },
            RecoveryAction::Rollback {
                step_id: "s0".into(),
            },
            RecoveryAction::Abandon,
        ] {
            round_trip(&action);
        }
        for verdict in [
            ProbeVerdict::Resisted,
            ProbeVerdict::Compromised,
            ProbeVerdict::Inconclusive,
        ] {
            round_trip(&ProbeResult {
                probe_id: "p1".into(),
                verdict,
            });
        }
        for admission in [AdmissionVerdict::Admit, AdmissionVerdict::Reject] {
            round_trip(&admission);
        }
        for kind in [
            crate::decision::QuestionKind::TrajectoryJoin,
            crate::decision::QuestionKind::StepVerification,
            crate::decision::QuestionKind::AdversarialAdmission,
        ] {
            round_trip(&kind);
        }
        assert_eq!(
            serde_json::to_string(&crate::decision::QuestionKind::AdversarialAdmission).unwrap(),
            "\"adversarial_admission\""
        );
        assert!(serde_json::from_str::<ProbeVerdict>("\"breached\"").is_err());

        // A digest that is not 64 lowercase hex characters is refused, and so
        // are unsorted source ids.
        assert!(TrajectoryEntry::new(
            "t".into(),
            EvidenceClass::Claim,
            entry.verifier_score,
            "ABC".into()
        )
        .is_err());
        assert!(TrajectoryEntry::new(
            "t".into(),
            EvidenceClass::Claim,
            entry.verifier_score,
            hex(10).to_uppercase()
        )
        .is_err());
        assert!(DistilledTrajectory::new(
            "d".into(),
            "t1".into(),
            vec!["t2".into(), "t1".into()],
            hex(2)
        )
        .is_err());
        assert!(StepVerdict::new("s".into(), true, hex(5), vec!["zz".into()]).is_err());
    }
}
