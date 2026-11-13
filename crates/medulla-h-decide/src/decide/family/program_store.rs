//! The `program_lookup` question family (MEDULLA-DECISION-ENGINE-R135.1,
//! R135.3, R135.4, R135.5).
//!
//! Which stored compiled program answers one task signature. The model is
//! `medulla_decision_types::decision::program_store`. The family reads the key
//! `DecisionLog` wrote (`run_record::program_key`) from the caller's tenant
//! and writes nothing. A signature with no stored program abstains with
//! `NO_PROGRAM`.

use medulla_decision_types::decision::program_store::preplan::{pattern_key, ReusablePattern};
use medulla_decision_types::decision::program_store::{CompiledProgram, NoProgram};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::decision_jobs::decode_artifact;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, single_option, text_param};
use super::DecisionFamily;

/// The one option a program lookup question declares.
pub const PROGRAM_OPTION: &str = "program";
/// The request parameter naming the task signature.
pub const PARAM_TASK_SIGNATURE: &str = "task_signature";
/// The prefix of the answer option that carries the stored program as JSON.
pub const ANSWER_PREFIX: &str = "program:";

/// The family of the `program_lookup` question kind.
pub struct ProgramLookupFamily;

fn signature(request: &DecideRequest) -> Result<&str, String> {
    match text_param(request, PARAM_TASK_SIGNATURE)? {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(invalid(
            "a program_lookup question requires a non-empty task_signature",
        )),
    }
}

/// The stored program for `task_signature` in the caller's tenant, if any.
fn stored(ctx: &ExecutionContext, task_signature: &str) -> Result<Option<CompiledProgram>, String> {
    let key = medulla_decision_types::decision::program_store::program_key(task_signature);
    let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? else {
        return Ok(None);
    };
    let program: CompiledProgram = decode_artifact(&bytes, "compiled program")?;
    program
        .validate()
        .map_err(|error| invalid(format!("the stored program is unreadable: {error}")))?;
    Ok(Some(program))
}

/// The pattern promoted for `task_signature` in the caller's tenant, if any
/// (MEDULLA-DECISION-ENGINE-R135.7). It is indexed here, beside the program.
fn promoted(
    ctx: &ExecutionContext,
    task_signature: &str,
) -> Result<Option<ReusablePattern>, String> {
    let Some(bytes) = ctx
        .store
        .decision_artifact(ctx.tenant_id, &pattern_key(task_signature))?
    else {
        return Ok(None);
    };
    decode_artifact(&bytes, "reusable pattern").map(Some)
}

fn claim(fact: String, field: &'static str) -> FamilyPremise {
    FamilyPremise {
        subject: "program_lookup",
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(field),
    }
}

impl DecisionFamily for ProgramLookupFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, PROGRAM_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        signature(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let task_signature = signature(request)?;
            let mut executed = match stored(ctx, task_signature)? {
                None => abstained(
                    QuestionKind::ProgramLookup,
                    &NoProgram {
                        task_signature: task_signature.to_string(),
                    },
                ),
                Some(program) => {
                    let body = serde_json::to_string(&program)
                        .map_err(|error| invalid(format!("program encoding failed: {error}")))?;
                    let mut executed = advisory(vec![format!("{ANSWER_PREFIX}{body}")])?;
                    executed.family_premises = vec![
                        claim(
                            format!("program.step_count={}", program.steps.len()),
                            PARAM_TASK_SIGNATURE,
                        ),
                        claim(
                            format!("program.parameter_count={}", program.parameters.len()),
                            PARAM_TASK_SIGNATURE,
                        ),
                        claim(
                            format!("program.source_run_id={}", program.source_run_id),
                            PARAM_TASK_SIGNATURE,
                        ),
                    ];
                    if let Some(pattern) = promoted(ctx, task_signature)? {
                        for fact in [
                            "program.reusable=true".to_string(),
                            format!(
                                "program.preplan_command_digest={}",
                                pattern.receipt.command_digest
                            ),
                            format!(
                                "program.preplan_result_digest={}",
                                pattern.receipt.result_digest
                            ),
                            format!(
                                "program.preplan_manifest_digest={}",
                                pattern.receipt.artifact_manifest_digest
                            ),
                        ] {
                            executed
                                .family_premises
                                .push(claim(fact, PARAM_TASK_SIGNATURE));
                        }
                    }
                    executed
                }
            };
            executed.family_premises.insert(
                0,
                claim(
                    format!("program.task_signature={task_signature}"),
                    PARAM_TASK_SIGNATURE,
                ),
            );
            Ok(executed)
        })())
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

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}
