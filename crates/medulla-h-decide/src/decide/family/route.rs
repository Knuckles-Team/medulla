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

use medulla_decision_stats::decision::ladder::LadderResult;
use std::collections::BTreeMap;

use medulla_decision_stats::decision::route::{
    route_pipeline, FastPath, OptionHistory, RouteChoice, RouteInput, RouteOption, RouteStage,
    TeamReuse, FAST_PATH_STAGE,
};
use medulla_decision_types::decision::knob_families::route::{
    fast_path_max_tokens, team_reuse_floors,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, ScoredOption, StatisticalErrorCode, StatisticalOutcome,
    TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, QuantScaleTag, QuantisedValue,
    ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext, RouteExecuted};
use crate::decide::stat_log::evaluations_of;
use crate::decide::stat_support::refusal;

use super::DecisionFamily;

/// The typed param naming the size of the turn being routed, in tokens.
const TURN_TOKENS_PARAM: &str = "route.turn_tokens";

/// A chosen route option at full (Q32 `1.0`) uncalibrated score.
const CHOSEN_SCORE: i64 = 1i64 << 32;

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
            outcome: StatisticalOutcome::Advisory {
                scores,
                calibrated: false,
            },
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        context_selection: None,
        thought: None,
        family_premises: Vec::new(),
        answered: None,
        route: Some(RouteExecuted {
            stage: choice.stage,
            option_id: choice.option_id.clone(),
            resolution: if choice.stage == FAST_PATH_STAGE {
                ResolutionKind::Constraint
            } else {
                ResolutionKind::Statistical
            },
            matched_records: if choice.stage == FAST_PATH_STAGE {
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

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .route
            .iter()
            .flat_map(|route| {
                let stage = PremiseRef {
                    subject: route.option_id.clone(),
                    fact: format!("route_stage={}", route.stage),
                    class: PremiseClass::Definition,
                    provenance: PremiseProvenance::Policy {
                        policy_digest: policy_digest.to_string(),
                    },
                };
                let matched = route.matched_records.iter().map(|record_id| PremiseRef {
                    subject: route.option_id.clone(),
                    fact: format!("matched_record:{record_id}"),
                    class: PremiseClass::Observation,
                    provenance: PremiseProvenance::Observation {
                        evaluation_id: record_id.clone(),
                    },
                });
                std::iter::once(stage).chain(matched)
            })
            .collect()
    }
}
