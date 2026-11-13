//! The `field_shielding` question family (MEDULLA-DECISION-ENGINE-R162.3,
//! R162.4).
//!
//! A step may read only the fields it declared, and a shielded field is
//! withheld even when declared. The caller sends the object's paths, the
//! shielded paths, the step's declared inputs and the reads it wants. The family
//! runs `release_projection` over the request alone and concludes
//! `ResolutionKind::Constraint`: one `release_projection` option, with the
//! allowed and withheld sets as premises. A requested read that is shielded or
//! undeclared, a required input the object lacks and a path deeper than the
//! `shielding.max_depth` knob each abstain with a registered code naming the
//! path. `Decide` reads no store and writes nothing.

use medulla_decision_types::decision::field_shielding::{
    max_depth, release_projection, FieldShieldingRequest,
};
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, param, single_option};
use super::DecisionFamily;

/// The one declared option: release the computed projection.
pub const RELEASE_OPTION: &str = "release_projection";

/// The family of the `field_shielding` question kind.
pub struct FieldShieldingFamily;

/// The shielding request the question binds.
fn shielding(request: &DecideRequest) -> Result<&FieldShieldingRequest, String> {
    match param(request, "field_shielding")? {
        Some(TypedValue::FieldShielding(shielding)) => Ok(shielding),
        Some(_) => Err(invalid(
            "parameter field_shielding is not a shielding request",
        )),
        None => Err(invalid("parameter field_shielding is required")),
    }
}

fn claim(subject: &'static str, fact: String) -> FamilyPremise {
    FamilyPremise {
        subject,
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.field_shielding"),
    }
}

fn conclude_projection(
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let shielding = shielding(request)?;
    let decision = match release_projection(shielding, max_depth(policy)) {
        Ok(decision) => decision,
        Err(refusal) => return Ok(abstained(QuestionKind::FieldShielding, &refusal)),
    };
    let mut executed = advisory(vec![RELEASE_OPTION.to_string()])?;
    let mut premises = vec![claim(
        "shielding_step",
        format!("shielding:step={}", shielding.step.step_id),
    )];
    premises.extend(decision.allowed.iter().map(|path| {
        claim(
            "shielding_allowed",
            format!("shielding:allowed={}", path.as_str()),
        )
    }));
    premises.extend(decision.withheld.iter().map(|path| {
        claim(
            "shielding_withheld",
            format!("shielding:withheld={}", path.as_str()),
        )
    }));
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for FieldShieldingFamily {
    fn validate_candidates(
        &self,
        options: &[medulla_decision_types::decision::statistical::declared::DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, RELEASE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        shielding(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_projection(request, policy))
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

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
