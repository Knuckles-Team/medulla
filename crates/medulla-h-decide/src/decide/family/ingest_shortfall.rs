//! The `ingest_shortfall` question family (MEDULLA-DECISION-ENGINE-R826).
//!
//! The throughput window of the ingest measurement, the declared goals and the
//! remaining budget arrive as typed parameters; the answer names the funded
//! goals (`goal:<id>`) and the eligible purge candidates (`purge:<id>`), see
//! `decision::ingest_shortfall`. Advisory; `Decide` writes nothing.

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_graph_core::graph::GraphView;
use medulla_types::ingest_throughput::ThroughputWindow;
use {
    medulla_decision_statistical_types::decision::ingest_shortfall::allocate,
    medulla_decision_statistical_types::decision::ingest_shortfall::purges,
    medulla_decision_statistical_types::decision::ingest_shortfall::window_fact,
    medulla_decision_statistical_types::decision::ingest_shortfall::Goal,
    medulla_decision_statistical_types::decision::ingest_shortfall::PurgeCandidate,
    medulla_decision_statistical_types::decision::ingest_shortfall::ALLOCATION_OPTION,
    medulla_decision_statistical_types::decision::ingest_shortfall::GOAL_PREFIX,
    medulla_decision_statistical_types::decision::ingest_shortfall::PARAM_BUDGET,
    medulla_decision_statistical_types::decision::ingest_shortfall::PARAM_GOALS,
    medulla_decision_statistical_types::decision::ingest_shortfall::PARAM_PURGE_CANDIDATES,
    medulla_decision_statistical_types::decision::ingest_shortfall::PARAM_WINDOW,
    medulla_decision_statistical_types::decision::ingest_shortfall::PURGE_PREFIX,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, json_param, param, single_option, unsigned_param};
use super::DecisionFamily;

/// The family of the `ingest_shortfall` question kind.
pub struct IngestShortfallFamily;

fn shortfall(request: &DecideRequest) -> Result<Executed, String> {
    let window: ThroughputWindow = json_param(request, PARAM_WINDOW)?;
    let goals: Vec<Goal> = json_param(request, PARAM_GOALS)?;
    let budget = unsigned_param(request, PARAM_BUDGET)?.ok_or_else(|| {
        invalid(format!(
            "an ingest_shortfall question requires {PARAM_BUDGET}"
        ))
    })?;
    let candidates: Vec<PurgeCandidate> = if param(request, PARAM_PURGE_CANDIDATES)?.is_some() {
        json_param(request, PARAM_PURGE_CANDIDATES)?
    } else {
        Vec::new()
    };
    let mut ids: Vec<String> = allocate(&goals, budget)
        .into_iter()
        .map(|goal| format!("{GOAL_PREFIX}{}", goal.goal_id))
        .collect();
    ids.extend(
        purges(&candidates)
            .into_iter()
            .map(|candidate| format!("{PURGE_PREFIX}{}", candidate.item_id)),
    );
    let mut executed = advisory(ids)?;
    executed.family_premises = vec![FamilyPremise {
        subject: "ingest_shortfall".into(),
        fact: window_fact(&window),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.window"),
    }];
    Ok(executed)
}

impl DecisionFamily for IngestShortfallFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ALLOCATION_OPTION)
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(shortfall(request))
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
