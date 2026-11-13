//! Compiled programs keyed by task signature: the model
//! (MEDULLA-DECISION-ENGINE-R135.1).
//!
//! A successful run compiles into a stored state-machine program: each step
//! names a state, an action and the state it moves to, and every literal of the
//! run became a named parameter. `Decide` with
//! [`QuestionKind::ProgramLookup`](super::statistical::QuestionKind::ProgramLookup)
//! returns the stored program for a task signature and writes nothing. Nothing
//! here reads a clock or a store.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::run_governance::{RunOutcome, RunTraceRecord, TraceScalar};
use medulla_agent_types::agent_component::ComponentDependency;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod insight;
pub mod preplan;

/// Most steps one compiled program may hold. A `Fixed` safety bound.
pub const MAX_PROGRAM_STEPS: usize = 40;
/// Most parameters one compiled program may hold.
pub const MAX_PROGRAM_PARAMETERS: usize = 64;
/// Current wire schema version of [`CompiledProgram`].
pub const PROGRAM_SCHEMA_VERSION: u32 = 1;

/// Refusal: a program of more than [`MAX_PROGRAM_STEPS`] steps.
pub const PROGRAM_TOO_LONG: RefusalCode = RefusalCode::new("PROGRAM_TOO_LONG");

/// Refusal: a run that did not succeed is never compiled.
pub const RUN_NOT_SUCCESSFUL: RefusalCode = RefusalCode::new("RUN_NOT_SUCCESSFUL");
/// Abstain reason: no program is stored for the signature.
pub const NO_PROGRAM: RefusalCode = RefusalCode::new("NO_PROGRAM");

/// One run-record commit (`DecisionLogOp::RunRecord`,
/// MEDULLA-DECISION-ENGINE-R135.3). A closed enum: later rows add variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RunRecord {
    /// Compile one successful run into a stored program. `policy` names the
    /// decision policy whose `program.*` knobs bound the compilation.
    ///
    /// `reusable` asks to promote the program to a reusable pattern
    /// (MEDULLA-DECISION-ENGINE-R135.7): it needs the `plan_binding` the plan
    /// was made under and a `preplan_receipt` that verifies against it. Both
    /// live on this request, never in the stored program body.
    CompiledProgram {
        trace: RunTraceRecord,
        policy: crate::decision::request::DecisionPolicyRef,
        #[serde(default)]
        reusable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_binding: Option<preplan::PlanBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preplan_receipt: Option<preplan::PrePlanTestReceipt>,
    },
    /// Store one run-derived insight (MEDULLA-DECISION-ENGINE-R135.12). The
    /// wire value is checked by [`insight::Insight::from_value`], so an insight
    /// with no source run refuses with `MISSING_SOURCE_RUN`.
    Insight { insight: serde_json::Value },
    /// Store one horizon's credit checkpoint (MEDULLA-DECISION-ENGINE-R134.11).
    /// `plan` is the plan `Decide` answered; the commit re-derives its horizon
    /// partition under `policy` and refuses a checkpoint of another revision.
    HorizonCheckpoint {
        plan: Box<super::task_plan::TaskPlan>,
        policy: crate::decision::request::DecisionPolicyRef,
        checkpoint: super::task_plan::HorizonCheckpoint,
    },
    /// Store one branch's outcome (MEDULLA-DECISION-ENGINE-R134.14).
    BranchRecord {
        record: super::branch_fusion::BranchRecord,
    },
    /// Store what a later branch consumed (MEDULLA-DECISION-ENGINE-R134.14).
    BranchRetrieval {
        receipt: super::branch_fusion::BranchRetrievalReceipt,
    },
}

/// What a committed [`RunRecord`] stored: the compiled program or the insight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StoredRun {
    /// The compiled program as stored.
    CompiledProgram {
        program: CompiledProgram,
        /// The reusable pattern the commit promoted, when it asked for one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<preplan::ReusablePattern>,
    },
    /// The insight as stored.
    Insight { insight: insight::Insight },
    /// The checkpoint as stored.
    HorizonCheckpoint {
        checkpoint: super::task_plan::HorizonCheckpoint,
    },
    /// The branch record as stored.
    BranchRecord {
        record: super::branch_fusion::BranchRecord,
    },
    /// The retrieval receipt as stored.
    BranchRetrieval {
        receipt: super::branch_fusion::BranchRetrievalReceipt,
    },
}

/// The answer of a committed [`RunRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunRecordStored {
    /// The artifact key the record is stored under.
    pub key: String,
    /// The record as stored.
    pub stored: StoredRun,
    /// Whether the commit answered an existing row with equal content.
    pub replayed: bool,
}

/// The decision-artifact key of the program stored for `task_signature`.
pub fn program_key(task_signature: &str) -> String {
    format!("run_record/compiled_program/{task_signature}")
}

/// A lookup that finds no stored program for the signature: the abstain reason
/// `NO_PROGRAM` (MEDULLA-DECISION-ENGINE-R135.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoProgram {
    pub task_signature: String,
}

impl crate::decision::record::outcome::DecisionRefusal for NoProgram {
    fn code(&self) -> RefusalCode {
        NO_PROGRAM
    }

    fn detail(&self) -> String {
        format!(
            "no program is stored for task_signature `{}`",
            self.task_signature
        )
    }
}

/// Every refusal code of the program-lookup family; joins
/// `codes::FAMILY_TABLES` under `QuestionKind::ProgramLookup`.
pub const REFUSALS: &[RefusalCode] = &[
    PROGRAM_TOO_LONG,
    RUN_NOT_SUCCESSFUL,
    NO_PROGRAM,
    preplan::PREPLAN_RECEIPT_MISSING,
    preplan::PREPLAN_COMMAND_MISMATCH,
    preplan::PREPLAN_RECEIPT_STALE,
    preplan::PREPLAN_ARTIFACTS_UNREADABLE,
    preplan::PREPLAN_ARTIFACTS_EMPTY,
    preplan::PREPLAN_TESTS_FAILED,
    RefusalCode::new(super::task_plan::horizon::HORIZON_BUDGET_RULE_INVALID),
    RefusalCode::new(super::task_plan::horizon::HORIZON_OVER_CREDIT),
    RefusalCode::new(super::task_plan::horizon::HORIZON_CHECKPOINT_MISSING),
    RefusalCode::new(super::task_plan::horizon::HORIZON_PLAN_STALE),
];

/// The `program.*` knob (MEDULLA-DECISION-ENGINE-R135.2). The hard cap
/// [`MAX_PROGRAM_STEPS`] stays `Fixed`; the knob only lowers it.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "program.max_steps",
    unit: "steps",
    default: KnobValue::Integer(MAX_PROGRAM_STEPS as i64),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(MAX_PROGRAM_STEPS as i64),
    safety: KnobSafety::Tunable,
}];

/// The resolved values of the `program.*` knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramKnobs {
    pub max_steps: usize,
}

impl ProgramKnobs {
    /// The values `policy` resolves, clamped to `[1, MAX_PROGRAM_STEPS]`.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let value = match policy.knob(&KNOBS[0]) {
            KnobValue::Integer(v) => v,
            KnobValue::DecimalMicros(v) => v,
            KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
        };
        let max_steps = usize::try_from(value)
            .unwrap_or(1)
            .clamp(1, MAX_PROGRAM_STEPS);
        Self { max_steps }
    }
}

/// The type of one program parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProgramParameterKind {
    Text,
    Int,
    Bool,
}

/// One parameter a run's literal became.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramParameter {
    pub name: String,
    pub kind: ProgramParameterKind,
}

/// One step of the state-machine program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramStep {
    pub state: String,
    pub action: String,
    /// The state this step moves to; `None` ends the program.
    #[serde(default)]
    pub next: Option<String>,
    /// The agent configuration the step runs with, pinned by revision digest.
    #[serde(default)]
    pub agent_config: Option<ComponentDependency>,
}

/// A compiled program for one task signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CompiledProgram {
    pub schema_version: u32,
    pub task_signature: String,
    pub steps: BoundedVec<ProgramStep, MAX_PROGRAM_STEPS>,
    pub parameters: BoundedVec<ProgramParameter, MAX_PROGRAM_PARAMETERS>,
    /// The successful run the program was compiled from.
    pub source_run_id: String,
    /// SHA-256 (64 lowercase hex characters) over the canonical bytes.
    pub program_digest: String,
}

/// Why a program was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramError {
    /// More steps than the cap allows (`PROGRAM_TOO_LONG`).
    TooLong { steps: usize },
    /// The run did not succeed (`RUN_NOT_SUCCESSFUL`).
    RunNotSuccessful { outcome: RunOutcome },
    /// The wire value did not decode.
    Malformed(String),
    /// A field broke an invariant.
    Invalid(String),
}

impl ProgramError {
    /// The registered refusal code, when the error is one.
    pub fn code(&self) -> Option<RefusalCode> {
        match self {
            Self::TooLong { .. } => Some(PROGRAM_TOO_LONG),
            Self::RunNotSuccessful { .. } => Some(RUN_NOT_SUCCESSFUL),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProgramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong { steps } => write!(
                f,
                "{}: {steps} steps exceed {MAX_PROGRAM_STEPS}",
                PROGRAM_TOO_LONG.as_str()
            ),
            Self::RunNotSuccessful { outcome } => write!(
                f,
                "{}: the run ended {outcome:?}",
                RUN_NOT_SUCCESSFUL.as_str()
            ),
            Self::Malformed(detail) => write!(f, "program is malformed: {detail}"),
            Self::Invalid(detail) => write!(f, "program is invalid: {detail}"),
        }
    }
}

impl std::error::Error for ProgramError {}

impl CompiledProgram {
    /// Decode a wire value. A program of more than [`MAX_PROGRAM_STEPS`] steps
    /// is `PROGRAM_TOO_LONG`, whatever else is wrong with it.
    pub fn from_value(value: serde_json::Value) -> Result<Self, ProgramError> {
        if let Some(steps) = value.get("steps").and_then(|steps| steps.as_array()) {
            if steps.len() > MAX_PROGRAM_STEPS {
                return Err(ProgramError::TooLong { steps: steps.len() });
            }
        }
        serde_json::from_value(value).map_err(|error| ProgramError::Malformed(error.to_string()))
    }

    /// The digest over the canonical bytes: compact JSON of the program with
    /// `program_digest` cleared, lowercase hex.
    pub fn compute_digest(&self) -> String {
        let mut canonical = self.clone();
        canonical.program_digest = String::new();
        let bytes = serde_json::to_vec(&canonical).expect("a program is plain data");
        hex::encode(Sha256::digest(bytes))
    }

    /// Set `program_digest` from the content.
    pub fn sealed(mut self) -> Self {
        self.program_digest = self.compute_digest();
        self
    }

    /// Check every invariant: schema version, non-empty signature and steps,
    /// unique state names, `next` naming a declared state, unique parameter
    /// names and a digest that matches the content.
    pub fn validate(&self) -> Result<(), ProgramError> {
        let invalid = |detail: &str| Err(ProgramError::Invalid(detail.to_string()));
        if self.schema_version != PROGRAM_SCHEMA_VERSION {
            return invalid("unsupported schema_version");
        }
        if self.task_signature.trim().is_empty() || self.source_run_id.trim().is_empty() {
            return invalid("task_signature and source_run_id are required");
        }
        if self.steps.is_empty() {
            return invalid("a program holds at least one step");
        }
        let states: std::collections::BTreeSet<&str> =
            self.steps.iter().map(|step| step.state.as_str()).collect();
        if states.len() != self.steps.len() || states.contains("") {
            return invalid("step states are unique and non-empty");
        }
        if self.steps.iter().any(|step| step.action.trim().is_empty()) {
            return invalid("every step names an action");
        }
        if self
            .steps
            .iter()
            .filter_map(|step| step.next.as_deref())
            .any(|next| !states.contains(next))
        {
            return invalid("a step moves to a state the program does not declare");
        }
        let names: std::collections::BTreeSet<&str> = self
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect();
        if names.len() != self.parameters.len() || names.contains("") {
            return invalid("parameter names are unique and non-empty");
        }
        let well_formed = self.program_digest.len() == 64
            && self
                .program_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !well_formed || self.program_digest != self.compute_digest() {
            return invalid("program_digest does not match the content");
        }
        Ok(())
    }
}

/// Compile one successful run into a program. Every string, integer or
/// boolean scalar in a step's arguments is a literal: it becomes a parameter
/// `p0`, `p1`, ... named by first occurrence, and equal literals (same kind and
/// value) share one name. Step `k` becomes state `s<k>`, moving to `s<k+1>`,
/// and the last step ends the program. A step's action is its `action_ref`
/// followed by its arguments with each literal replaced by its parameter name,
/// so two runs that differ only in literals compile to the same program.
pub fn compile_program(
    trace: &RunTraceRecord,
    knobs: &ProgramKnobs,
) -> Result<CompiledProgram, ProgramError> {
    if trace.outcome != RunOutcome::Succeeded {
        return Err(ProgramError::RunNotSuccessful {
            outcome: trace.outcome,
        });
    }
    let cap = knobs.max_steps.min(MAX_PROGRAM_STEPS);
    if trace.steps.len() > cap {
        return Err(ProgramError::TooLong {
            steps: trace.steps.len(),
        });
    }
    let mut seen: Vec<(ProgramParameterKind, String)> = Vec::new();
    let mut steps = Vec::with_capacity(trace.steps.len());
    for (k, step) in trace.steps.iter().enumerate() {
        let mut args = Vec::new();
        for argument in &step.arguments {
            let (kind, text) = match &argument.value {
                TraceScalar::Text(text) => (ProgramParameterKind::Text, text.clone()),
                TraceScalar::Int(int) => (ProgramParameterKind::Int, int.to_string()),
                TraceScalar::Bool(flag) => (ProgramParameterKind::Bool, flag.to_string()),
            };
            let slot = match seen.iter().position(|known| *known == (kind, text.clone())) {
                Some(slot) => slot,
                None => {
                    seen.push((kind, text));
                    seen.len() - 1
                }
            };
            args.push(format!("{}=p{slot}", argument.name));
        }
        let action = if args.is_empty() {
            step.action_ref.clone()
        } else {
            format!("{}({})", step.action_ref, args.join(","))
        };
        steps.push(ProgramStep {
            state: format!("s{k}"),
            action,
            next: (k + 1 < trace.steps.len()).then(|| format!("s{}", k + 1)),
            agent_config: None,
        });
    }
    let parameters = seen
        .iter()
        .enumerate()
        .map(|(slot, (kind, _))| ProgramParameter {
            name: format!("p{slot}"),
            kind: *kind,
        })
        .collect::<Vec<_>>();
    let too_many = |count: usize| {
        ProgramError::Invalid(format!(
            "{count} parameters exceed {MAX_PROGRAM_PARAMETERS}"
        ))
    };
    let count = parameters.len();
    let program = CompiledProgram {
        schema_version: PROGRAM_SCHEMA_VERSION,
        task_signature: trace.task_signature.clone(),
        steps: BoundedVec::new(steps).map_err(ProgramError::Invalid)?,
        parameters: BoundedVec::new(parameters).map_err(|_| too_many(count))?,
        source_run_id: trace.run_id.clone(),
        program_digest: String::new(),
    }
    .sealed();
    program.validate()?;
    Ok(program)
}

/// The stored program for `task_signature`, or none.
pub fn lookup_program<'a>(
    artifacts: &'a [CompiledProgram],
    task_signature: &str,
) -> Option<&'a CompiledProgram> {
    artifacts
        .iter()
        .find(|program| program.task_signature == task_signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::AgentComponentKind;

    fn step(state: &str, next: Option<&str>) -> ProgramStep {
        ProgramStep {
            state: state.into(),
            action: format!("do {state}"),
            next: next.map(str::to_string),
            agent_config: Some(ComponentDependency {
                component_id: "agent-1".into(),
                kind: AgentComponentKind::Tool,
                definition_digest: "sha256:00".into(),
            }),
        }
    }

    fn program(count: usize) -> CompiledProgram {
        let steps = (0..count)
            .map(|index| {
                let next = (index + 1 < count).then(|| format!("s{}", index + 1));
                step(&format!("s{index}"), next.as_deref())
            })
            .collect();
        CompiledProgram {
            schema_version: PROGRAM_SCHEMA_VERSION,
            task_signature: "summarize-report".into(),
            steps: BoundedVec::new(steps).unwrap(),
            parameters: BoundedVec::new(vec![
                ProgramParameter {
                    name: "title".into(),
                    kind: ProgramParameterKind::Text,
                },
                ProgramParameter {
                    name: "limit".into(),
                    kind: ProgramParameterKind::Int,
                },
                ProgramParameter {
                    name: "dry_run".into(),
                    kind: ProgramParameterKind::Bool,
                },
            ])
            .unwrap(),
            source_run_id: "run-1".into(),
            program_digest: String::new(),
        }
        .sealed()
    }

    fn trace(
        run_id: &str,
        outcome: RunOutcome,
        literals: [&str; 2],
        steps: usize,
    ) -> RunTraceRecord {
        use crate::decision::run_governance::{StepKind, StepOutcome, TraceArgument, TraceStep};
        let make = |index: usize| {
            let mut arguments = vec![TraceArgument {
                name: "title".into(),
                value: TraceScalar::Text(literals[0].into()),
            }];
            if index == 0 {
                arguments.push(TraceArgument {
                    name: "limit".into(),
                    value: TraceScalar::Int(10),
                });
                arguments.push(TraceArgument {
                    name: "dry_run".into(),
                    value: TraceScalar::Bool(true),
                });
                arguments.push(TraceArgument {
                    name: "label".into(),
                    value: TraceScalar::Text(literals[1].into()),
                });
            }
            TraceStep {
                index: index as u32,
                kind: StepKind::ToolCall,
                action_ref: format!("tool:t{index}"),
                outcome: StepOutcome::Ok,
                arguments: BoundedVec::new(arguments).unwrap(),
            }
        };
        RunTraceRecord {
            schema_version: 1,
            run_id: run_id.into(),
            task_signature: "summarize-report".into(),
            model_id: "model".into(),
            steps: BoundedVec::new((0..steps).map(make).collect()).unwrap(),
            outcome,
            started_at_ms: 1,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.2
    #[test]
    fn literals_become_parameters() {
        let knobs = ProgramKnobs {
            max_steps: MAX_PROGRAM_STEPS,
        };
        let first = compile_program(
            &trace("run-1", RunOutcome::Succeeded, ["Q3", "x"], 2),
            &knobs,
        )
        .unwrap();
        let second = compile_program(
            &trace("run-1", RunOutcome::Succeeded, ["Q4", "y"], 2),
            &knobs,
        )
        .unwrap();
        // Two traces that differ only in literals compile to one program.
        assert_eq!(first, second);
        // Parameters are named by first occurrence; equal literals share a name.
        let names: Vec<&str> = first.parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["p0", "p1", "p2", "p3"]);
        let kinds: Vec<ProgramParameterKind> = first.parameters.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                ProgramParameterKind::Text,
                ProgramParameterKind::Int,
                ProgramParameterKind::Bool,
                ProgramParameterKind::Text
            ]
        );
        assert_eq!(
            first.steps.as_slice()[0].action,
            "tool:t0(title=p0,limit=p1,dry_run=p2,label=p3)"
        );
        // The second step repeats the title literal and reuses p0.
        assert_eq!(first.steps.as_slice()[1].action, "tool:t1(title=p0)");
        assert_eq!(first.steps.as_slice()[0].state, "s0");
        assert_eq!(first.steps.as_slice()[0].next.as_deref(), Some("s1"));
        assert_eq!(first.steps.as_slice()[1].next, None);
        first.validate().unwrap();
        // Different run ids give different digests.
        let other = compile_program(
            &trace("run-2", RunOutcome::Succeeded, ["Q3", "x"], 2),
            &knobs,
        )
        .unwrap();
        assert_ne!(other.program_digest, first.program_digest);
        // Equal text and int literals stay distinct parameters (kind matters).
        let mut mixed = trace("run-1", RunOutcome::Succeeded, ["10", "x"], 1);
        let compiled = compile_program(&mixed, &knobs).unwrap();
        assert_eq!(compiled.parameters.len(), 4);
        mixed.steps = BoundedVec::new(Vec::new()).unwrap();
        assert!(matches!(
            compile_program(&mixed, &knobs),
            Err(ProgramError::Invalid(_))
        ));

        // The knob lowers the cap; the hard cap stays 40.
        let capped = ProgramKnobs { max_steps: 1 };
        let refused = compile_program(
            &trace("run-1", RunOutcome::Succeeded, ["a", "b"], 2),
            &capped,
        )
        .unwrap_err();
        assert_eq!(refused, ProgramError::TooLong { steps: 2 });
        let wide = ProgramKnobs { max_steps: 1000 };
        let refused = compile_program(
            &trace("run-1", RunOutcome::Succeeded, ["a", "b"], 41),
            &wide,
        )
        .unwrap_err();
        assert_eq!(refused.code(), Some(PROGRAM_TOO_LONG));
        // A failed run is never compiled.
        let failed = compile_program(&trace("run-1", RunOutcome::Failed, ["a", "b"], 1), &knobs)
            .unwrap_err();
        assert_eq!(failed.code(), Some(RUN_NOT_SUCCESSFUL));
        assert!(failed.to_string().starts_with("RUN_NOT_SUCCESSFUL"));

        // Lookup returns the stored program for its signature, else none.
        let stored = vec![first.clone()];
        assert_eq!(lookup_program(&stored, "summarize-report"), Some(&first));
        assert_eq!(lookup_program(&stored, "other"), None);
        // The knob resolves from a policy and defaults to the hard cap.
        assert_eq!(
            ProgramKnobs::from_policy(&DecisionPolicy::engine_default()).max_steps,
            MAX_PROGRAM_STEPS
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.1
    #[test]
    fn program_round_trips() {
        let original = program(3);
        original.validate().unwrap();
        assert_eq!(original.program_digest.len(), 64);

        // Every field survives JSON and MessagePack.
        let wire = serde_json::to_value(&original).unwrap();
        let decoded = CompiledProgram::from_value(wire.clone()).unwrap();
        assert_eq!(decoded, original);
        decoded.validate().unwrap();
        let packed = rmp_serde::to_vec_named(&original).unwrap();
        let unpacked: CompiledProgram = rmp_serde::from_slice(&packed).unwrap();
        assert_eq!(unpacked, original);
        assert_eq!(wire["parameters"][0]["kind"], "text");
        assert_eq!(
            serde_json::to_string(&crate::decision::statistical::QuestionKind::ProgramLookup)
                .unwrap(),
            "\"program_lookup\""
        );

        // A tampered field breaks the digest.
        let mut tampered = original.clone();
        tampered.source_run_id = "run-2".into();
        assert!(matches!(tampered.validate(), Err(ProgramError::Invalid(_))));
        // An unknown field is refused.
        let mut unknown = wire.clone();
        unknown["extra"] = serde_json::json!(1);
        assert!(matches!(
            CompiledProgram::from_value(unknown),
            Err(ProgramError::Malformed(_))
        ));
        // A dangling `next` is refused.
        let mut dangling = original.clone();
        dangling.steps = BoundedVec::new(vec![step("s0", Some("missing"))]).unwrap();
        assert!(dangling.sealed().validate().is_err());

        // 40 steps are accepted; 41 are PROGRAM_TOO_LONG at decode.
        let forty = serde_json::to_value(program(MAX_PROGRAM_STEPS)).unwrap();
        CompiledProgram::from_value(forty)
            .unwrap()
            .validate()
            .unwrap();
        let mut too_long = serde_json::to_value(program(MAX_PROGRAM_STEPS)).unwrap();
        let extra = serde_json::to_value(step("s40", None)).unwrap();
        too_long["steps"].as_array_mut().unwrap().push(extra);
        let refused = CompiledProgram::from_value(too_long).unwrap_err();
        assert_eq!(refused, ProgramError::TooLong { steps: 41 });
        assert_eq!(refused.code(), Some(PROGRAM_TOO_LONG));
        assert!(refused.to_string().starts_with("PROGRAM_TOO_LONG"));
    }
}
