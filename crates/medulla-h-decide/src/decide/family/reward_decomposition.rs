//! The `reward_decomposition` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R751.755).
//!
//! Splits one trajectory reward over its steps and task hierarchy with the pure
//! `decompose` kernel. The value estimates are caller claims, so the record is
//! a `Claim` resolved as a `Constraint`. Nothing is written: the caller records
//! the rewards it accepts through `DecisionLog`.

use medulla_decision_stats::decision::reward::decompose;
use medulla_decision_types::decision::reward::{RewardDecompositionRequest, RewardKnobs};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{advisory, body, claim, one_option, shape};
use super::DecisionFamily;

const KIND: &str = "reward_decomposition";
const PARAM: &str = "reward_decomposition";

fn parse(request: &DecideRequest) -> Result<&RewardDecompositionRequest, String> {
    let options = shape(request, KIND)?;
    let TypedValue::RewardDecomposition(trajectory) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    if one_option(options, KIND)? != trajectory.trajectory_id {
        return Err(invalid_parameter(format!(
            "the declared candidate must be the trajectory id {}",
            trajectory.trajectory_id
        )));
    }
    Ok(&**trajectory)
}

/// The deepest chain of the hierarchy: a root is depth 1.
fn hierarchy_depth(trajectory: &RewardDecompositionRequest) -> usize {
    let mut depth: Vec<usize> = Vec::with_capacity(trajectory.hierarchy.len());
    for node in trajectory.hierarchy.iter() {
        let parent = node
            .parent
            .and_then(|p| depth.get(p as usize).copied())
            .unwrap_or(0);
        depth.push(parent + 1);
    }
    depth.into_iter().max().unwrap_or(0)
}

/// The family of the `reward_decomposition` question kind.
pub struct RewardDecompositionFamily;

impl DecisionFamily for RewardDecompositionFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|trajectory| {
            let knobs = RewardKnobs::from_policy(policy);
            let reward = decompose(trajectory, &knobs)?;
            let field = "params.reward_decomposition";
            let id = trajectory.trajectory_id.as_str();
            let premises: Vec<PremiseRef> = vec![
                claim(
                    id,
                    format!(
                        "reward:trajectory={id}/{}",
                        trajectory.trajectory_reward.value
                    ),
                    field,
                ),
                claim(
                    id,
                    format!("reward:steps={}", trajectory.steps.len()),
                    field,
                ),
                claim(
                    id,
                    format!("reward:hierarchy_depth={}", hierarchy_depth(trajectory)),
                    field,
                ),
                claim(
                    id,
                    format!("reward:keep={}", reward.keep_for_training),
                    field,
                ),
            ];
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::RewardDecomposition(Box::new(reward))),
                    premises,
                    resolution: None,
                    evidence: None,
                }),
            ))
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
