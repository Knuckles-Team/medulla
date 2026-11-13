//! The `run_adjustment` question family (MEDULLA-DECISION-ENGINE-R158.3).
//!
//! The declared options are the legal models, each carrying a whole-number
//! `tier` encoded as Q32. The caller names the partition (`child_id`,
//! `operation`) and what runs now (`current_model`, `current_width`,
//! `host_width_ceiling`, `budget_tokens`, and `mid_run` when a run is in
//! flight). The family reads the newest `width.window` runs of the tenant's
//! usage ledger for that partition that are no older than
//! `width.window_max_age_s` (`recent_windows`, one ledger query, no second
//! ledger read), counts the errored ones, takes the p95 of the per-run latency
//! and the mean per-run cost, and calls `decide_adjustment`. The answer is an advisory naming
//! `model:<id>`, `width:<n>` and `budget_tokens:<n>`; `Decide` writes nothing.
//! Fewer runs than `width.min_runs` abstains `INSUFFICIENT_SAMPLES`; every
//! other refusal is a `PARAMETER_INVALID` request refusal naming its code.

use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::run_adjustment::{
    decide_adjustment, error_rate_q32, latency_p95, AdjustmentRefusal, Current, ErrorWindow,
    ModelTier, Observed, WidthKnobs, INSUFFICIENT_SAMPLES, LATENCY_UNKNOWN,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{AbstainReason, DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::usage_ledger::RunWindow;
use medulla_types::contract::BoundedVec;
use medulla_types::usage::PriceCatalog;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, int_param, invalid, param, text_param};
use super::DecisionFamily;

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

/// The newest runs of the partition inside the age bound, read from the
/// tenant's usage ledger and priced against the tenant's price entries.
fn recent_windows(
    ctx: &ExecutionContext,
    child_id: &str,
    operation: &str,
    knobs: &WidthKnobs,
) -> Result<Vec<RunWindow>, String> {
    if child_id.is_empty() || operation.is_empty() {
        return Ok(Vec::new());
    }
    let refused = |error: medulla_persistence_records::usage_ledger::UsageRefusal| {
        invalid(format!("the usage ledger refused the read: {error:?}"))
    };
    let catalog = PriceCatalog {
        aliases: std::collections::BTreeMap::new(),
        entries: ctx.store.price_entries(ctx.tenant_id).map_err(refused)?,
    };
    let since_ms = ctx
        .now_ms
        .saturating_sub(knobs.window_max_age_s.saturating_mul(1_000));
    ctx.store
        .recent_run_windows(
            ctx.tenant_id,
            child_id,
            operation,
            knobs.window as usize,
            since_ms,
            &catalog,
        )
        .map_err(refused)
}

/// The latency and cost the windows observed.
fn observed(runs: &[RunWindow]) -> Observed {
    let costs: Option<Vec<u64>> = runs.iter().map(|run| run.cost_micro).collect();
    let predicted_cost_micro = costs
        .filter(|costs| !costs.is_empty())
        .map(|costs| costs.iter().sum::<u64>() / costs.len() as u64);
    Observed {
        latency_p95_ms: latency_p95(runs.iter().map(|run| run.latency_ms).collect()),
        predicted_cost_micro,
    }
}

fn conclude_adjustment(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid("a run_adjustment question declares its models"));
    };
    let child_id = text_param(request, "child_id")?.unwrap_or_default();
    let operation = text_param(request, "operation")?.unwrap_or_default();
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
    let runs = recent_windows(ctx, child_id, operation, &knobs)?;
    let window = ErrorWindow {
        tenant: ctx.tenant_id.to_string(),
        child_id: child_id.to_string(),
        operation: operation.to_string(),
        runs: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        errored_runs: u32::try_from(runs.iter().filter(|run| run.errored).count())
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
            // The counts come from the usage ledger; the newest run is the
            // node that holds them.
            let newest = runs
                .iter()
                .max_by_key(|run| run.newest_at_ms)
                .map(|run| run.run_id.clone())
                .unwrap_or_default();
            let counted = |subject: &'static str, fact: String| FamilyPremise {
                subject,
                fact,
                class: PremiseClass::Observation,
                source: FamilyPremiseSource::Observation(newest.clone()),
            };
            let stated = |subject: &'static str, fact: String, field: &'static str| FamilyPremise {
                subject,
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
                stated(
                    "run_adjustment_child_id",
                    format!("run_adjustment.child_id={child_id}"),
                    "params.child_id",
                ),
                stated(
                    "run_adjustment_operation",
                    format!("run_adjustment.operation={operation}"),
                    "params.operation",
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
        Err(AdjustmentRefusal::InsufficientSamples { .. }) => {
            let mut executed = advisory(Vec::new())?;
            executed.ladder.outcome = StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![AbstainReason::Family {
                    family: QuestionKind::RunAdjustment,
                    code: RefusalCode::new(INSUFFICIENT_SAMPLES),
                    detail: format!(
                        "{} runs are below width.min_runs {}",
                        window.runs, knobs.min_runs
                    ),
                }])?,
            };
            Ok(executed)
        }
        Err(AdjustmentRefusal::Invalid {
            code: LATENCY_UNKNOWN,
            detail,
        }) => {
            let mut executed = advisory(Vec::new())?;
            executed.ladder.outcome = StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![AbstainReason::Family {
                    family: QuestionKind::RunAdjustment,
                    code: RefusalCode::new(LATENCY_UNKNOWN),
                    detail,
                }])?,
            };
            Ok(executed)
        }
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
