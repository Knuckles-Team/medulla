//! The `skill_workflow_compile` question family (MEDULLA-DECISION-ENGINE-R156.1,
//! R156.3, R156.4, R156.5).
//!
//! Compiles a skill document's typed draft into a program. The draft arrives as the
//! typed parameter `skill_workflow` and the one declared option is `workflow`. The family
//! checks the draft is complete (`SkillWorkflowDraft::check_complete`), compiles it with
//! `compile_skill_workflow` and answers `FamilyAnswer::SkillWorkflow`. `Decide` writes
//! nothing: committing the record through `DecisionLog` stores the program under
//! `skill:<document digest>`, where `ProgramLookup` reads it. An incomplete draft refuses by
//! name and compiles no partial program.

use medulla_decision_plan::plan::skill_workflow::{compile_skill_workflow, COMPILER_ID, COMPILER_REVISION};
use medulla_decision_types::decision::skill_workflow::SkillWorkflowDraft;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyExecuted, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, param, single_option};
use super::DecisionFamily;

/// The one option a skill workflow question declares.
pub const WORKFLOW_OPTION: &str = "workflow";
/// The request parameter that carries the draft.
pub const PARAM_SKILL_WORKFLOW: &str = "skill_workflow";

/// The family of the `skill_workflow_compile` question kind.
pub struct SkillWorkflowFamily;

fn draft_of(request: &DecideRequest) -> Result<&SkillWorkflowDraft, String> {
    match param(request, PARAM_SKILL_WORKFLOW)? {
        Some(TypedValue::SkillWorkflowDraft(draft)) => Ok(draft),
        _ => Err(invalid(format!(
            "a skill_workflow_compile question carries a typed {PARAM_SKILL_WORKFLOW} draft"
        ))),
    }
}

fn premise(fact: String, class: PremiseClass) -> FamilyPremise {
    FamilyPremise {
        subject: "skill_workflow".into(),
        fact,
        class,
        source: FamilyPremiseSource::Request(PARAM_SKILL_WORKFLOW),
    }
}

fn compile(request: &DecideRequest) -> Result<Executed, String> {
    let draft = draft_of(request)?;
    // Every check runs before the compile; a refusal names the step and writes nothing.
    draft.check_complete().map_err(invalid)?;
    let program = compile_skill_workflow(draft).map_err(invalid)?;
    let premises = vec![
        // The harness produced the draft; the program is a pure function of it.
        premise(
            format!("skill_workflow:document={}", draft.skill_document_digest.to_hex()),
            PremiseClass::Claim,
        ),
        premise(
            format!("skill_workflow:program={}", program.program_digest),
            PremiseClass::Proof,
        ),
        premise(
            format!("skill_workflow:steps={}", program.steps.len()),
            PremiseClass::Claim,
        ),
        premise(
            format!("skill_workflow:compiler={COMPILER_ID}@{COMPILER_REVISION}"),
            PremiseClass::Definition,
        ),
    ];
    let mut executed = advisory(vec![WORKFLOW_OPTION.to_string()])?;
    executed.family = Some(FamilyExecuted {
        answer: Some(FamilyAnswer::SkillWorkflow(Box::new(program))),
        premises: Vec::new(),
        resolution: Some(ResolutionKind::Constraint),
        evidence: Some(EvidenceClass::Claim),
    });
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for SkillWorkflowFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, WORKFLOW_OPTION)
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(compile(request))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }
}
