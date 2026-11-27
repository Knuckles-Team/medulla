//! The `route` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R035.750.1).
//!
//! A route question runs the rule stages of the route pipeline first
//! (`route_pipeline`): when the turn is short enough the fast path answers;
//! otherwise team reuse answers with the option the caller's own earlier
//! decisions in this context already ran, when it has been judged often enough
//! and well enough (`route.team_reuse_min_samples`, `route.team_reuse_min_rate`).
//! When no rule stage answers, the family concludes nothing and the shared
//! matrix ladder decides: the ladder is the learned-policy stage. The turn size
//! is the typed int param `route.turn_tokens`; a question that does not state
//! it, or that declares no options, has no fast path to run.

use medulla_decision_stats::decision::ladder::{cost_tie_break, tie_ids, CostTie, LadderResult};
use medulla_decision_stats_records::decision::candidate::CandidateView;
use std::collections::BTreeMap;

use medulla_decision_types::decision::codes::PREFERENCE_NOT_LEGAL;
use medulla_decision_types::decision::knob_families::route::{
    cost_tie_epsilon_micros, fast_path_max_tokens, team_reuse_floors,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, ScoredOption, StatisticalErrorCode, StatisticalOutcome,
    TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, QuantScaleTag,
    QuantisedValue, ResolutionKind, StatisticalDecisionRecord,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_stats_records::decision::route::route_pipeline,
    medulla_decision_stats_records::decision::route::FastPath,
    medulla_decision_stats_records::decision::route::OptionHistory,
    medulla_decision_stats_records::decision::route::RouteChoice,
    medulla_decision_stats_records::decision::route::RouteInput,
    medulla_decision_stats_records::decision::route::RouteOption,
    medulla_decision_stats_records::decision::route::RouteStage,
    medulla_decision_stats_records::decision::route::TeamReuse,
    medulla_decision_stats_records::decision::route::FAST_PATH_STAGE,
};

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource, Pinned, RouteExecuted,
};
use crate::decide::stat_log::{evaluations_of, OBSERVED_COST_KEY};
use crate::decide::stat_support::refusal;

use super::DecisionFamily;

/// The typed param naming the size of the turn being routed, in tokens.
const TURN_TOKENS_PARAM: &str = "route.turn_tokens";

/// A chosen route option at full (Q32 `1.0`) uncalibrated score.
const CHOSEN_SCORE: i64 = 1i64 << 32;

/// The fact prefix of the premise that records one tie-set option's observed
/// cost, `observed_cost:<option>=<micro-units>` (R039.2).
const OBSERVED_COST_FACT: &str = "observed_cost:";

/// The typed param naming the option the caller prefers (R039.3).
const PREFERRED_OPTION_PARAM: &str = "route.preferred_option";

/// The fact prefix of the premise that records a preferred option the route
/// honoured, `preferred_option:<option>` (R039.3). The preference is a claim.
const PREFERRED_FACT: &str = "preferred_option:";

/// The fact prefix of the premise that records a preferred option the route
/// could not honour, `preference_not_honoured:<option>` (R039.3). Its subject
/// is the registered code `PREFERENCE_NOT_LEGAL`.
const NOT_HONOURED_FACT: &str = "preference_not_honoured:";

/// The family of the `route` question kind.
pub struct RouteFamily;

fn turn_tokens(request: &DecideRequest) -> Result<Option<i64>, String> {
    match request
        .params
        .iter()
        .find(|param| param.name == TURN_TOKENS_PARAM)
    {
        None => Ok(None),
        Some(param) => match &param.value {
            TypedValue::Int(tokens) if *tokens >= 0 => Ok(Some(*tokens)),
            TypedValue::Int(_) => Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "route.turn_tokens must not be negative",
            )),
            _ => Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "route.turn_tokens must be an int",
            )),
        },
    }
}

/// Per executed option, how many of the matched decisions' judgements counted
/// (not censored) and how many of those were successes. An option's
/// judgements are every independent evaluation of every matched decision that
/// ran it.
fn history_of(ctx: &ExecutionContext) -> Result<BTreeMap<String, (u64, u64)>, String> {
    let mut history: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for matched in ctx.matched {
        let slot = history.entry(matched.option_id.clone()).or_default();
        for stored in evaluations_of(ctx.store, ctx.tenant_id, &matched.record_id)? {
            if let Some(success) = stored.evaluation.success {
                slot.0 += 1;
                slot.1 += u64::from(success);
            }
        }
    }
    Ok(history)
}

fn executed(choice: &RouteChoice, ctx: &ExecutionContext) -> Result<Executed, String> {
    let scores = BoundedVec::new(vec![ScoredOption {
        option_id: choice.option_id.clone(),
        score: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: CHOSEN_SCORE,
        },
        probability: None,
    }])
    .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    Ok(Executed {
        matrix: None,
        ladder: LadderResult {
            outcome: StatisticalOutcome::advisory(scores, false),
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        family: None,
        family_premises: Vec::new(),
        route: Some(RouteExecuted {
            rule: choice.rule,
            option_id: choice.option_id.clone(),
            resolution: if choice.rule == FAST_PATH_STAGE {
                ResolutionKind::Constraint
            } else {
                ResolutionKind::Statistical
            },
            matched_records: if choice.rule == FAST_PATH_STAGE {
                Vec::new()
            } else {
                ctx.matched
                    .iter()
                    .filter(|matched| matched.option_id == choice.option_id)
                    .map(|matched| matched.record_id.clone())
                    .collect()
            },
        }),
    })
}

/// The `Q32` scale of the epsilon knob: micros of a unit, as a `Q32` integer.
fn epsilon_q32(policy: &DecisionPolicy) -> i64 {
    let micros = i128::from(cost_tie_epsilon_micros(policy));
    i64::try_from((micros << 32) / 1_000_000).unwrap_or(i64::MAX)
}

/// Each option's observed cost in micro-units, read from the `Q32` number the
/// ledger fill wrote; an option without the number has no cost.
fn costs_of_views(views: &[CandidateView]) -> BTreeMap<String, u64> {
    views
        .iter()
        .filter_map(|view| {
            let q32 = *view.numbers.get(OBSERVED_COST_KEY)?;
            Some((view.id.clone(), u64::try_from(q32 >> 32).ok()?))
        })
        .collect()
}

/// The caller's preferred option, when the request states one (R039.3).
fn preferred_option(request: &DecideRequest) -> Result<Option<String>, String> {
    match request
        .params
        .iter()
        .find(|param| param.name == PREFERRED_OPTION_PARAM)
    {
        None => Ok(None),
        Some(param) => match &param.value {
            TypedValue::Text(option) if !option.is_empty() => Ok(Some(option.clone())),
            _ => Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "route.preferred_option must be a non-empty text",
            )),
        },
    }
}

fn preference_premise(option_id: &str, honoured: bool) -> FamilyPremise {
    FamilyPremise {
        subject: if honoured {
            "route_preference".into()
        } else {
            PREFERENCE_NOT_LEGAL.as_str().into()
        },
        fact: format!(
            "{}{option_id}",
            if honoured {
                PREFERRED_FACT
            } else {
                NOT_HONOURED_FACT
            }
        ),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(PREFERRED_OPTION_PARAM),
    }
}

fn observed_cost_premise(option_id: &str, cost: u64) -> FamilyPremise {
    FamilyPremise {
        subject: "observed_cost".into(),
        fact: format!("{OBSERVED_COST_FACT}{option_id}={cost}"),
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(format!("usage_ledger:{option_id}")),
    }
}

/// Move `option_id` to the front of an advisory score list.
fn promote(
    scores: &[ScoredOption],
    option_id: &str,
    calibrated: bool,
) -> Result<StatisticalOutcome, String> {
    let mut ordered: Vec<ScoredOption> = scores
        .iter()
        .filter(|score| score.option_id == option_id)
        .cloned()
        .collect();
    ordered.extend(scores.iter().filter(|s| s.option_id != option_id).cloned());
    Ok(StatisticalOutcome::advisory(
        BoundedVec::new(ordered)
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?,
        calibrated,
    ))
}

/// Honour a caller-preferred option when it is legal, then prefer the
/// cheapest option among the near-tied advisory scores (R039.2, R039.3).
/// An answer a rule stage gave, and any outcome that is not an advisory
/// score list, stand as they are: a calibrated act is licensed for its own
/// top option and a rule stage chose by its own rule. A preferred option is
/// legal when the ladder scored it (the preference never widens the legal
/// set) and its score lies within epsilon of the best; a legal preference
/// wins the tie. An illegal one is recorded as `preference_not_honoured` and
/// the options rank as before. A tie-set option with no observed cost
/// abstains naming the missing fact.
fn apply_cost_tie(
    mut executed: Executed,
    costs: &BTreeMap<String, u64>,
    epsilon_q32: i64,
    preferred: Option<&str>,
) -> Result<Executed, String> {
    if executed.route.is_some() {
        return Ok(executed);
    }
    let StatisticalOutcome::Advisory {
        scores, calibrated, ..
    } = &executed.ladder.outcome
    else {
        return Ok(executed);
    };
    let (scores, calibrated) = (scores.as_slice().to_vec(), *calibrated);
    let mut preference: Vec<FamilyPremise> = Vec::new();
    if let Some(option_id) = preferred {
        if tie_ids(&scores, epsilon_q32).contains(&option_id) {
            executed.ladder.outcome = promote(&scores, option_id, calibrated)?;
            executed.family_premises = vec![preference_premise(option_id, true)];
            return Ok(executed);
        }
        preference.push(preference_premise(option_id, false));
    }
    match cost_tie_break(&scores, costs, epsilon_q32) {
        CostTie::NoTie => {
            executed.family_premises = preference;
            Ok(executed)
        }
        CostTie::Chosen { option_id, tie_set } => {
            executed.ladder.outcome = promote(&scores, &option_id, calibrated)?;
            executed.family_premises = preference
                .into_iter()
                .chain(
                    tie_set
                        .iter()
                        .map(|(id, cost)| observed_cost_premise(id, *cost)),
                )
                .collect();
            Ok(executed)
        }
        CostTie::MissingCost { option_id } => {
            executed.family_premises =
                preference
                    .into_iter()
                    .chain(tie_ids(&scores, epsilon_q32).into_iter().filter_map(|id| {
                        costs.get(id).map(|cost| observed_cost_premise(id, *cost))
                    }))
                    .collect();
            executed.ladder = LadderResult {
                outcome: StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::UnknownFact {
                        component_id: option_id,
                        field: OBSERVED_COST_KEY.to_string(),
                    }])
                    .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?,
                },
                calibration: None,
                explained: None,
                audit: None,
                logging: Vec::new(),
            };
            Ok(executed)
        }
    }
}

/// The replay of the cost tie-break: a logged record has no ledger to read,
/// so the observed costs it sealed as premises stand in for it, and the
/// preference the request stated is read back from its premise.
pub(in crate::decide) fn replay_cost_tie(
    record: &StatisticalDecisionRecord,
    executed: Executed,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let costs: BTreeMap<String, u64> = record
        .premises
        .iter()
        .filter_map(|premise| {
            let (id, cost) = premise
                .fact
                .strip_prefix(OBSERVED_COST_FACT)?
                .rsplit_once('=')?;
            Some((id.to_string(), cost.parse::<u64>().ok()?))
        })
        .collect();
    let preferred = record.premises.iter().find_map(|premise| {
        premise
            .fact
            .strip_prefix(PREFERRED_FACT)
            .or_else(|| premise.fact.strip_prefix(NOT_HONOURED_FACT))
    });
    apply_cost_tie(executed, &costs, epsilon_q32(policy), preferred)
}

impl DecisionFamily for RouteFamily {
    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let tokens = match turn_tokens(request) {
            Ok(tokens) => tokens,
            Err(detail) => return Some(Err(detail)),
        };
        let CandidateSource::Declared { options } = &request.candidates else {
            return None;
        };
        let input = RouteInput {
            turn_tokens: tokens,
            options: options
                .iter()
                .map(|option| RouteOption {
                    option_id: &option.option_id,
                    classification: option.classification.as_slice(),
                })
                .collect(),
        };
        let fast_path = FastPath {
            max_tokens: fast_path_max_tokens(policy),
        };
        let history = match history_of(ctx) {
            Ok(history) => history,
            Err(detail) => return Some(Err(detail)),
        };
        let (min_samples, min_rate_micros) = team_reuse_floors(policy);
        let team_reuse = TeamReuse {
            history: history
                .iter()
                .map(|(option_id, (evaluated, successes))| OptionHistory {
                    option_id,
                    evaluated: *evaluated,
                    successes: *successes,
                })
                .collect(),
            min_samples,
            min_rate_micros,
        };
        let stages: [&dyn RouteStage; 2] = [&fast_path, &team_reuse];
        // No rule stage answering is the ladder's turn, not a refusal here.
        route_pipeline(&stages, &input)
            .ok()
            .map(|choice| executed(&choice, ctx))
    }

    fn refine(
        &self,
        request: Option<&DecideRequest>,
        _pinned: &Pinned,
        views: &[CandidateView],
        policy: &DecisionPolicy,
        _policy_digest: &str,
        executed: Executed,
    ) -> Result<Executed, String> {
        // A replay has no ledger; `replay_cost_tie` carries it from the record.
        if request.is_none() {
            return Ok(executed);
        }
        let preferred = match request.map(preferred_option).transpose() {
            Ok(preferred) => preferred.flatten(),
            Err(detail) => return Err(detail),
        };
        apply_cost_tie(
            executed,
            &costs_of_views(views),
            epsilon_q32(policy),
            preferred.as_deref(),
        )
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let observed = executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest));
        let route = executed
            .route
            .iter()
            .flat_map(|route| {
                let stage = PremiseRef {
                    subject: route.option_id.clone(),
                    fact: format!("route_stage={}", route.rule),
                    class: PremiseClass::Definition,
                    provenance: PremiseProvenance::Policy {
                        policy_digest: policy_digest.to_string(),
                    },
                    trust_tier: None,
                };
                let matched = route.matched_records.iter().map(|record_id| PremiseRef {
                    subject: route.option_id.clone(),
                    fact: format!("matched_record:{record_id}"),
                    class: PremiseClass::Observation,
                    provenance: PremiseProvenance::Observation {
                        evaluation_id: record_id.clone(),
                    },
                    trust_tier: None,
                });
                std::iter::once(stage).chain(matched)
            })
            .collect::<Vec<_>>();
        route.into_iter().chain(observed).collect()
    }
}
