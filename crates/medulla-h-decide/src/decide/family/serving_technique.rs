//! The `serving_technique_activation` question family (MEDULLA-DECISION-ENGINE-R819).
//!
//! The question names a technique, a model and a configuration digest and
//! carries the benchmark receipts as JSON. The verdict is `activate` only when
//! every receipt matches and the candidate metric stays inside
//! `serving.max_regression` and gains `serving.min_gain` (see
//! `decision::serving_technique`). A missing, mismatched or regressed receipt
//! set is a coded refusal. Advisory; `Decide` writes nothing.

use medulla_decision_types::decision::serving_technique::{
    max_regression_ppm, min_gain_ppm, verdict, ServingReceipt, Subject, ACTIVATION_OPTION,
    PARAM_CONFIG_DIGEST, PARAM_MODEL, PARAM_RECEIPTS, PARAM_TECHNIQUE,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, json_param, single_option, text_param};
use super::DecisionFamily;

/// The family of the `serving_technique_activation` question kind.
pub struct ServingTechniqueFamily;

fn required<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("a serving technique question requires {name}")))
}

fn activate(request: &DecideRequest, policy: &DecisionPolicy) -> Result<Executed, String> {
    let subject = Subject {
        technique: required(request, PARAM_TECHNIQUE)?,
        model: required(request, PARAM_MODEL)?,
        config_digest: required(request, PARAM_CONFIG_DIGEST)?,
    };
    let receipts: Vec<ServingReceipt> = json_param(request, PARAM_RECEIPTS)?;
    let answer = verdict(
        subject,
        &receipts,
        max_regression_ppm(policy),
        min_gain_ppm(policy),
    )
    .map_err(|refusal| format!("{}: {}", refusal.code.as_str(), refusal.detail))?;
    let mut executed = advisory(vec![answer.to_string()])?;
    executed.family_premises = vec![FamilyPremise {
        subject: "serving_technique",
        fact: format!("receipts:{}", receipts.len()),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.receipts"),
    }];
    Ok(executed)
}

impl DecisionFamily for ServingTechniqueFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ACTIVATION_OPTION)
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(activate(request, policy))
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
