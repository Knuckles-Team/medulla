//! The trajectory decisions of a `task_plan` Decide (MEDULLA-DECISION-ENGINE-R140.104).
//!
//! The four decisions are pure kernels in `medulla-decision-types`. This module
//! is where their caller-supplied facts come from: the knobs of the resolved
//! decision policy, the engine clock, and the tenant-visible outcome records
//! read through the [`LogReader`]. A kernel refusal leaves as `CODE: detail`,
//! the code being one the TaskPlan family registers.

use std::collections::BTreeMap;

use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::task_plan::branch_breadth::{
    decide_breadth, BranchHistory, BreadthKnobs, ResultClass, StoredBranchOutcome,
};
use medulla_decision_types::decision::task_plan::pairwise::{rank_pairwise, PairwiseKnobs};
use medulla_decision_types::decision::task_plan::trajectory::{
    TrajectoryAnswer, TrajectoryDecision, TrajectoryQuery, TRAJECTORY_PARAM,
};
use medulla_decision_types::decision::task_plan::trajectory_budget::{
    decide_budget, TrajectoryKnobs,
};
use medulla_decision_types::decision::task_plan::trajectory_projection::{
    validate_projection, StoredSource,
};
use medulla_decision_types::decision::task_plan::trajectory_refusal::TrajectoryRefusal;
use medulla_decision_types::decision::PremiseRef;

use super::stat_executor::ExecutionContext;
use super::stat_log::{evaluations_of, visible_entry, LogReader};
use super::stat_support::ResolvedPolicy;

/// The trajectory query a request carries, when it carries one.
pub(super) fn query_of(request: &DecideRequest) -> Option<&TrajectoryQuery> {
    request.params.iter().find_map(|param| match &param.value {
        TypedValue::Trajectory(query) if param.name == TRAJECTORY_PARAM => Some(&**query),
        _ => None,
    })
}

fn coded(refused: TrajectoryRefusal) -> String {
    format!("{}: {}", refused.code.as_str(), refused.detail)
}

/// One cited outcome record as the reader sees it: the committed record of the
/// cited id, visible to the reader, with at least one stored outcome
/// evaluation. Its revision counts the stored evaluations, its digest is the
/// committed record digest, and its class is the evaluations' verdict. A record
/// the reader cannot see (another tenant's, another principal's) is absent.
struct StoredOutcome {
    tenant_id: String,
    revision: u64,
    digest: String,
    class: ResultClass,
}

fn stored_outcome(
    ctx: &ExecutionContext,
    reader: &LogReader,
    id: &str,
) -> Result<Option<StoredOutcome>, String> {
    let Some(entry) = visible_entry(ctx.store, reader, id)? else {
        return Ok(None);
    };
    let evaluations = evaluations_of(ctx.store, &reader.tenant_id, id)?;
    if evaluations.is_empty() {
        return Ok(None);
    }
    let passed = evaluations
        .iter()
        .filter(|e| e.evaluation.success == Some(true))
        .count();
    let failed = evaluations
        .iter()
        .filter(|e| e.evaluation.success == Some(false))
        .count();
    let class = match (passed, failed) {
        (p, 0) if p > 0 => ResultClass::Success,
        (0, f) if f > 0 => ResultClass::Failure,
        _ => ResultClass::Partial,
    };
    Ok(Some(StoredOutcome {
        tenant_id: entry.record.tenant_id.clone(),
        revision: evaluations.len() as u64,
        digest: entry.record.record_digest.clone(),
        class,
    }))
}

fn stored_outcomes(
    ctx: &ExecutionContext,
    reader: &LogReader,
    ids: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, StoredOutcome>, String> {
    let mut out = BTreeMap::new();
    for id in ids {
        if let Some(stored) = stored_outcome(ctx, reader, &id)? {
            out.insert(id, stored);
        }
    }
    Ok(out)
}

/// Take the decision `query` asks for, with the premises it rests on.
pub(super) fn decide(
    ctx: &ExecutionContext,
    reader: &LogReader,
    policy: &ResolvedPolicy,
    query: &TrajectoryQuery,
) -> Result<(TrajectoryDecision, Vec<PremiseRef>), String> {
    let (answer, premises) = match query {
        TrajectoryQuery::Budget(request) => {
            let knobs = TrajectoryKnobs::from_policy(&policy.policy);
            let (budget, premises) = decide_budget(request, &knobs, ctx.now_ms).map_err(coded)?;
            (TrajectoryAnswer::Budget(budget), premises)
        }
        TrajectoryQuery::Projection(request) => {
            let stored: BTreeMap<String, StoredSource> = stored_outcomes(
                ctx,
                reader,
                request.sources.iter().map(|s| s.run_id.clone()),
            )?
            .into_iter()
            .map(|(id, s)| {
                (
                    id,
                    StoredSource {
                        source_revision: s.revision,
                        trace_digest: s.digest,
                    },
                )
            })
            .collect();
            let (plan, premises) = validate_projection(request, &stored).map_err(coded)?;
            (TrajectoryAnswer::Projection(plan), premises)
        }
        TrajectoryQuery::Breadth(request) => {
            let ids: Vec<String> = match &request.history {
                BranchHistory::Absent => Vec::new(),
                BranchHistory::Present { refs, .. } => {
                    refs.iter().map(|r| r.branch_id.clone()).collect()
                }
            };
            let stored: BTreeMap<String, StoredBranchOutcome> =
                stored_outcomes(ctx, reader, ids.into_iter())?
                    .into_iter()
                    .map(|(id, s)| {
                        (
                            id,
                            StoredBranchOutcome {
                                tenant_id: s.tenant_id,
                                outcome_revision: s.revision,
                                outcome_digest: s.digest,
                                result_class: s.class,
                            },
                        )
                    })
                    .collect();
            let knobs = BreadthKnobs::from_policy(&policy.policy);
            let (decision, premises) =
                decide_breadth(request, &reader.tenant_id, &stored, &knobs).map_err(coded)?;
            (TrajectoryAnswer::Breadth(decision), premises)
        }
        TrajectoryQuery::Pairwise(request) => {
            let knobs = PairwiseKnobs::from_policy(&policy.policy);
            let (ranking, premises) = rank_pairwise(request, &knobs).map_err(coded)?;
            (TrajectoryAnswer::Pairwise(ranking), premises)
        }
    };
    let decision = TrajectoryDecision {
        decision_digest: answer.decision_digest().to_string(),
        answer,
        policy_digest: policy.digest.clone(),
        source_digests: query.source_digests(),
    };
    Ok((decision, premises))
}
