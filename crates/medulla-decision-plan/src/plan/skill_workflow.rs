//! Skill workflow compilation (MEDULLA-DECISION-ENGINE-R156.2).
//!
//! [`compile_skill_workflow`] is a pure function: it validates a typed
//! [`SkillWorkflowDraft`] and maps each step to a program step in the declared
//! order. The same draft yields the same program bytes. The skill document
//! remains the one source of truth; the program holds only its digest.

use medulla_decision_types::decision::program_store::{
    CompiledProgram, ProgramEdge, ProgramError, ProgramStep, PROGRAM_SCHEMA_VERSION,
};
use medulla_decision_types::decision::skill_workflow::SkillWorkflowDraft;
use medulla_types::contract::BoundedVec;

/// The compiler identity a sealed record names next to the program digest.
pub const COMPILER_ID: &str = "skill_workflow_compile";
/// The revision of the compiler; it changes when a draft would compile to
/// different bytes.
pub const COMPILER_REVISION: u32 = 1;
/// The label of the edge a step's `on_error` handler becomes.
pub const ERROR_EDGE_LABEL: &str = "error";

/// Why a draft did not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillCompileError {
    /// The draft breaks a structural rule; the text names the step.
    Invalid(String),
    /// The compiled program broke a program invariant.
    Program(ProgramError),
}

impl std::fmt::Display for SkillCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(detail) => write!(f, "skill workflow draft is invalid: {detail}"),
            Self::Program(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SkillCompileError {}

/// Compile `draft` into a program.
///
/// Step `state` is the draft's `step_id` and the order of steps is kept. A
/// step's declared `next` edges become its edges in order, and `next` is the
/// first declared edge's target. `on_error` becomes one further edge labelled
/// `error`. The program's `task_signature` is `skill:<skill_document_digest>`
/// and it has no `source_run_id`.
pub fn compile_skill_workflow(
    draft: &SkillWorkflowDraft,
) -> Result<CompiledProgram, SkillCompileError> {
    draft.validate().map_err(SkillCompileError::Invalid)?;
    let steps: Vec<ProgramStep> = draft
        .steps
        .iter()
        .map(|step| {
            let mut edges: Vec<ProgramEdge> = step
                .next
                .iter()
                .map(|edge| ProgramEdge {
                    label: edge.label.clone(),
                    to: edge.step_id.clone(),
                })
                .collect();
            let next = edges.first().map(|edge| edge.to.clone());
            if let Some(handler) = &step.on_error {
                edges.push(ProgramEdge {
                    label: Some(ERROR_EDGE_LABEL.to_string()),
                    to: handler.clone(),
                });
            }
            ProgramStep {
                state: step.step_id.clone(),
                action: step.action.clone(),
                next,
                agent_config: None,
                edges,
                input_types: step.input_types.iter().copied().collect(),
                output_types: step.output_types.iter().copied().collect(),
            }
        })
        .collect();
    let program = CompiledProgram {
        schema_version: PROGRAM_SCHEMA_VERSION,
        task_signature: format!("skill:{}", draft.skill_document_digest.to_hex()),
        steps: BoundedVec::new(steps)
            .map_err(|detail| SkillCompileError::Program(ProgramError::Invalid(detail)))?,
        parameters: BoundedVec::new(Vec::new())
            .map_err(|detail| SkillCompileError::Program(ProgramError::Invalid(detail)))?,
        source_run_id: None,
        program_digest: String::new(),
    }
    .sealed();
    program.validate().map_err(SkillCompileError::Program)?;
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::skill_workflow::{DraftStep, StepEdge, TypeRef};
    use medulla_types::contract::Digest256;

    fn step(id: &str, next: &[(&str, Option<&str>)], on_error: Option<&str>) -> DraftStep {
        DraftStep {
            step_id: id.into(),
            action: format!("tool.{id}"),
            input_types: BoundedVec::new(vec![TypeRef::Text]).unwrap(),
            output_types: BoundedVec::new(vec![TypeRef::Json]).unwrap(),
            next: BoundedVec::new(
                next.iter()
                    .map(|(to, label)| StepEdge {
                        step_id: (*to).into(),
                        label: label.map(str::to_string),
                    })
                    .collect(),
            )
            .unwrap(),
            on_error: on_error.map(str::to_string),
        }
    }

    fn golden() -> SkillWorkflowDraft {
        SkillWorkflowDraft {
            skill_document_digest: Digest256::sha256(b"golden skill"),
            steps: BoundedVec::new(vec![
                step(
                    "fetch",
                    &[("check", None), ("publish", Some("skip"))],
                    Some("recover"),
                ),
                step("check", &[("publish", Some("ok"))], None),
                step("publish", &[], None),
                step("recover", &[], None),
            ])
            .unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R156.2
    #[test]
    fn golden_draft_compiles_in_order() {
        let draft = golden();
        let program = compile_skill_workflow(&draft).unwrap();
        let states: Vec<&str> = program.steps.iter().map(|s| s.state.as_str()).collect();
        assert_eq!(states, ["fetch", "check", "publish", "recover"]);
        assert_eq!(
            program.task_signature,
            format!("skill:{}", draft.skill_document_digest.to_hex())
        );
        assert_eq!(program.source_run_id, None);
        let fetch = &program.steps[0];
        assert_eq!(fetch.next.as_deref(), Some("check"));
        let edges: Vec<(Option<&str>, &str)> = fetch
            .edges
            .iter()
            .map(|e| (e.label.as_deref(), e.to.as_str()))
            .collect();
        assert_eq!(
            edges,
            [
                (None, "check"),
                (Some("skip"), "publish"),
                (Some("error"), "recover")
            ]
        );
        assert_eq!(program.steps[2].next, None);
        assert!(program.steps[2].edges.is_empty());
        assert_eq!(fetch.input_types, [TypeRef::Text]);
        assert_eq!(fetch.output_types, [TypeRef::Json]);
        program.validate().unwrap();
    }

    // spec: MEDULLA-DECISION-ENGINE-R156.2
    #[test]
    fn compile_is_deterministic() {
        let first = compile_skill_workflow(&golden()).unwrap();
        let second = compile_skill_workflow(&golden()).unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(first.program_digest, second.program_digest);
    }
}
