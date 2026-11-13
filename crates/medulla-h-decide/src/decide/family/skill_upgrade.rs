//! The `skill_upgrade` question family (MEDULLA-DECISION-ENGINE-R167.3).
//!
//! Whether a new consumer contract still serves the previous one, for a skill,
//! tool or prompt. The request model is
//! `medulla_decision_types::decision::skill_upgrade_request`; the additive-only
//! rule is `compare_contracts` in `medulla_decision_outcome_types`. The single
//! candidate is `admit_upgrade`. The family recomputes both contract digests,
//! compares the contracts and concludes a constraint. `Decide` is evaluate-only:
//! it writes nothing, and the caller commits through `DecisionCommit`. Every
//! check runs before any store call, so a refusal writes nothing.

use medulla_decision_outcome_types::decision::skill_upgrade::compare_contracts;
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::skill_upgrade_request::{
    SkillUpgradeRequest, SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION, SKILL_UPGRADE_DIGEST_MISMATCH,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_support::invalid_parameter;

use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R167.4), declared
/// beside the model so the uniqueness test of the code registry collects it.
pub use medulla_decision_types::decision::skill_upgrade_request::REFUSALS;

/// The param that carries the typed upgrade request.
pub(crate) const PARAM_SKILL_UPGRADE: &str = "skill_upgrade";
/// The single candidate of an upgrade question.
const ADMIT_UPGRADE: &str = "admit_upgrade";

/// The family of the `skill_upgrade` question kind.
pub struct SkillUpgradeFamily;

/// The typed upgrade request of a `skill_upgrade` question.
pub(crate) fn upgrade_of(request: &DecideRequest) -> Result<&SkillUpgradeRequest, String> {
    let upgrade = request
        .params
        .iter()
        .find_map(
            |param| match (&param.value, param.name == PARAM_SKILL_UPGRADE) {
                (TypedValue::SkillUpgrade(upgrade), true) => Some(upgrade.as_ref()),
                _ => None,
            },
        )
        .ok_or_else(|| {
            invalid_parameter(format!(
                "a skill_upgrade question requires a {PARAM_SKILL_UPGRADE} param"
            ))
        })?;
    upgrade.validate().map_err(invalid_parameter)?;
    Ok(upgrade)
}

fn executed(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        family: None,
    }
}

/// A refusal of a well-formed upgrade: an abstention with the registered code
/// and no receipt (MEDULLA-DECISION-ENGINE-R167.4).
fn abstained(code: RefusalCode, detail: String) -> Executed {
    executed(StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: QuestionKind::SkillUpgrade,
            code,
            detail,
        }])
        .expect("one reason fits the bound"),
    })
}

/// Decide one upgrade question: digests first, then the additive-only rule.
fn admission(upgrade: &SkillUpgradeRequest) -> Result<Executed, String> {
    let previous = upgrade.previous.digest().map_err(invalid_parameter)?;
    let new = upgrade.new.digest().map_err(invalid_parameter)?;
    for (side, claimed, recomputed) in [
        ("previous", upgrade.claimed_previous_digest, previous),
        ("new", upgrade.claimed_new_digest, new),
    ] {
        if claimed.is_some_and(|claimed| claimed != recomputed) {
            return Ok(abstained(
                SKILL_UPGRADE_DIGEST_MISMATCH,
                format!("the claimed {side} contract digest differs from the recomputed one"),
            ));
        }
    }
    let diff = compare_contracts(&upgrade.previous, &upgrade.new);
    if !diff.is_compatible() && upgrade.declared_migration_id.is_none() {
        return Ok(abstained(
            SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION,
            format!(
                "breaking upgrade without a declared migration: added_required={:?} removed={:?} type_changed={:?} became_required={:?}",
                diff.added_required, diff.removed, diff.type_changed, diff.became_required
            ),
        ));
    }
    Ok(executed(StatisticalOutcome::Advisory {
        scores: BoundedVec::new(vec![ScoredOption {
            option_id: ADMIT_UPGRADE.to_string(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 1i64 << 32,
            },
            probability: None,
        }])
        .expect("one score fits the bound"),
        calibrated: false,
    }))
}

impl DecisionFamily for SkillUpgradeFamily {
    /// The single candidate is `admit_upgrade`.
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        match options {
            [only] if only.option_id == ADMIT_UPGRADE => Ok(()),
            _ => Err(invalid_parameter(format!(
                "a skill_upgrade question declares exactly one option, {ADMIT_UPGRADE}"
            ))),
        }
    }

    /// The request carries one well-formed upgrade; checked before any store read.
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        upgrade_of(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(upgrade_of(request).and_then(admission))
    }

    /// The record's evidence (MEDULLA-DECISION-ENGINE-R167.5): the comparison
    /// of the two contracts with both digests and the diff class, and the
    /// declared migration or `none`. Both are `Claim` premises, because the
    /// caller supplies both schemas.
    fn premises(
        &self,
        request: &DecideRequest,
        _graph_view: Option<&GraphView>,
    ) -> Vec<PremiseRef> {
        let Ok(upgrade) = upgrade_of(request) else {
            return Vec::new();
        };
        let (Ok(previous), Ok(new)) = (upgrade.previous.digest(), upgrade.new.digest()) else {
            return Vec::new();
        };
        let class = if previous == new {
            "identical"
        } else if compare_contracts(&upgrade.previous, &upgrade.new).is_compatible() {
            "additive"
        } else {
            "breaking"
        };
        let claim = |fact: String| PremiseRef {
            subject: upgrade.subject_id.as_str().to_string(),
            fact,
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: PARAM_SKILL_UPGRADE.to_string(),
            },
        };
        let migration = upgrade
            .declared_migration_id
            .as_ref()
            .map_or("none", |id| id.as_str());
        vec![
            claim(format!(
                "upgrade:contract_comparison previous={} new={} diff={class}",
                previous.to_hex(),
                new.to_hex()
            )),
            claim(format!("upgrade:migration={migration}")),
        ]
    }

    /// An admitted upgrade is a constraint; an abstention keeps its own kind.
    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => None,
            _ => Some(ResolutionKind::Constraint),
        }
    }
}
