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

use medulla_decision_types::decision::held_item::{
    is_digest, min_passes, HeldItemKind, HeldItemRefusal, HeldItemVerdict, PARAM_CONTENT_DIGEST,
    PARAM_GRAPH, PARAM_ITEM_KIND, PARAM_ITEM_REF, ROW_COMPILED_BY, ROW_CONTENT_DIGEST,
    ROW_CRITIQUE_DIGEST, ROW_CRITIQUE_REF, ROW_CRITIQUE_VERDICT, ROW_ITEM_KIND, ROW_LIFECYCLE,
    ROW_VERIFICATION_PASSES, ROW_VERIFICATION_REF, ROW_VERIFIED_BY,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::filtered_view;

use super::support::{abstained, advisory, invalid, single_option, text_param};
use super::DecisionFamily;

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
pub(in crate::decide) async fn held_item_view(
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

fn refused(refusal: HeldItemRefusal) -> Result<Executed, String> {
    Ok(abstained(QuestionKind::HeldItemAdmission, &refusal))
}

fn premise(fact: String, item_ref: &str) -> FamilyPremise {
    FamilyPremise {
        subject: "held_item",
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
        return refused(HeldItemRefusal::ItemUnknown {
            item_ref: item_ref.to_string(),
        });
    }
    let stored = text(ROW_CONTENT_DIGEST).unwrap_or_default();
    if stored != expected {
        return refused(HeldItemRefusal::DigestMismatch {
            stored,
            expected: expected.to_string(),
        });
    }
    match kind {
        HeldItemKind::StoredRunProgram => {
            if text(ROW_LIFECYCLE).as_deref() == Some("retired") {
                return refused(HeldItemRefusal::Retired {
                    item_ref: item_ref.to_string(),
                });
            }
            let verification = text(ROW_VERIFICATION_REF);
            let independent = match (text(ROW_VERIFIED_BY), text(ROW_COMPILED_BY)) {
                (Some(verifier), Some(compiler)) => verifier != compiler,
                _ => false,
            };
            let Some(verification) = verification.filter(|_| independent) else {
                return refused(HeldItemRefusal::VerifierNotIndependent {
                    item_ref: item_ref.to_string(),
                });
            };
            let passes = row
                .as_ref()
                .and_then(|props| props.get(ROW_VERIFICATION_PASSES))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let required = min_passes(policy);
            if passes < required {
                return refused(HeldItemRefusal::PassesInsufficient { passes, required });
            }
            let mut executed = advisory(vec![HeldItemVerdict::Admitted.option_id().to_string()])?;
            executed.family_premises =
                vec![premise(format!("verification:{verification}"), item_ref)];
            Ok(executed)
        }
        HeldItemKind::RunOutputProposal => {
            let bound = text(ROW_CRITIQUE_DIGEST).as_deref() == Some(expected);
            let critique = text(ROW_CRITIQUE_REF).filter(|_| bound);
            let verdict = match text(ROW_CRITIQUE_VERDICT).as_deref() {
                Some("accept") => HeldItemVerdict::Admitted,
                Some("reject") => HeldItemVerdict::Rejected,
                _ => {
                    return refused(HeldItemRefusal::CritiqueMissing {
                        item_ref: item_ref.to_string(),
                    })
                }
            };
            let Some(critique) = critique else {
                return refused(HeldItemRefusal::CritiqueMissing {
                    item_ref: item_ref.to_string(),
                });
            };
            let mut executed = advisory(vec![verdict.option_id().to_string()])?;
            executed.family_premises = vec![premise(format!("critique:{critique}"), item_ref)];
            Ok(executed)
        }
    }
}
