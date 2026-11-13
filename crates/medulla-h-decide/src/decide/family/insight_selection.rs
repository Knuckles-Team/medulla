//! The `insight_selection` question family (MEDULLA-DECISION-ENGINE-R135.10,
//! R135.12, R135.13, R135.14).
//!
//! Which stored run-derived insights apply to one context signature. The model
//! is `medulla_decision_types::decision::program_store::insight`. The family
//! reads every row `DecisionLog` stored under `run_record/insight/` in the
//! caller's tenant, selects with `select_insights` under the policy's
//! `insight.*` knobs and writes nothing. A context signature that is empty, or
//! matches no stored insight, abstains with `UNKNOWN_CONTEXT_SIGNATURE`.

use medulla_decision_stats::decision::insight::select_insights;
use medulla_decision_types::decision::program_store::insight::{
    Insight, InsightKnobs, UnknownContextSignature, INSIGHT_KEY_PREFIX,
};
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

/// The one option an insight selection question declares.
pub const INSIGHT_OPTION: &str = "insight";
/// The request parameter naming the context signature.
pub const PARAM_CONTEXT_SIGNATURE: &str = "context_signature";
/// The prefix of an answer option that names one selected insight.
pub const ANSWER_PREFIX: &str = "insight:";
/// Most stored insights one selection reads; a tenant past it refuses rather than truncates.
pub const MAX_STORED_INSIGHTS: usize = 10_000;

/// The family of the `insight_selection` question kind.
pub struct InsightSelectionFamily;

fn signature(request: &DecideRequest) -> Result<&str, String> {
    Ok(text_param(request, PARAM_CONTEXT_SIGNATURE)?.unwrap_or(""))
}

/// Every insight stored for the caller's tenant.
fn stored(ctx: &ExecutionContext) -> Result<Vec<Insight>, String> {
    ctx.store
        .decision_artifacts_with_prefix(ctx.tenant_id, INSIGHT_KEY_PREFIX, MAX_STORED_INSIGHTS)?
        .into_iter()
        .map(|(_, bytes)| {
            let insight: Insight = decode_artifact(&bytes, "insight")?;
            insight
                .validate()
                .map_err(|error| invalid(format!("the stored insight is unreadable: {error}")))?;
            Ok(insight)
        })
        .collect()
}

fn observation(fact: String) -> FamilyPremise {
    FamilyPremise {
        subject: "insight_selection",
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Request(PARAM_CONTEXT_SIGNATURE),
    }
}

impl DecisionFamily for InsightSelectionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, INSIGHT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        signature(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let context_signature = signature(request)?;
            let unknown = || {
                abstained(
                    QuestionKind::InsightSelection,
                    &UnknownContextSignature {
                        context_signature: context_signature.to_string(),
                    },
                )
            };
            if context_signature.trim().is_empty() {
                return Ok(unknown());
            }
            let knobs = InsightKnobs::from_policy(policy);
            let selected = select_insights(&stored(ctx)?, context_signature, &knobs);
            if selected.is_empty() {
                return Ok(unknown());
            }
            let mut executed = advisory(
                selected
                    .iter()
                    .map(|chosen| format!("{ANSWER_PREFIX}{}", chosen.insight.insight_id))
                    .collect(),
            )?;
            executed.family_premises = selected
                .iter()
                .map(|chosen| {
                    observation(format!(
                        "insight:id={};sources={};match_q32={};selection_revision={}",
                        chosen.insight.insight_id,
                        chosen
                            .insight
                            .source_run_ids
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(","),
                        chosen.match_q32,
                        chosen.insight.selection_revision
                    ))
                })
                .collect();
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
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}
