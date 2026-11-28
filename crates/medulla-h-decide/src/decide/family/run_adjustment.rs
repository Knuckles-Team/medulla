//! The `run_adjustment` question family (MEDULLA-DECISION-ENGINE-R158.3,
//! MEDULLA-ORCH-VOCAB-R009.18.3).
//!
//! The declared options are the legal models, each carrying a whole-number
//! `tier` encoded as Q32. The caller names the run (`run_id`) and what runs now
//! (`current_model`, `current_width`, `host_width_ceiling`, `budget_tokens`, and
//! `mid_run` when a run is in flight). The serving path reads the run's
//! `RunObservationWindow` nodes from the graph and hands them in the execution
//! context; the family never reads the graph. It keeps the newest
//! `width.window` windows no older than `width.window_max_age_s`
//! (`select_windows`), counts each window as one run, takes the p95 of the
//! per-window latency and the mean per-window cost, and calls
//! `decide_adjustment`. The answer is an advisory naming `model:<id>`,
//! `width:<n>` and `budget_tokens:<n>`; `Decide` writes nothing, and the sealed
//! `DecisionRecord` is the adjustment record, whose premises name the window it
//! read. Fewer runs than `width.min_runs` abstains `INSUFFICIENT_SAMPLES`; a
//! run whose every window is stale abstains `WINDOW_STALE` and a newest window
//! with an unknown error rate abstains `RATE_UNKNOWN`; every other refusal is a
//! `PARAMETER_INVALID` request refusal naming its code.

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
use medulla_work_types::run_observation::{Measured, RunObservationWindow};
use {
    medulla_decision_policy_types::decision::run_adjustment::decide_adjustment,
    medulla_decision_policy_types::decision::run_adjustment::error_rate_q32,
    medulla_decision_policy_types::decision::run_adjustment::latency_p95,
    medulla_decision_policy_types::decision::run_adjustment::AdjustmentRefusal,
    medulla_decision_policy_types::decision::run_adjustment::Current,
    medulla_decision_policy_types::decision::run_adjustment::ErrorWindow,
    medulla_decision_policy_types::decision::run_adjustment::ModelTier,
    medulla_decision_policy_types::decision::run_adjustment::Observed,
    medulla_decision_policy_types::decision::run_adjustment::WidthKnobs,
    medulla_decision_policy_types::decision::run_adjustment::INSUFFICIENT_SAMPLES,
    medulla_decision_policy_types::decision::run_adjustment::LATENCY_UNKNOWN,
    medulla_decision_policy_types::decision::run_adjustment::RATE_UNKNOWN,
    medulla_decision_policy_types::decision::run_adjustment::WINDOW_STALE,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::AbstainReason,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::CandidateSource,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::TypedValue,
};

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, int_param, invalid, param, text_param};
use super::DecisionFamily;
use crate::decide::windows_read::select_windows;

/// The family of the `run_adjustment` question kind.
pub struct RunAdjustmentFamily;

/// The `tier` number of a declared model, as a whole number.
const TIER_KEY: &str = "tier";
/// The optional relative run cost of a declared model, as a whole number.
const COST_WEIGHT_KEY: &str = "cost_weight";

fn iri_param<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    match request.params.iter().find(|p| p.name == name) {
        Some(param) => match &param.value {
            TypedValue::Iri(iri) => Ok(iri.as_str()),
            _ => Err(invalid(format!("parameter {name} is not an IRI"))),
        },
        None => Err(invalid(format!("parameter {name} is required"))),
    }
}

fn count_param(request: &DecideRequest, name: &str) -> Result<u64, String> {
    let value = int_param(request, name)?
        .ok_or_else(|| invalid(format!("parameter {name} is required")))?;
    u64::try_from(value).map_err(|_| invalid(format!("parameter {name} must not be negative")))
}

fn tiers(options: &[DeclaredOption]) -> Result<Vec<ModelTier>, String> {
    options
        .iter()
        .map(|option| {
            let number = option
                .numbers
                .iter()
                .find(|n| n.key == TIER_KEY)
                .ok_or_else(|| invalid(format!("model {} declares no tier", option.option_id)))?;
            Ok(ModelTier {
                model: option.option_id.clone(),
                tier: number.q32 >> 32,
                cost_weight: option
                    .numbers
                    .iter()
                    .find(|n| n.key == COST_WEIGHT_KEY)
                    .map_or(1, |n| u64::try_from(n.q32 >> 32).unwrap_or(1).max(1)),
            })
        })
        .collect()
}

/// The 95th percentile of one window's latency samples; `None` when the
/// samples are unknown or empty.
fn window_latency_ms(window: &RunObservationWindow) -> Option<u64> {
    match &window.latency_samples_ms {
        Measured::Measured(samples) => latency_p95(samples.clone()),
        Measured::Unknown(_) => None,
    }
}

/// A window is one run; it is errored when its `error_count` is measured and
/// above zero.
fn errored(window: &RunObservationWindow) -> bool {
    matches!(window.error_count, Measured::Measured(count) if count > 0)
}

/// The latency and cost the windows observed. The cost is the mean, in
/// micro-units of the major unit (ISO 4217 minor-unit exponent of each
/// window's currency), over the windows whose cost is measured in the newest
/// measured window's currency; no such window leaves it unknown.
fn observed(windows: &[RunObservationWindow]) -> Observed {
    let currency = windows.iter().find_map(|window| match &window.cost {
        Measured::Measured(cost) => Some(cost.currency.as_str()),
        Measured::Unknown(_) => None,
    });
    let costs: Vec<u64> = windows
        .iter()
        .filter_map(|window| match &window.cost {
            Measured::Measured(cost) if Some(cost.currency.as_str()) == currency => {
                cost.micro_units()
            }
            _ => None,
        })
        .collect();
    let predicted_cost_micro = (!costs.is_empty()).then(|| {
        let total: u128 = costs.iter().map(|cost| u128::from(*cost)).sum();
        u64::try_from(total / costs.len() as u128).unwrap_or(u64::MAX)
    });
    Observed {
        latency_p95_ms: latency_p95(windows.iter().filter_map(window_latency_ms).collect()),
        predicted_cost_micro,
    }
}

/// An abstention of this family with no advisory lines.
fn abstain(code: &'static str, detail: String) -> Result<Executed, String> {
    let mut executed = advisory(Vec::new())?;
    executed.ladder.outcome = StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: QuestionKind::RunAdjustment,
            code: RefusalCode::new(code),
            detail,
        }])?,
    };
    Ok(executed)
}

fn conclude_adjustment(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid("a run_adjustment question declares its models"));
    };
    let run_id = text_param(request, "run_id")?.unwrap_or_default();
    let current = Current {
        model: iri_param(request, "current_model")?.to_string(),
        width: u32::try_from(count_param(request, "current_width")?)
            .map_err(|_| invalid("parameter current_width is too large"))?,
        host_ceiling: u32::try_from(count_param(request, "host_width_ceiling")?)
            .map_err(|_| invalid("parameter host_width_ceiling is too large"))?,
        budget_tokens: count_param(request, "budget_tokens")?,
        mid_run: match param(request, "mid_run")? {
            Some(TypedValue::Bool(value)) => *value,
            Some(_) => return Err(invalid("parameter mid_run is not a boolean")),
            None => false,
        },
    };
    let knobs = WidthKnobs::from_policy(policy);
    let read = ctx.observation_windows.unwrap_or_default();
    let runs = select_windows(read.to_vec(), ctx.now_ms, &knobs);
    if !run_id.is_empty() {
        if !read.is_empty() && runs.is_empty() {
            return abstain(
                WINDOW_STALE,
                format!(
                    "every window of run {run_id} is older than width.window_max_age_s {}",
                    knobs.window_max_age_s
                ),
            );
        }
        if let Some(Measured::Unknown(reason)) = runs.first().map(|newest| newest.error_rate()) {
            return abstain(
                RATE_UNKNOWN,
                format!("the newest window of run {run_id} has no error rate: {reason}"),
            );
        }
    }
    let newest = runs.first();
    let window = ErrorWindow {
        tenant: ctx.tenant_id.to_string(),
        run_id: run_id.to_string(),
        window_id: newest.map(|w| w.window_id.clone()).unwrap_or_default(),
        window_revision: newest.map_or(0, |w| w.window_revision),
        runs: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        errored_runs: u32::try_from(runs.iter().filter(|run| errored(run)).count())
            .unwrap_or(u32::MAX),
    };
    let observed = observed(&runs);
    match decide_adjustment(
        &window,
        &observed,
        &current,
        &tiers(options.as_slice())?,
        &knobs,
    ) {
        Ok(adjustment) => {
            let mut executed = advisory(vec![
                format!("model:{}", adjustment.after_model),
                format!("width:{}", adjustment.after_width),
                format!("budget_tokens:{}", adjustment.after_budget_tokens),
            ])?;
            // The counts come from the observation windows; the newest window
            // is the node that holds them.
            let newest = window.window_id.clone();
            let counted = |subject: &'static str, fact: String| FamilyPremise {
                subject: subject.into(),
                fact,
                class: PremiseClass::Observation,
                source: FamilyPremiseSource::Observation(newest.clone()),
            };
            let stated = |subject: &'static str, fact: String, field: &'static str| FamilyPremise {
                subject: subject.into(),
                fact,
                class: PremiseClass::Claim,
                source: FamilyPremiseSource::Request(field),
            };
            let rationale = serde_json::to_value(adjustment.rationale)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .ok_or_else(|| invalid("the adjustment rationale is not a name"))?;
            executed.family_premises = vec![
                counted(
                    "run_adjustment_runs",
                    format!("run_adjustment.runs={}", window.runs),
                ),
                counted(
                    "run_adjustment_errored_runs",
                    format!("run_adjustment.errored_runs={}", window.errored_runs),
                ),
                counted(
                    "run_adjustment_error_rate",
                    format!("run_adjustment.error_rate_q32={}", error_rate_q32(&window)),
                ),
                counted(
                    "run_adjustment_window_id",
                    format!("run_adjustment.window_id={}", window.window_id),
                ),
                counted(
                    "run_adjustment_window_revision",
                    format!("run_adjustment.window_revision={}", window.window_revision),
                ),
                stated(
                    "run_adjustment_run_id",
                    format!("run_adjustment.run_id={run_id}"),
                    "params.run_id",
                ),
                stated(
                    "run_adjustment_current_model",
                    format!("run_adjustment.current_model={}", current.model),
                    "params.current_model",
                ),
                stated(
                    "run_adjustment_current_width",
                    format!("run_adjustment.current_width={}", current.width),
                    "params.current_width",
                ),
                stated(
                    "run_adjustment_host_ceiling",
                    format!("run_adjustment.host_width_ceiling={}", current.host_ceiling),
                    "params.host_width_ceiling",
                ),
                stated(
                    "run_adjustment_budget_tokens",
                    format!("run_adjustment.budget_tokens={}", current.budget_tokens),
                    "params.budget_tokens",
                ),
                counted(
                    "run_adjustment_rationale",
                    format!("run_adjustment.rationale={rationale}"),
                ),
                counted(
                    "run_adjustment_rationale_digest",
                    format!(
                        "run_adjustment.rationale_digest={}",
                        adjustment.rationale_digest.to_hex()
                    ),
                ),
            ];
            Ok(executed)
        }
        Err(AdjustmentRefusal::InsufficientSamples { .. }) => abstain(
            INSUFFICIENT_SAMPLES,
            format!(
                "{} runs are below width.min_runs {}",
                window.runs, knobs.min_runs
            ),
        ),
        Err(AdjustmentRefusal::Invalid {
            code: LATENCY_UNKNOWN,
            detail,
        }) => abstain(LATENCY_UNKNOWN, detail),
        Err(AdjustmentRefusal::Invalid { code, detail }) => {
            Err(invalid(format!("{code}: {detail}")))
        }
    }
}

impl DecisionFamily for RunAdjustmentFamily {
    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_adjustment(ctx, request, policy))
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
