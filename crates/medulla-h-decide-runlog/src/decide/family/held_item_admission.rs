//! The `held_item_admission` question family (MEDULLA-DECISION-ENGINE-R038.3).
//!
//! A run holds compiled programs and output proposals until they are admitted.
//! The item is a node of the graph the question names, opened through the
//! caller's access check and row-level filter. The stored digest must equal the
//! digest the caller expects. A program is admitted only when it is active,
//! verified by someone other than its compiler, and the verification earned at
//! least `held_item.min_passes`; a proposal is admitted or rejected by the
//! critique verdict bound to its digest. The verdict is advisory and `Decide`
//! writes nothing.

use medulla_decision_statistical_types::decision::record::outcome::refusal::DecisionRefusal;
use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::held_item::{
    critique_checklist, is_digest, min_passes, HeldItemAnswer, HeldItemKind, HeldItemOutcome,
    HeldItemRefusal, HeldItemVerdict, PARAM_CONTENT_DIGEST, PARAM_GRAPH, PARAM_ITEM_KIND,
    PARAM_ITEM_REF, ROW_COMPILED_BY, ROW_CONTENT_DIGEST, ROW_CRITIQUE_DIGEST, ROW_CRITIQUE_REF,
    ROW_CRITIQUE_VERDICT, ROW_ITEM_KIND, ROW_LIFECYCLE, ROW_VERIFICATION_PASSES,
    ROW_VERIFICATION_REF, ROW_VERIFIED_BY,
};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};

use medulla_h_decide_base::decide::stat_support::filtered_view;
use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyExecuted,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::abstained,
    medulla_h_decide_base::decide::family::support::advisory,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::single_option,
    medulla_h_decide_base::decide::family::support::text_param,
};

/// The one option a held-item admission question declares.
pub const ADMISSION_OPTION: &str = "admission";

/// The family of the `held_item_admission` question kind.
pub struct HeldItemAdmissionFamily;

/// The text parameter `name`, required and non-empty.
fn required<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("a held_item_admission question requires {name}")))
}

/// Open the graph holding the held item before any read, through the caller's
/// row-level filter.
pub async fn held_item_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    filtered_view(state, agent_id, required(request, PARAM_GRAPH)?).await
}

impl DecisionFamily for HeldItemAdmissionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ADMISSION_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        required(request, PARAM_GRAPH)?;
        required(request, PARAM_ITEM_REF)?;
        let kind = required(request, PARAM_ITEM_KIND)?;
        HeldItemKind::parse(kind).ok_or_else(|| {
            invalid(format!(
                "{PARAM_ITEM_KIND} `{kind}` is not run_output_proposal or stored_run_program"
            ))
        })?;
        let digest = required(request, PARAM_CONTENT_DIGEST)?;
        if !is_digest(digest) {
            return Err(invalid(format!(
                "{PARAM_CONTENT_DIGEST} is not 64 lowercase hex characters"
            )));
        }
        Ok(())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(admit(request, policy, graph_view))
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

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}

fn refused(refusal: HeldItemRefusal, kind: HeldItemKind) -> Result<Executed, String> {
    let mut executed = abstained(QuestionKind::HeldItemAdmission, &refusal);
    executed.family = Some(answer_of(HeldItemAnswer {
        outcome: HeldItemOutcome::Abstained,
        verdict_ref: String::new(),
        verification_ref: None,
        verifier: None,
        compiler: None,
        reason: Some(refusal.code().as_str().to_string()),
        checklist: checklist_of(kind),
        critique_digest: None,
        critique_verdict: None,
    }));
    Ok(executed)
}

/// The questions a critique of an item of `kind` answers: the checklist for a proposal, even
/// when the question is refused for want of a critique, and none for a program.
fn checklist_of(
    kind: HeldItemKind,
) -> Vec<medulla_decision_types::decision::held_item::CritiqueQuestion> {
    match kind {
        HeldItemKind::RunOutputProposal => critique_checklist(),
        HeldItemKind::StoredRunProgram => Vec::new(),
    }
}

/// The typed answer a question seals; the verdict ref is the sealing record's
/// id, filled in when the record is sealed.
fn answer_of(answer: HeldItemAnswer) -> FamilyExecuted {
    FamilyExecuted {
        answer: Some(FamilyAnswer::HeldItemAdmission(Box::new(answer))),
        premises: Vec::new(),
        resolution: None,
        evidence: None,
    }
}

fn premise(fact: String, item_ref: &str) -> FamilyPremise {
    FamilyPremise {
        subject: "held_item".into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(item_ref.to_string()),
    }
}

fn admit(
    request: &DecideRequest,
    policy: &DecisionPolicy,
    view: Option<&GraphView>,
) -> Result<Executed, String> {
    let item_ref = required(request, PARAM_ITEM_REF)?;
    let kind = HeldItemKind::parse(required(request, PARAM_ITEM_KIND)?)
        .ok_or_else(|| invalid("a held_item_admission question names a known item_kind"))?;
    let expected = required(request, PARAM_CONTENT_DIGEST)?;
    let view = view.ok_or_else(|| invalid("a held_item_admission question reads a graph"))?;
    let row = view.node_row_object(item_ref);
    let text = |key: &str| {
        row.as_ref()
            .and_then(|props| props.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    if text(ROW_ITEM_KIND).as_deref() != Some(kind.name()) {
        return refused(
            HeldItemRefusal::ItemUnknown {
                item_ref: item_ref.to_string(),
            },
            kind,
        );
    }
    let stored = text(ROW_CONTENT_DIGEST).unwrap_or_default();
    if stored != expected {
        return refused(
            HeldItemRefusal::DigestMismatch {
                stored,
                expected: expected.to_string(),
            },
            kind,
        );
    }
    match kind {
        HeldItemKind::StoredRunProgram => {
            if text(ROW_LIFECYCLE).as_deref() == Some("retired") {
                return refused(
                    HeldItemRefusal::Retired {
                        item_ref: item_ref.to_string(),
                    },
                    kind,
                );
            }
            let verification = text(ROW_VERIFICATION_REF);
            let (verifier, compiler) = (text(ROW_VERIFIED_BY), text(ROW_COMPILED_BY));
            let independent = match (&verifier, &compiler) {
                (Some(verifier), Some(compiler)) => verifier != compiler,
                _ => false,
            };
            let Some(verification) = verification.filter(|_| independent) else {
                return refused(
                    HeldItemRefusal::VerifierNotIndependent {
                        item_ref: item_ref.to_string(),
                    },
                    kind,
                );
            };
            let passes = row
                .as_ref()
                .and_then(|props| props.get(ROW_VERIFICATION_PASSES))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let required = min_passes(policy);
            if passes < required {
                return refused(
                    HeldItemRefusal::PassesInsufficient { passes, required },
                    kind,
                );
            }
            let mut executed = advisory(vec![HeldItemVerdict::Admitted.option_id().to_string()])?;
            executed.family_premises =
                vec![premise(format!("verification:{verification}"), item_ref)];
            executed.family = Some(answer_of(HeldItemAnswer {
                outcome: HeldItemOutcome::Admitted,
                verdict_ref: String::new(),
                verification_ref: Some(verification),
                verifier,
                compiler,
                reason: None,
                checklist: Vec::new(),
                critique_digest: None,
                critique_verdict: None,
            }));
            Ok(executed)
        }
        HeldItemKind::RunOutputProposal => {
            let bound = text(ROW_CRITIQUE_DIGEST).as_deref() == Some(expected);
            let critique = text(ROW_CRITIQUE_REF).filter(|_| bound);
            let verdict = match text(ROW_CRITIQUE_VERDICT).as_deref() {
                Some("accept") => HeldItemVerdict::Admitted,
                Some("reject") => HeldItemVerdict::Rejected,
                _ => {
                    return refused(
                        HeldItemRefusal::CritiqueMissing {
                            item_ref: item_ref.to_string(),
                        },
                        kind,
                    )
                }
            };
            let Some(critique) = critique else {
                return refused(
                    HeldItemRefusal::CritiqueMissing {
                        item_ref: item_ref.to_string(),
                    },
                    kind,
                );
            };
            let mut executed = advisory(vec![verdict.option_id().to_string()])?;
            executed.family_premises = vec![premise(format!("critique:{critique}"), item_ref)];
            executed.family = Some(answer_of(HeldItemAnswer {
                outcome: match verdict {
                    HeldItemVerdict::Admitted => HeldItemOutcome::Admitted,
                    HeldItemVerdict::Rejected => HeldItemOutcome::Rejected,
                },
                verdict_ref: String::new(),
                verification_ref: Some(critique),
                verifier: None,
                compiler: None,
                reason: None,
                checklist: critique_checklist(),
                critique_digest: text(ROW_CRITIQUE_DIGEST),
                critique_verdict: text(ROW_CRITIQUE_VERDICT),
            }));
            Ok(executed)
        }
    }
}
