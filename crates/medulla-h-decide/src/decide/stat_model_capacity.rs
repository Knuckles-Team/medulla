//! The `model_capacity` question family (MEDULLA-DECISION-ENGINE-R825).
//!
//! Feeds the caller's recorded observations of one model endpoint to the
//! admission controllers of `medulla-resource` (`ModelCapacityController`,
//! `CircuitBreaker`, `CognitiveScheduler`) and answers the limits they leave. The
//! handler copies none of their logic and reads no ledger or log. A ceiling
//! above the declared cell ceiling abstains `PolicyLoosening`. Evaluate-only.

use medulla_decision_types::decision::model_capacity::{
    breaker_cooldown_ms, breaker_failures, latency_target_ms, BreakerKind, CapacityAnswer,
    CapacityBounds, CapacityObservation, CapacityOutcome, CAPACITY_OPTION, MAX_OBSERVATIONS,
    PARAM_BOUNDS, PARAM_OBSERVATIONS,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseRef,
};
use medulla_graph_core::graph::GraphView;
use medulla_resource::admission::{
    AgentProcess, CircuitBreaker, CircuitState, CognitiveDecision, CognitiveScheduler,
    InferenceBudget, ModelCapacityController, Outcome, PriorityClass,
    CONCURRENCY_ABOVE_CELL_CEILING,
};
use medulla_types::contract::BoundedVec;
use sha2::{Digest, Sha256};

use super::family::{advisory, invalid, json_param, single_option, text_param};
use super::family::DecisionFamily;
use super::stat_executor::{Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource};

/// The family of the `model_capacity` question kind.
pub struct ModelCapacityFamily;

/// The observations, at most [`MAX_OBSERVATIONS`] with `at_ms` non-decreasing.
fn observations(request: &DecideRequest) -> Result<Vec<CapacityObservation>, String> {
    let rows: Vec<CapacityObservation> = json_param(request, PARAM_OBSERVATIONS)?;
    if rows.len() > MAX_OBSERVATIONS {
        return Err(invalid(format!(
            "{PARAM_OBSERVATIONS} holds more than {MAX_OBSERVATIONS} observations"
        )));
    }
    if rows.windows(2).any(|pair| pair[1].at_ms < pair[0].at_ms) {
        return Err(invalid(format!("{PARAM_OBSERVATIONS} at_ms must not decrease")));
    }
    Ok(rows)
}

fn kind_of(state: CircuitState) -> BreakerKind {
    match state {
        CircuitState::Closed { .. } => BreakerKind::Closed,
        CircuitState::HalfOpen => BreakerKind::HalfOpen,
        CircuitState::Open { .. } => BreakerKind::Open,
    }
}

fn decide_capacity(request: &DecideRequest, policy: &DecisionPolicy) -> Result<Executed, String> {
    let rows = observations(request)?;
    let bounds: CapacityBounds = json_param(request, PARAM_BOUNDS)?;
    if rows.last().is_some_and(|last| bounds.now_ms < last.at_ms) {
        return Err(invalid(format!(
            "{PARAM_BOUNDS} now_ms is before the last observation"
        )));
    }
    let mut controller = match ModelCapacityController::new(
        bounds.initial,
        bounds.min,
        bounds.ceiling,
        bounds.cell_ceiling,
        latency_target_ms(policy),
    ) {
        Ok(controller) => controller,
        Err(error) if error.code == CONCURRENCY_ABOVE_CELL_CEILING => {
            let mut executed = advisory(Vec::new())?;
            executed.ladder.outcome = StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![AbstainReason::PolicyLoosening {
                    field: "ceiling".to_string(),
                }])
                .expect("one reason is inside the bound"),
            };
            return Ok(executed);
        }
        Err(error) => return Err(invalid(error)),
    };
    let mut breaker = CircuitBreaker::new(breaker_failures(policy), breaker_cooldown_ms(policy));
    for row in &rows {
        controller.on_latency(row.latency_ms);
        breaker.tick(row.at_ms);
        let outcome = match row.outcome {
            CapacityOutcome::Success => Outcome::Success,
            CapacityOutcome::Failure => Outcome::Failure,
        };
        breaker
            .observe(Some(outcome), row.at_ms)
            .map_err(invalid)?;
    }
    breaker.tick(bounds.now_ms);
    // The group's slots are the scheduler's budget: each grant is one
    // one-token process of one class, so the scheduler admits until the slots
    // are used and refuses the rest.
    let mut scheduler = CognitiveScheduler::new(InferenceBudget {
        tokens_per_window: u64::from(bounds.group_slots),
    });
    let mut granted: u32 = 0;
    for slot in 0..controller.effective_limit(&breaker) {
        let process = AgentProcess {
            id: format!("{}:{slot}", bounds.group),
            class: PriorityClass::Normal,
            used_tokens: 0,
        };
        match scheduler.request(process, 1) {
            CognitiveDecision::Admit => granted += 1,
            CognitiveDecision::Preempt { .. } | CognitiveDecision::Refuse => break,
        }
    }
    let answer = CapacityAnswer {
        limit: controller.limit(),
        effective_limit: controller.effective_limit(&breaker),
        breaker: kind_of(breaker.state),
        granted,
        group_slots_free: bounds.group_slots.saturating_sub(granted),
    };
    let text = text_param(request, PARAM_OBSERVATIONS)?.unwrap_or_default();
    let mut executed = advisory(vec![format!(
        "{CAPACITY_OPTION}:{}",
        serde_json::to_string(&answer).map_err(invalid)?
    )])?;
    executed.family_premises = vec![FamilyPremise {
        subject: "model_capacity".into(),
        fact: format!(
            "capacity:observations={}@{}",
            rows.len(),
            hex::encode(Sha256::digest(text.as_bytes()))
        ),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.capacity_observations"),
    }];
    Ok(executed)
}

impl DecisionFamily for ModelCapacityFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, CAPACITY_OPTION)
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(decide_capacity(request, policy))
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
