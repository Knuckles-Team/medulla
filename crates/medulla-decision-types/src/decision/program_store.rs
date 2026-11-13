//! Compiled programs keyed by task signature: the model
//! (MEDULLA-DECISION-ENGINE-R135.1).
//!
//! A successful run compiles into a stored state-machine program: each step
//! names a state, an action and the state it moves to, and every literal of the
//! run became a named parameter. `Decide` with
//! [`QuestionKind::ProgramLookup`](super::statistical::QuestionKind::ProgramLookup)
//! returns the stored program for a task signature and writes nothing. Nothing
//! here reads a clock or a store.

use medulla_agent_types::agent_component::ComponentDependency;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Most steps one compiled program may hold. A `Fixed` safety bound.
pub const MAX_PROGRAM_STEPS: usize = 40;
/// Most parameters one compiled program may hold.
pub const MAX_PROGRAM_PARAMETERS: usize = 64;
/// Current wire schema version of [`CompiledProgram`].
pub const PROGRAM_SCHEMA_VERSION: u32 = 1;

/// Refusal: a program of more than [`MAX_PROGRAM_STEPS`] steps.
pub const PROGRAM_TOO_LONG: RefusalCode = RefusalCode::new("PROGRAM_TOO_LONG");

/// Every refusal code of the program-lookup family; joins
/// `codes::FAMILY_TABLES` under `QuestionKind::ProgramLookup`.
pub const REFUSALS: &[RefusalCode] = &[PROGRAM_TOO_LONG];

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
    /// More than [`MAX_PROGRAM_STEPS`] steps (`PROGRAM_TOO_LONG`).
    TooLong { steps: usize },
    /// The wire value did not decode.
    Malformed(String),
    /// A field broke an invariant.
    Invalid(String),
}

impl ProgramError {
    /// The registered refusal code, when the error is one.
    pub fn code(&self) -> Option<RefusalCode> {
        matches!(self, Self::TooLong { .. }).then_some(PROGRAM_TOO_LONG)
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
