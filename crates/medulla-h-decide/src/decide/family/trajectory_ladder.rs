//! The `trajectory_ladder` question family (MEDULLA-DECISION-ENGINE-R134.12,
//! R134.12.1).
//!
//! Chooses one rung of the caller's width and depth ladder. The legal set comes
//! from the request's `max_width` and the `trajectory.max_depth` knob; a
//! promoted head ranks only that set. A claim of improvement is answered only
//! when a stored paired verdict proves it. Evaluate-only: `Decide` writes
//! nothing; verdicts are written through `DecisionLog`.

use std::collections::BTreeSet;

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_stats::decision::trajectory_ladder::{choose_rung, improvement_proven};
use medulla_decision_types::decision::knob_families::trajectory;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::head::DecisionHeadBody;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalErrorCode, StatisticalOutcome,
};
use medulla_decision_types::decision::trajectory_ladder::{
    PairedVerdict, TrajectoryLadderRequest, MAX_RUNGS, TRAJECTORY_IMPROVEMENT_UNPROVEN,
    VERDICT_PREFIX,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::decision_jobs::decode_artifact;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::{pinned_body, refusal};

use super::support::{advisory, invalid, json_param, single_option};
use super::DecisionFamily;

/// The one option a trajectory-ladder question declares.
pub const LADDER_OPTION: &str = "ladder";
/// The prefix of the answer option that carries the rung choice as JSON.
pub const ANSWER_PREFIX: &str = "choice:";
/// The request parameter carrying the ladder.
pub const PARAM_LADDER: &str = "ladder";
/// The most verdicts one decision reads.
const MAX_VERDICTS: usize = 4096;

/// The family of the `trajectory_ladder` question kind.
pub struct TrajectoryLadderFamily;

fn ladder(request: &DecideRequest) -> Result<TrajectoryLadderRequest, String> {
    let ladder: TrajectoryLadderRequest = json_param(request, PARAM_LADDER)?;
    if ladder.task_digest.trim().is_empty() || ladder.input_snapshot_digest.trim().is_empty() {
        return Err(invalid(
            "a trajectory_ladder question names its task and input snapshot digests",
        ));
    }
    if ladder.rungs.len() > MAX_RUNGS {
        return Err(invalid(format!("a ladder holds at most {MAX_RUNGS} rungs")));
    }
    let ids: BTreeSet<&String> = ladder.rungs.iter().map(|rung| &rung.rung_id).collect();
    if ids.len() != ladder.rungs.len() || ids.iter().any(|id| id.trim().is_empty()) {
        return Err(invalid("rung ids are non-empty and unique"));
    }
    Ok(ladder)
}

fn claim(fact: String, class: PremiseClass) -> FamilyPremise {
    FamilyPremise {
        subject: "trajectory_ladder".into(),
        fact,
        class,
        source: FamilyPremiseSource::Request(PARAM_LADDER),
    }
}

impl DecisionFamily for TrajectoryLadderFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, LADDER_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        ladder(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let asked = ladder(request)?;
            // The head is read from the caller's own tenant only: another tenant's
            // component is absent here.
            let head = request
                .head
                .as_ref()
                .map(|pin| {
                    let (body, digest) = pinned_body::<DecisionHeadBody>(
                        ctx.store,
                        ctx.tenant_id,
                        pin,
                        AgentComponentKind::DecisionHead,
                        StatisticalErrorCode::HeadInvalid,
                    )?;
                    let body = body
                        .checked()
                        .map_err(|detail| refusal(StatisticalErrorCode::HeadInvalid, detail))?;
                    Ok::<_, String>((body, digest))
                })
                .transpose()?;
            if let Some(rung) = &asked.improvement_claim {
                let mut verdicts = Vec::new();
                for (_, bytes) in ctx.store.decision_artifacts_with_prefix(
                    ctx.tenant_id,
                    VERDICT_PREFIX,
                    MAX_VERDICTS,
                )? {
                    verdicts.push(decode_artifact::<PairedVerdict>(&bytes, "paired verdict")?);
                }
                if !improvement_proven(&verdicts, rung, &asked.input_snapshot_digest) {
                    return Err(format!(
                        "{TRAJECTORY_IMPROVEMENT_UNPROVEN}: no stored verdict has `{rung}` beating the baseline on this snapshot"
                    ));
                }
            }
            let max_depth = u32::try_from(trajectory::read(policy, &trajectory::MAX_DEPTH))
                .map_err(|_| invalid("trajectory.max_depth is out of range"))?;
            let default_rung = usize::try_from(trajectory::read(policy, &trajectory::DEFAULT_RUNG))
                .map_err(|_| invalid("trajectory.default_rung is out of range"))?;
            let choice = choose_rung(
                &asked,
                max_depth,
                default_rung,
                head.as_ref().map(|(body, _)| body),
            )
            .map_err(|refused| {
                refused
                    .render()
                    .unwrap_or_else(|| match refused {
                        medulla_decision_stats::decision::trajectory_ladder::LadderRefusal::HeadShape(detail) => {
                            refusal(StatisticalErrorCode::HeadInvalid, detail)
                        }
                        _ => invalid("the ladder cannot choose a rung"),
                    })
            })?;
            let body = serde_json::to_string(&choice)
                .map_err(|error| invalid(format!("choice encoding failed: {error}")))?;
            let mut executed = advisory(vec![format!("{ANSWER_PREFIX}{body}")])?;
            executed.family_premises.push(claim(
                format!(
                    "trajectory_rung:id={};width={};depth={}",
                    choice.rung_id, choice.width, choice.depth
                ),
                PremiseClass::Claim,
            ));
            if let Some((_, digest)) = &head {
                executed.family_premises.push(claim(
                    format!("trajectory_head={digest}"),
                    PremiseClass::Claim,
                ));
            }
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
