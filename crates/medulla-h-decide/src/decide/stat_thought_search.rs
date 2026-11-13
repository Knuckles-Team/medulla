//! The served side of the `thought_search` family (MEDULLA-DECISION-ENGINE-R750.754).
//!
//! The caller keeps the thought arena and sends a bounded frontier summary in
//! the typed `thought_search` parameter, with its task class in `task_class`.
//! The strategy comes from the pinned policy and the budgets from the
//! `thought_search.*` knobs; a request may lower a budget through an integer
//! parameter named after the knob, never raise it. Evaluate-only.

use medulla_decision_stats::decision::thought_search::choose_expansion;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, StatisticalErrorCode, StatisticalOutcome,
    TypedValue,
};
use medulla_decision_types::decision::thought_search::{
    ReasoningState, ReasoningStrategy, ThoughtSearchAnswer, ThoughtSearchKnobs, KNOBS,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_query_types::solve_scalar::Scalar;
use medulla_types::contract::BoundedVec;

use super::stat_executor::ThoughtExecuted;
use super::stat_support::{invalid_parameter, refusal};

const STATE_PARAM: &str = "thought_search";
const TASK_CLASS_PARAM: &str = "task_class";

/// The request, decoded.
pub(super) struct Parsed<'a> {
    pub(super) state: &'a ReasoningState,
    pub(super) task_class: &'a str,
    /// Budget lowerings by knob index, in `KNOBS` order.
    pub(super) lowered: Vec<(usize, i64)>,
}

/// Decode and shape-check a `thought_search` request.
pub(super) fn parse(request: &DecideRequest) -> Result<Parsed<'_>, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
    {
        return Err(invalid_parameter(
            "thought_search needs ordinary safety, no head and no belief slices",
        ));
    }
    let (mut state, mut task_class) = (None, None);
    let mut lowered = Vec::new();
    for param in request.params.iter() {
        match (param.name.as_str(), &param.value) {
            (STATE_PARAM, TypedValue::ThoughtSearch(body)) => state = Some(&**body),
            (TASK_CLASS_PARAM, TypedValue::Text(text)) => task_class = Some(text.as_str()),
            (name, TypedValue::Int(value)) => {
                let index = KNOBS
                    .iter()
                    .position(|spec| spec.name == name)
                    .ok_or_else(|| {
                        invalid_parameter(format!("thought_search has no parameter {name}"))
                    })?;
                lowered.push((index, *value));
            }
            (name, _) => {
                return Err(invalid_parameter(format!(
                    "thought_search parameter {name} has the wrong name or type"
                )))
            }
        }
    }
    let (Some(state), Some(task_class)) = (state, task_class) else {
        return Err(invalid_parameter(
            "thought_search needs the thought_search and task_class parameters",
        ));
    };
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "thought_search declares its candidates: the leaf ids",
        ));
    };
    let mut leaf_ids: Vec<String> = state.nodes.iter().map(|n| n.id.to_string()).collect();
    leaf_ids.sort();
    let declared: Vec<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
    if declared != leaf_ids.iter().map(String::as_str).collect::<Vec<_>>()
        || options
            .iter()
            .any(|o| !o.classification.is_empty() || !o.numbers.is_empty() || !o.texts.is_empty())
    {
        return Err(refusal(
            StatisticalErrorCode::ThoughtNodeInvalid,
            "declared candidates must be exactly the sorted leaf ids without facts",
        ));
    }
    Ok(Parsed {
        state,
        task_class,
        lowered,
    })
}

fn strategy_name(strategy: ReasoningStrategy) -> &'static str {
    match strategy {
        ReasoningStrategy::Chain => "chain",
        ReasoningStrategy::Tree => "tree",
        ReasoningStrategy::Graph => "graph",
        ReasoningStrategy::Planning => "planning",
    }
}

fn knob_value(knobs: &ThoughtSearchKnobs, index: usize) -> i64 {
    let value = match index {
        0 => knobs.max_nodes,
        1 => u64::from(knobs.max_depth),
        2 => u64::from(knobs.max_branching),
        3 => knobs.max_tokens,
        _ => u64::from(knobs.beam_width),
    };
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn set_knob(knobs: &mut ThoughtSearchKnobs, index: usize, value: i64) {
    let wide = u64::try_from(value).unwrap_or(0);
    let narrow = u32::try_from(value).unwrap_or(0);
    match index {
        0 => knobs.max_nodes = wide,
        1 => knobs.max_depth = narrow,
        2 => knobs.max_branching = narrow,
        3 => knobs.max_tokens = wide,
        _ => knobs.beam_width = narrow,
    }
}

fn claim(subject: &str, fact: String, field: &str) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: field.to_string(),
        },
    }
}

fn premises(
    parsed: &Parsed<'_>,
    strategy: ReasoningStrategy,
    policy_digest: &str,
    answer: &ThoughtSearchAnswer,
) -> Vec<PremiseRef> {
    let ledger = &parsed.state.ledger;
    let field = "params.thought_search";
    let mut out = vec![
        PremiseRef {
            subject: parsed.task_class.to_string(),
            fact: format!("thought:strategy={}", strategy_name(strategy)),
            class: PremiseClass::Definition,
            provenance: PremiseProvenance::Policy {
                policy_digest: policy_digest.to_string(),
            },
        },
        claim(
            "ledger",
            format!(
                "thought:ledger={}/{}/{}",
                ledger.nodes_used, ledger.tokens_used, ledger.depth_reached
            ),
            field,
        ),
    ];
    if let ThoughtSearchAnswer::Expand { node_ids } = answer {
        out.extend(
            node_ids
                .iter()
                .map(|id| claim(&id.to_string(), format!("thought:expand={id}"), field)),
        );
    }
    out.extend(parsed.state.nodes.iter().map(|n| {
        claim(
            &n.id.to_string(),
            format!("thought:score={}/{}", n.id, n.score.value),
            field,
        )
    }));
    out
}

/// Conclude one thought-search decision: the answer, its outcome and premises.
pub(super) fn conclude(
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<(StatisticalOutcome, Option<ThoughtExecuted>), String> {
    let parsed = parse(request)?;
    let strategy = *policy.reasoning.get(parsed.task_class).ok_or_else(|| {
        refusal(
            StatisticalErrorCode::ThoughtStrategyUnknown,
            format!(
                "the policy names no strategy for task class {}",
                parsed.task_class
            ),
        )
    })?;
    parsed
        .state
        .validate(strategy == ReasoningStrategy::Graph)
        .map_err(|fault| fault.refusal())?;
    let mut knobs = ThoughtSearchKnobs::from_policy(policy);
    for (index, value) in &parsed.lowered {
        let spec = &KNOBS[*index];
        if !spec.admits(&KnobValue::Integer(*value)) {
            return Err(refusal(
                StatisticalErrorCode::ThoughtBudgetInvalid,
                format!("{} is outside [min, max]", spec.name),
            ));
        }
        if *value > knob_value(&knobs, *index) {
            let reasons = vec![AbstainReason::PolicyLoosening {
                field: spec.name.to_string(),
            }];
            return Ok((
                StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(reasons).expect("one reason"),
                },
                None,
            ));
        }
        set_knob(&mut knobs, *index, *value);
    }
    let answer = choose_expansion(parsed.state, strategy, &knobs);
    let outcome = match &answer {
        // A budget stop is an abstention that names the best node.
        ThoughtSearchAnswer::Stop {
            best_node,
            certified_gap: true,
            gap,
            ..
        } => StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::BudgetExhausted {
                incumbent: Some(best_node.to_string()),
                lower_bound: Scalar::new(i128::from(gap.value)),
            }])
            .expect("one reason"),
        },
        // Scores are caller claims: neither calibrated nor an authorization.
        _ => StatisticalOutcome::Advisory {
            scores: BoundedVec::default(),
            calibrated: false,
        },
    };
    let policy_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    let premises = premises(&parsed, strategy, &policy_digest, &answer);
    Ok((outcome, Some(ThoughtExecuted { answer, premises })))
}
