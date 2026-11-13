//! The `over_window_plan` question family (MEDULLA-DECISION-ENGINE-R137.3,
//! MEDULLA-DECISION-ENGINE-R137.4).
//!
//! Plans how a prompt longer than the model window is split: the window comes
//! from the model profile (an observation), the request (a claim) or the
//! `context.default_window_tokens` knob, in that order. A request limit may only
//! tighten its knob value. The family calls `decide_over_window` and answers an
//! advisory; `Decide` is evaluate-only and writes nothing.

use medulla_agent_types::agent_component::{AgentComponentFacts, AgentComponentKind};
use medulla_decision_outcome_types::decision::context_budget::{
    decide_over_window, ContextBudgetLimit, ContextBudgetRefusal, OverWindowPlan,
    ParallelMapLimits, WindowBudget,
};
use medulla_decision_types::decision::knob_families::context;
use medulla_decision_types::decision::numeric::UnitRationalWire;
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, param, single_option, unsigned_param};
use super::DecisionFamily;

/// The one option a context budget question declares.
pub const PLAN_OPTION: &str = "plan";

/// The family of the `over_window_plan` question kind.
pub struct OverWindowPlanFamily;

/// What the request asks, before any knob or profile is read.
struct Asked {
    prompt_tokens: u64,
    window_tokens: Option<u64>,
    profile_ref: Option<String>,
    max_depth: Option<u64>,
    max_items: Option<u64>,
    max_cost: Option<u64>,
    max_model_calls: Option<u64>,
}

fn asked(request: &DecideRequest) -> Result<Asked, String> {
    let prompt_tokens = unsigned_param(request, "prompt_tokens")?
        .ok_or_else(|| invalid("parameter prompt_tokens is required"))?;
    let profile_ref = match param(request, "model_profile_ref")? {
        None => None,
        Some(TypedValue::Iri(iri)) => Some(iri.clone()),
        Some(_) => return Err(invalid("parameter model_profile_ref is not an IRI")),
    };
    Ok(Asked {
        prompt_tokens,
        window_tokens: unsigned_param(request, "window_tokens")?,
        profile_ref,
        max_depth: unsigned_param(request, "max_depth")?,
        max_items: unsigned_param(request, "max_items")?,
        max_cost: unsigned_param(request, "max_cost")?,
        max_model_calls: unsigned_param(request, "max_model_calls")?,
    })
}

/// A request limit may only tighten the knob value.
fn tighten(knob: u64, asked: Option<u64>) -> u64 {
    asked.map_or(knob, |value| value.min(knob))
}

/// Where the window the plan used came from.
enum WindowSource {
    /// The tenant's model profile, at the revision read.
    Profile {
        component_id: String,
        definition_digest: String,
    },
    /// The request's `window_tokens`.
    Request,
    /// The `context.default_window_tokens` knob.
    Default,
}

/// The window: the profile (the tenant's own, never another's), then the
/// request, then the default knob.
fn window(
    ctx: &ExecutionContext,
    asked: &Asked,
    policy: &DecisionPolicy,
) -> Result<(u64, WindowSource), String> {
    if let Some(profile) = &asked.profile_ref {
        let entry = ctx
            .store
            .current_component(ctx.tenant_id, profile)
            .map_err(invalid)?;
        if let Some(entry) = entry {
            if entry.kind == AgentComponentKind::ModelProfile {
                if let AgentComponentFacts::ModelProfile {
                    context_window_tokens,
                    ..
                } = entry.facts
                {
                    return Ok((
                        u64::from(context_window_tokens),
                        WindowSource::Profile {
                            component_id: entry.component_id.clone(),
                            definition_digest: entry.definition_digest.clone(),
                        },
                    ));
                }
            }
        }
    }
    Ok(match asked.window_tokens {
        Some(tokens) => (tokens, WindowSource::Request),
        None => (context::integer(policy, 5), WindowSource::Default),
    })
}

fn limit_name(limit: ContextBudgetLimit) -> &'static str {
    match limit {
        ContextBudgetLimit::MaxDepth => "max_depth",
        ContextBudgetLimit::MaxCost => "max_cost",
        ContextBudgetLimit::MaxModelCalls => "max_model_calls",
        ContextBudgetLimit::WindowTokens => "window_tokens",
    }
}

/// A budget refusal as the wire error: `PARAMETER_INVALID` naming the registered
/// code, the limit and the observed value. It carries no plan.
fn refused(refusal: ContextBudgetRefusal) -> String {
    let detail = match refusal {
        ContextBudgetRefusal::BudgetExhausted { limit, observed } => {
            format!("limit={} observed={observed}", limit_name(limit))
        }
        ContextBudgetRefusal::TooManyItems { observed, .. } => {
            format!("limit=max_items observed={observed}")
        }
    };
    invalid(format!("{}: {detail}", refusal.code().as_str()))
}

/// The plan the request and the pinned policy produce, with the premises it
/// rests on (MEDULLA-DECISION-ENGINE-R137.5).
fn plan(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<(OverWindowPlan, Vec<FamilyPremise>), String> {
    let asked = asked(request)?;
    let fraction = UnitRationalWire::new(context::over_window_fraction_micros(policy), 1_000_000)
        .map_err(invalid)?;
    let (window_tokens, source) = window(ctx, &asked, policy)?;
    let budget = WindowBudget {
        window_tokens,
        over_window_fraction: fraction,
    };
    let depth = tighten(context::integer(policy, 1), asked.max_depth);
    let items = tighten(context::integer(policy, 2), asked.max_items);
    let calls = tighten(context::integer(policy, 3), asked.max_model_calls);
    let cost = tighten(context::integer(policy, 4), asked.max_cost);
    let limits = ParallelMapLimits {
        max_depth: u8::try_from(depth).unwrap_or(u8::MAX),
        max_items: u32::try_from(items).unwrap_or(u32::MAX),
        max_model_calls: u32::try_from(calls).unwrap_or(u32::MAX),
        max_cost: cost,
    };
    let plan = decide_over_window(asked.prompt_tokens, &budget, &limits).map_err(refused)?;
    let premise = |fact: String, class: PremiseClass, source: FamilyPremiseSource| FamilyPremise {
        subject: "context_budget",
        fact,
        class,
        source,
    };
    let fraction_used =
        u128::from(asked.prompt_tokens) * (1u128 << 32) / u128::from(window_tokens.max(1));
    let mut premises = vec![
        premise(
            format!("context_budget.prompt_tokens={}", asked.prompt_tokens),
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.prompt_tokens"),
        ),
        premise(
            format!("context_budget.fraction_used_q32={fraction_used}"),
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.prompt_tokens"),
        ),
    ];
    let window_fact = format!("context_budget.window_tokens={window_tokens}");
    let defaulted = matches!(source, WindowSource::Default);
    let window_premise = match source {
        WindowSource::Profile {
            component_id,
            definition_digest,
        } => premise(
            window_fact,
            PremiseClass::Observation,
            FamilyPremiseSource::Publisher {
                component_id,
                definition_digest,
            },
        ),
        WindowSource::Request => premise(
            window_fact,
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.window_tokens"),
        ),
        WindowSource::Default => premise(
            window_fact,
            PremiseClass::Claim,
            FamilyPremiseSource::Policy,
        ),
    };
    premises.push(window_premise);
    if defaulted {
        premises.push(premise(
            "context_budget.default:window_tokens".to_string(),
            PremiseClass::Definition,
            FamilyPremiseSource::Policy,
        ));
    }
    for (name, value, given) in [
        ("max_depth", depth, asked.max_depth),
        ("max_items", items, asked.max_items),
        ("max_model_calls", calls, asked.max_model_calls),
        ("max_cost", cost, asked.max_cost),
    ] {
        premises.push(premise(
            format!("context_budget.limit:{name}={value}"),
            PremiseClass::Definition,
            FamilyPremiseSource::Policy,
        ));
        if given.is_none() {
            premises.push(premise(
                format!("context_budget.default:{name}"),
                PremiseClass::Definition,
                FamilyPremiseSource::Policy,
            ));
        }
    }
    Ok((plan, premises))
}

impl DecisionFamily for OverWindowPlanFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, PLAN_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        asked(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(plan(ctx, request, policy).and_then(|(plan, premises)| {
            let mut executed = advisory(vec![
                format!("exceeds:{}", plan.exceeds),
                format!("chunks:{}", plan.chunks),
                format!("depth:{}", plan.depth),
                format!("chunk_tokens:{}", plan.chunk_tokens),
                format!("model_calls:{}", plan.model_calls),
            ])?;
            executed.family_premises = premises;
            Ok(executed)
        }))
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
