//! Run traces as typed facts and runtime quotas (MEDULLA-DECISION-ENGINE-R133.1,
//! R133.2).
//!
//! A runtime quota bounds one run: wall time, memory, process count and the
//! grace period before a kill. A request may only tighten the default for its
//! task class and can never exceed the fixed ceiling. A run trace is the typed
//! record of what one run did, step by step.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_types::contract::BoundedVec;

/// The most steps one run trace carries.
pub const MAX_TRACE_STEPS: usize = 256;

/// The resolved bounds of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RuntimeQuota {
    pub wall_time_ms: u64,
    pub memory_bytes: u64,
    pub process_count: u64,
    pub grace_ms: u64,
}

/// The bounds a caller asks for; an omitted field keeps the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QuotaRequest {
    pub wall_time_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub process_count: Option<u64>,
    pub grace_ms: Option<u64>,
}

/// Most arguments one trace step carries.
pub const MAX_STEP_ARGUMENTS: usize = 32;

/// One scalar a step's action was called with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TraceScalar {
    Bool(bool),
    Int(i64),
    Text(String),
}

/// One named argument of a step's action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceArgument {
    pub name: String,
    pub value: TraceScalar,
}

/// What kind of step a trace step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StepKind {
    ToolCall,
    ModelCall,
    Other,
}

/// How one step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StepOutcome {
    Ok,
    Error,
}

/// How a whole run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// One step of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceStep {
    pub index: u32,
    pub kind: StepKind,
    pub action_ref: String,
    pub outcome: StepOutcome,
    /// The scalars the action ran with, in call order. Omitted when empty, so a
    /// step without arguments keeps its serialized form.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub arguments: BoundedVec<TraceArgument, MAX_STEP_ARGUMENTS>,
}

/// The typed record of one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunTraceRecord {
    pub schema_version: u16,
    pub run_id: String,
    pub task_signature: String,
    pub model_id: String,
    pub steps: BoundedVec<TraceStep, MAX_TRACE_STEPS>,
    pub outcome: RunOutcome,
    pub started_at_ms: u64,
}

/// The four bounded quantities of a quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum QuotaField {
    WallTimeMs,
    MemoryBytes,
    ProcessCount,
    GraceMs,
}

impl QuotaField {
    /// Every field, in the order the answer names them.
    pub const ALL: [Self; 4] = [
        Self::WallTimeMs,
        Self::MemoryBytes,
        Self::ProcessCount,
        Self::GraceMs,
    ];

    /// The wire name of the field.
    pub fn name(self) -> &'static str {
        match self {
            Self::WallTimeMs => "wall_time_ms",
            Self::MemoryBytes => "memory_bytes",
            Self::ProcessCount => "process_count",
            Self::GraceMs => "grace_ms",
        }
    }
}

impl RuntimeQuota {
    /// The value of `field`.
    pub fn get(&self, field: QuotaField) -> u64 {
        match field {
            QuotaField::WallTimeMs => self.wall_time_ms,
            QuotaField::MemoryBytes => self.memory_bytes,
            QuotaField::ProcessCount => self.process_count,
            QuotaField::GraceMs => self.grace_ms,
        }
    }

    fn set(&mut self, field: QuotaField, value: u64) {
        match field {
            QuotaField::WallTimeMs => self.wall_time_ms = value,
            QuotaField::MemoryBytes => self.memory_bytes = value,
            QuotaField::ProcessCount => self.process_count = value,
            QuotaField::GraceMs => self.grace_ms = value,
        }
    }
}

impl QuotaRequest {
    /// The value the caller asked for on `field`, if any.
    pub fn get(&self, field: QuotaField) -> Option<u64> {
        match field {
            QuotaField::WallTimeMs => self.wall_time_ms,
            QuotaField::MemoryBytes => self.memory_bytes,
            QuotaField::ProcessCount => self.process_count,
            QuotaField::GraceMs => self.grace_ms,
        }
    }
}

/// A requested quota above the fixed ceiling.
pub const QUOTA_ABOVE_CEILING: RefusalCode = RefusalCode::new("QUOTA_ABOVE_CEILING");

/// The `REFUSALS` table of the family, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[QUOTA_ABOVE_CEILING];

/// Why a quota request is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaRefusal {
    /// The request asked for more than the ceiling on `field`.
    AboveCeiling { field: QuotaField },
}

impl DecisionRefusal for QuotaRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::AboveCeiling { .. } => QUOTA_ABOVE_CEILING,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::AboveCeiling { field } => format!("field={}", field.name()),
        }
    }
}

/// The quota a run gets: per field, a request above the ceiling refuses first
/// (before any clamping), otherwise the smaller of the default and the request,
/// or the default when the request omits the field. A request never raises a
/// value above its default.
pub fn decide_runtime_quota(
    defaults: &RuntimeQuota,
    ceilings: &RuntimeQuota,
    request: &QuotaRequest,
) -> Result<RuntimeQuota, QuotaRefusal> {
    for field in QuotaField::ALL {
        if request
            .get(field)
            .is_some_and(|asked| asked > ceilings.get(field))
        {
            return Err(QuotaRefusal::AboveCeiling { field });
        }
    }
    let mut decided = *defaults;
    for field in QuotaField::ALL {
        if let Some(asked) = request.get(field) {
            decided.set(field, asked.min(defaults.get(field)));
        }
    }
    Ok(decided)
}

/// The `quota.*` knobs (MEDULLA-DECISION-ENGINE-R133.2). Defaults are
/// `Tunable`; the ceilings are `Fixed`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "quota.wall_time_ms_default",
        unit: "ms",
        default: KnobValue::DurationMs(600_000),
        min: KnobValue::DurationMs(1_000),
        max: KnobValue::DurationMs(3_600_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "quota.memory_bytes_default",
        unit: "bytes",
        default: KnobValue::Integer(2_147_483_648),
        min: KnobValue::Integer(67_108_864),
        max: KnobValue::Integer(8_589_934_592),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "quota.process_count_default",
        unit: "count",
        default: KnobValue::Integer(32),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(128),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "quota.grace_ms_default",
        unit: "ms",
        default: KnobValue::DurationMs(10_000),
        min: KnobValue::DurationMs(0),
        max: KnobValue::DurationMs(60_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "quota.wall_time_ms_ceiling",
        unit: "ms",
        default: KnobValue::DurationMs(3_600_000),
        min: KnobValue::DurationMs(3_600_000),
        max: KnobValue::DurationMs(3_600_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "quota.memory_bytes_ceiling",
        unit: "bytes",
        default: KnobValue::Integer(8_589_934_592),
        min: KnobValue::Integer(8_589_934_592),
        max: KnobValue::Integer(8_589_934_592),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "quota.process_count_ceiling",
        unit: "count",
        default: KnobValue::Integer(128),
        min: KnobValue::Integer(128),
        max: KnobValue::Integer(128),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "quota.grace_ms_ceiling",
        unit: "ms",
        default: KnobValue::DurationMs(60_000),
        min: KnobValue::DurationMs(60_000),
        max: KnobValue::DurationMs(60_000),
        safety: KnobSafety::Fixed,
    },
];

/// The numeric value of the knob `name` of [`KNOBS`] under `policy`.
pub fn knob_u64(policy: &super::policy::DecisionPolicy, index: usize) -> u64 {
    match policy.knob(&KNOBS[index]) {
        KnobValue::DurationMs(v) => v,
        KnobValue::Integer(v) => u64::try_from(v).unwrap_or(0),
        KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(0),
    }
}

/// The defaults and ceilings `policy` resolves, from the `quota.*` knobs.
pub fn knob_quotas(policy: &super::policy::DecisionPolicy) -> (RuntimeQuota, RuntimeQuota) {
    let at = |offset: usize| RuntimeQuota {
        wall_time_ms: knob_u64(policy, offset),
        memory_bytes: knob_u64(policy, offset + 1),
        process_count: knob_u64(policy, offset + 2),
        grace_ms: knob_u64(policy, offset + 3),
    };
    (at(0), at(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(value: u64) -> RuntimeQuota {
        RuntimeQuota {
            wall_time_ms: value,
            memory_bytes: value,
            process_count: value,
            grace_ms: value,
        }
    }

    fn request(value: Option<u64>) -> QuotaRequest {
        QuotaRequest {
            wall_time_ms: value,
            memory_bytes: value,
            process_count: value,
            grace_ms: value,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.1
    #[test]
    fn quota_round_trips() {
        let trace = RunTraceRecord {
            schema_version: 1,
            run_id: "run-1".to_string(),
            task_signature: "sig".to_string(),
            model_id: "model".to_string(),
            steps: BoundedVec::new(vec![TraceStep {
                index: 0,
                kind: StepKind::ToolCall,
                action_ref: "tool:a".to_string(),
                outcome: StepOutcome::Ok,
                arguments: BoundedVec::default(),
            }])
            .unwrap(),
            outcome: RunOutcome::Succeeded,
            started_at_ms: 5,
        };
        let value = serde_json::to_value((quota(3), request(Some(4)), &trace)).unwrap();
        let back: (RuntimeQuota, QuotaRequest, RunTraceRecord) =
            serde_json::from_value(value).unwrap();
        assert_eq!(back, (quota(3), request(Some(4)), trace.clone()));

        let mut unknown = serde_json::to_value(quota(3)).unwrap();
        unknown["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<RuntimeQuota>(unknown).is_err());
        let mut unknown = serde_json::to_value(request(None)).unwrap();
        unknown["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<QuotaRequest>(unknown).is_err());
        let mut unknown = serde_json::to_value(&trace).unwrap();
        unknown["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<RunTraceRecord>(unknown).is_err());

        let step = &trace.steps.as_slice()[0];
        let mut long = serde_json::to_value(&trace).unwrap();
        long["steps"] = serde_json::Value::Array(vec![
            serde_json::to_value(step).unwrap();
            MAX_TRACE_STEPS + 1
        ]);
        assert!(serde_json::from_value::<RunTraceRecord>(long).is_err());
        let mut full = serde_json::to_value(&trace).unwrap();
        full["steps"] =
            serde_json::Value::Array(vec![serde_json::to_value(step).unwrap(); MAX_TRACE_STEPS]);
        assert!(serde_json::from_value::<RunTraceRecord>(full).is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.2
    #[test]
    fn request_only_tightens_quota() {
        let (defaults, ceilings) = (quota(10), quota(20));
        let decide =
            |asked: Option<u64>| decide_runtime_quota(&defaults, &ceilings, &request(asked));
        assert_eq!(decide(None).unwrap(), quota(10));
        assert_eq!(decide(Some(5)).unwrap(), quota(5));
        assert_eq!(decide(Some(15)).unwrap(), quota(10));
        assert_eq!(decide(Some(20)).unwrap(), quota(10));
        assert_eq!(
            decide(Some(21)),
            Err(QuotaRefusal::AboveCeiling {
                field: QuotaField::WallTimeMs
            })
        );
        // The ceiling check runs before any clamp, on every field.
        let one = QuotaRequest {
            grace_ms: Some(21),
            ..QuotaRequest::default()
        };
        assert_eq!(
            decide_runtime_quota(&defaults, &ceilings, &one),
            Err(QuotaRefusal::AboveCeiling {
                field: QuotaField::GraceMs
            })
        );
        assert_eq!(QUOTA_ABOVE_CEILING.as_str(), "QUOTA_ABOVE_CEILING");
        assert_eq!(
            QuotaRefusal::AboveCeiling {
                field: QuotaField::GraceMs
            }
            .code(),
            QUOTA_ABOVE_CEILING
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.2
    #[test]
    fn knob_defaults_are_within_ceilings() {
        let policy = super::super::policy::DecisionPolicy::engine_default();
        let (defaults, ceilings) = knob_quotas(&policy);
        for field in QuotaField::ALL {
            assert!(defaults.get(field) <= ceilings.get(field), "{field:?}");
        }
        assert_eq!(defaults.wall_time_ms, 600_000);
        assert_eq!(ceilings.process_count, 128);
    }
}
