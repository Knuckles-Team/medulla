//! Improvement-loop records and the `Loops` op body (MEDULLA-DECISION-ENGINE-R653.1).
//!
//! A loop's `state` names a state of `loop_statechart_def()` in
//! `medulla-store`; that definition keeps the single state list, and the
//! `Loops` handler refuses any other value. Severities and rates are parts per
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
    /// A state of `loop_statechart_def()`.
    pub state: String,
    /// Scheduling priority; higher runs first.
    pub priority: u32,
    /// Creation time, milliseconds since the epoch.
    pub created_at_ms: u64,
    /// The current lease, if any.
    pub lease: Option<LoopLease>,
}

/// The settlement state of a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GapState {
    /// Filed, no offer yet.
    Open,
    /// An offer is out.
    Offered,
    /// Settled; never reopens.
    Settled,
}

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
    /// The stage name.
    pub stage: String,
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

/// The `Loops` op body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LoopOp {
    /// Take the loop's lease.
    Claim { loop_id: String },
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
            state: "submitted".into(),
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
            stage: "learn".into(),
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
}
