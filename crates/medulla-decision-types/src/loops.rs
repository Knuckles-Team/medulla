//! Improvement-loop records and the `Loops` op body (MEDULLA-DECISION-ENGINE-R653.1).
//!
//! A loop's `state` is a [`LoopState`], the one state list that
//! `loop_statechart_def()` in `medulla-store` builds its chart from. Severities and rates are parts per
//! million in `[0, 1_000_000]`.

use serde::{Deserialize, Serialize};

/// The upper bound of every `*_ppm` field.
pub const PPM_MAX: u32 = 1_000_000;

/// The fenced lease a loop holder owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LoopLease {
    /// The principal holding the lease.
    pub holder: String,
    /// The fencing epoch; every claim raises it by one.
    pub epoch: u64,
    /// Lease expiry, milliseconds since the epoch.
    pub expires_at_ms: u64,
}

/// The lifecycle state of a loop: the 16 states of the durable loop statechart
/// (`loop_statechart_def()` in `medulla-store`), which is the one machine
/// (MEDULLA-DECISION-ENGINE-R830.6). The stored and served strings are the snake_case
/// variant names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LoopState {
    /// Accepted, no holder yet; the initial state.
    Submitted,
    /// A holder is iterating.
    Running,
    /// Waiting on a callee.
    Pending,
    /// An evaluator is checking the iteration.
    Validating,
    /// Paused for a human.
    Paused,
    /// The lease was lost; re-claimable.
    Orphaned,
    /// The goal was met.
    Completed,
    /// Ended in failure.
    Failed,
    /// Cancelled by a human or the callee.
    Cancelled,
    /// Refused at intake.
    Rejected,
    /// The turn cap was reached.
    MaxIterationsExceeded,
    /// The budget was spent.
    BudgetExceeded,
    /// The wall clock deadline passed.
    WallClockExceeded,
    /// No progress was made.
    Stalled,
    /// The error threshold tripped.
    ErrorThresholdExceeded,
    /// The awaited external event fired.
    ExternalEventSatisfied,
}

impl LoopState {
    /// Every state: the non-final states, then the ten final states. The loop chart builds
    /// its state list from this array.
    pub const ALL: [LoopState; 16] = [
        Self::Submitted,
        Self::Running,
        Self::Pending,
        Self::Validating,
        Self::Paused,
        Self::Orphaned,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Rejected,
        Self::MaxIterationsExceeded,
        Self::BudgetExceeded,
        Self::WallClockExceeded,
        Self::Stalled,
        Self::ErrorThresholdExceeded,
        Self::ExternalEventSatisfied,
    ];

    /// The stored text of this state (the chart state id).
    pub fn stored(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Running => "running",
            Self::Pending => "pending",
            Self::Validating => "validating",
            Self::Paused => "paused",
            Self::Orphaned => "orphaned",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
            Self::MaxIterationsExceeded => "max_iterations_exceeded",
            Self::BudgetExceeded => "budget_exceeded",
            Self::WallClockExceeded => "wall_clock_exceeded",
            Self::Stalled => "stalled",
            Self::ErrorThresholdExceeded => "error_threshold_exceeded",
            Self::ExternalEventSatisfied => "external_event_satisfied",
        }
    }

    /// The state whose stored text is `stored`, if any.
    pub fn from_stored(stored: &str) -> Option<LoopState> {
        Self::ALL.into_iter().find(|state| state.stored() == stored)
    }

    /// Whether the state is final: it accepts no event.
    pub fn is_final(self) -> bool {
        !matches!(
            self,
            Self::Submitted
                | Self::Running
                | Self::Pending
                | Self::Validating
                | Self::Paused
                | Self::Orphaned
        )
    }

    /// Whether a lease holder acts in this state.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Pending | Self::Validating)
    }
}

/// One improvement loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LoopInstance {
    /// The owning tenant.
    pub tenant_id: String,
    /// The loop id, unique within the tenant.
    pub loop_id: String,
    /// Digest of the loop's objective.
    pub objective_digest: String,
    /// The loop's state in the durable loop statechart. `LoopStore::view_of` is the one
    /// writer (MEDULLA-DECISION-ENGINE-R830.6).
    pub state: LoopState,
    /// Scheduling priority; higher runs first.
    pub priority: u32,
    /// Creation time, milliseconds since the epoch.
    pub created_at_ms: u64,
    /// The current lease, if any.
    pub lease: Option<LoopLease>,
}

/// The settlement state of a gap: the closed state of `GapMachine`
/// (MEDULLA-DECISION-ENGINE-R830.5).
use medulla_types::gap_machine::GapState;

/// A capability gap a loop filed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Gap {
    /// The gap id.
    pub gap_id: String,
    /// The loop that filed it.
    pub loop_id: String,
    /// Severity in `[0, 1_000_000]` ppm.
    pub severity_ppm: u32,
    /// Evidence references for the gap.
    pub evidence: Vec<String>,
    /// Settlement state.
    pub state: GapState,
}

impl Gap {
    /// Whether `severity_ppm` lies in `[0, PPM_MAX]`.
    pub fn severity_in_range(&self) -> bool {
        self.severity_ppm <= PPM_MAX
    }
}

/// A work offer derived for a gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkOffer {
    /// The gap the offer serves.
    pub gap_id: String,
    /// Offer rate in ppm.
    pub rate_ppm: u32,
}

/// A liveness beacon: the stage a loop reached and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StageBeacon {
    /// The loop id.
    pub loop_id: String,
    /// The stage identifier the loop reports. It names a stage, it is not a lifecycle
    /// state (MEDULLA-DECISION-ENGINE-R830.9); the wire key stays `stage`.
    #[serde(rename = "stage")]
    pub stage_id: String,
    /// Beacon time, milliseconds since the epoch.
    pub at_ms: u64,
}

/// One ledger measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ImprovementSample {
    /// Sample time, milliseconds since the epoch.
    pub at_ms: u64,
    /// The measured metric.
    pub metric: String,
    /// The value in millionths.
    pub value_micro: i64,
}

/// A loop's measurement ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ImprovementLedger {
    /// The loop id.
    pub loop_id: String,
    /// Samples in recording order.
    pub samples: Vec<ImprovementSample>,
}

/// The capability ratchet's verdict on a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RatchetVerdict {
    /// The change held its gain.
    Keep,
    /// The change is rolled back.
    Revert,
}

/// A mined failure subsequence and its support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MinedPattern {
    /// The contiguous step sequence.
    pub sequence: Vec<String>,
    /// The number of failure sequences containing it.
    pub support: u32,
}

/// One state change of a loop with its actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LoopTransition {
    /// The lease epoch current after the change.
    pub epoch: u64,
    /// The event that fired.
    pub event: String,
    pub from: String,
    pub to: String,
    pub at_ms: u64,
    /// The verified principal that caused it.
    pub actor: String,
}

/// The inputs an offer was derived from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OfferEvidence {
    pub gap_id: String,
    pub rate_ppm: u32,
    pub severity_ppm: u32,
    pub saturation_ppm: u32,
    pub open_offers: u32,
    pub capacity: u32,
}

/// The ratchet comparison with both means and sample counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RatchetEvidence {
    pub metric: String,
    pub change_at_ms: u64,
    pub before_mean_micro: i64,
    pub before_samples: u32,
    pub after_mean_micro: i64,
    pub after_samples: u32,
    pub verdict: RatchetVerdict,
}

/// A loop with all its evidence, as every `Loops` op answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LoopView {
    pub instance: LoopInstance,
    pub gaps: Vec<Gap>,
    pub offers: Vec<OfferEvidence>,
    pub beacons: Vec<StageBeacon>,
    pub ledger: ImprovementLedger,
    pub patterns: Vec<MinedPattern>,
    pub ratchets: Vec<RatchetEvidence>,
    pub transitions: Vec<LoopTransition>,
}

impl LoopOp {
    /// Whether the op changes durable state.
    pub fn is_mutation(&self) -> bool {
        !matches!(self, LoopOp::Get { .. })
    }

    /// The authorization action the op needs.
    pub fn authz_action(&self) -> &'static str {
        if self.is_mutation() {
            "agent:loop-write"
        } else {
            "agent:loop-read"
        }
    }
}

/// The `Loops` op body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LoopOp {
    /// Take the loop's lease. The first claim of an unknown loop creates it
    /// with `objective_digest` and `priority`; later claims ignore both.
    Claim {
        loop_id: String,
        #[serde(default)]
        objective_digest: String,
        #[serde(default)]
        priority: u32,
    },
    /// Give the lease back.
    Release { loop_id: String, epoch: u64 },
    /// Report the stage reached.
    Beacon {
        loop_id: String,
        epoch: u64,
        stage: String,
    },
    /// File a capability gap.
    FileGap {
        loop_id: String,
        epoch: u64,
        gap_id: String,
        severity_ppm: u32,
        evidence: Vec<String>,
    },
    /// Derive a work offer for an open gap.
    Offer {
        loop_id: String,
        epoch: u64,
        gap_id: String,
    },
    /// Settle an offered gap.
    Settle {
        loop_id: String,
        epoch: u64,
        gap_id: String,
    },
    /// Append one ledger sample.
    RecordSample {
        loop_id: String,
        epoch: u64,
        sample: ImprovementSample,
    },
    /// Ask for a ratchet verdict around a change time.
    Ratchet {
        loop_id: String,
        epoch: u64,
        metric: String,
        change_at_ms: u64,
    },
    /// Mine failure sequences for repeated patterns.
    Mine {
        loop_id: String,
        epoch: u64,
        failure_sequences: Vec<Vec<String>>,
    },
    /// Read a loop and its evidence.
    Get { loop_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("encode");
        let back: T = serde_json::from_str(&json).expect("decode");
        assert_eq!(&back, value);
    }

    // spec: MEDULLA-DECISION-ENGINE-R653.1
    #[test]
    fn loop_types_round_trip_and_deny_unknown_fields() {
        let instance = LoopInstance {
            tenant_id: "tenant-a".into(),
            loop_id: "loop-1".into(),
            objective_digest: "sha256:00".into(),
            state: LoopState::Submitted,
            priority: 5,
            created_at_ms: 10,
            lease: Some(LoopLease {
                holder: "worker-1".into(),
                epoch: 1,
                expires_at_ms: 60_010,
            }),
        };
        round_trip(&instance);
        let gap = Gap {
            gap_id: "gap-1".into(),
            loop_id: "loop-1".into(),
            severity_ppm: PPM_MAX,
            evidence: vec!["run:1".into()],
            state: GapState::Offered,
        };
        assert!(gap.severity_in_range());
        assert!(!Gap {
            severity_ppm: PPM_MAX + 1,
            ..gap.clone()
        }
        .severity_in_range());
        round_trip(&gap);
        round_trip(&WorkOffer {
            gap_id: "gap-1".into(),
            rate_ppm: 500_000,
        });
        round_trip(&StageBeacon {
            loop_id: "loop-1".into(),
            stage_id: "learn".into(),
            at_ms: 3,
        });
        let sample = ImprovementSample {
            at_ms: 4,
            metric: "pass_rate".into(),
            value_micro: -7,
        };
        round_trip(&ImprovementLedger {
            loop_id: "loop-1".into(),
            samples: vec![sample.clone()],
        });
        round_trip(&RatchetVerdict::Keep);
        round_trip(&MinedPattern {
            sequence: vec!["a".into(), "b".into()],
            support: 3,
        });
        for op in [
            LoopOp::Claim {
                loop_id: "loop-1".into(),
                objective_digest: "sha256:00".into(),
                priority: 5,
            },
            LoopOp::RecordSample {
                loop_id: "loop-1".into(),
                epoch: 1,
                sample,
            },
            LoopOp::Mine {
                loop_id: "loop-1".into(),
                epoch: 1,
                failure_sequences: vec![vec!["a".into()]],
            },
        ] {
            round_trip(&op);
        }
        assert_eq!(
            serde_json::to_value(GapState::Settled).unwrap(),
            serde_json::json!("settled")
        );

        let mut extra = serde_json::to_value(&instance).unwrap();
        extra["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<LoopInstance>(extra).is_err());
        let mut extra = serde_json::to_value(&gap).unwrap();
        extra["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Gap>(extra).is_err());
        assert!(serde_json::from_value::<LoopOp>(
            serde_json::json!({"op": "claim", "loop_id": "l", "surprise": 1})
        )
        .is_err());
        assert!(serde_json::from_value::<LoopOp>(serde_json::json!({"op": "unknown"})).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R830.6
    #[test]
    fn loop_state_round_trips_every_stored_form() {
        assert_eq!(LoopState::ALL.len(), 16);
        assert_eq!(LoopState::ALL.iter().filter(|s| s.is_final()).count(), 10);
        assert_eq!(LoopState::ALL.iter().filter(|s| s.is_active()).count(), 3);
        for state in LoopState::ALL {
            let stored = state.stored();
            assert_eq!(LoopState::from_stored(stored), Some(state));
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(stored)
            );
            assert_eq!(
                serde_json::from_value::<LoopState>(serde_json::json!(stored)).unwrap(),
                state
            );
        }
        assert_eq!(LoopState::from_stored("nonsense"), None);
        assert!(serde_json::from_value::<LoopState>(serde_json::json!("nonsense")).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R830.9
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        // The census named `StageBeacon.stage: String`. A beacon is an append-only report of a
        // stage identifier; it has no lifecycle. The Rust field is `stage_id`, the wire key is
        // unchanged, and the field is only set where a beacon is constructed.
        let beacon: StageBeacon = serde_json::from_value(serde_json::json!({
            "loop_id": "l", "stage": "learn", "at_ms": 1
        }))
        .unwrap();
        assert_eq!(beacon.stage_id, "learn");
        assert_eq!(
            serde_json::to_value(&beacon).unwrap(),
            serde_json::json!({"loop_id": "l", "stage": "learn", "at_ms": 1})
        );
        let source = include_str!("loops.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("pub stage: String"));
    }
}
