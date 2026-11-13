//! The `feedback_loop` question family (MEDULLA-DECISION-ENGINE-R169.3).
//!
//! A checker reviews each round of an agent's output and the loop decides
//! whether to go on. The model and the rule are in
//! `medulla_decision_types::decision::checker_feedback` (`next_round`,
//! `effective_bound`). The two candidates are `continue` and `stop`. `Decide`
//! is evaluate-only; the caller commits each round through the decision log,
//! and the latest committed record of a loop id is the durable continuation
//! ([`latest_continuation`]).

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::checker_feedback::{
    effective_bound, next_round, Continuation, FeedbackRecord, StopReason, FEEDBACK_BOUND_INVALID,
    FEEDBACK_REPLAYED_KEY, FEEDBACK_UNGRADED_OUTPUT, KNOBS,
};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalDecisionRecord, StatisticalOutcome,
    TypedParam, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_types::contract::{BoundedVec, Digest256};

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_log::{visible_entries, LogReader};
use crate::decide::stat_support::invalid_parameter;

use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R169.4), declared
/// beside the model so the uniqueness test of the code registry collects it.
pub use medulla_decision_types::decision::checker_feedback::REFUSALS;

/// The param that carries one round's typed feedback record.
pub(crate) const PARAM_FEEDBACK: &str = "feedback";
/// The param that carries the caller's round bound.
pub(crate) const PARAM_BOUND: &str = "bound";
const CONTINUE: &str = "continue";
const STOP: &str = "stop";

/// The family of the `feedback_loop` question kind.
pub struct FeedbackLoopFamily;

fn coded(code: medulla_types::refusal_code::RefusalCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The feedback record a `feedback_loop` question carries.
fn record_of(params: &[TypedParam]) -> Result<&FeedbackRecord, String> {
    params
        .iter()
        .find_map(|param| match (&param.value, param.name == PARAM_FEEDBACK) {
            (TypedValue::Feedback(record), true) => Some(record.as_ref()),
            _ => None,
        })
        .ok_or_else(|| {
            invalid_parameter(format!(
                "a feedback_loop question requires a {PARAM_FEEDBACK} param"
            ))
        })
}

/// The caller's round bound, before it is checked against the knob.
fn bound_of(params: &[TypedParam]) -> Result<i64, String> {
    params
        .iter()
        .find_map(|param| match (&param.value, param.name == PARAM_BOUND) {
            (TypedValue::Int(bound), true) => Some(*bound),
            _ => None,
        })
        .ok_or_else(|| {
            invalid_parameter(format!(
                "a feedback_loop question requires an integer {PARAM_BOUND} param"
            ))
        })
}

/// The feedback record a committed record sealed, when it is a feedback round.
fn feedback_of(record: &StatisticalDecisionRecord) -> Option<&FeedbackRecord> {
    if record.question.kind != QuestionKind::FeedbackLoop {
        return None;
    }
    record
        .inputs
        .params
        .iter()
        .find_map(|param| match &param.value {
            TypedValue::Feedback(feedback) if param.name == PARAM_FEEDBACK => {
                Some(feedback.as_ref())
            }
            _ => None,
        })
}

/// The continuation one committed round sealed: the loop stops when its
/// decision chose `stop`, for `NoFindings` when the round found nothing and
/// for `RoundBound` otherwise.
fn continuation_of(record: &StatisticalDecisionRecord) -> Option<Continuation> {
    let feedback = feedback_of(record)?;
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        return None;
    };
    let stops = scores
        .iter()
        .any(|score| score.option_id == STOP && score.score.value != 0);
    let stop = stops.then(|| {
        if feedback.finding_count == 0 {
            StopReason::NoFindings
        } else {
            StopReason::RoundBound
        }
    });
    Some(Continuation {
        loop_id: feedback.loop_id.clone(),
        round: feedback.round,
        stop,
    })
}

/// The continuation of `loop_id`: the latest committed round the reader can
/// see (MEDULLA-DECISION-ENGINE-R169.3). `None` when no round is committed.
pub(crate) fn latest_continuation(
    store: &AgentLibraryStore,
    reader: &LogReader,
    loop_id: &str,
) -> Result<Option<Continuation>, String> {
    Ok(visible_entries(store, reader)?
        .iter()
        .filter_map(|entry| continuation_of(&entry.record))
        .filter(|continuation| continuation.loop_id == loop_id)
        .max_by_key(|continuation| continuation.round))
}

fn executed(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        thought: None,
        context_selection: None,
        family_premises: Vec::new(),
        route: None,
        answered: None,
    }
}

fn scored(option_id: &str, chosen: bool) -> ScoredOption {
    ScoredOption {
        option_id: option_id.to_string(),
        score: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: i64::from(chosen) << 32,
        },
        probability: None,
    }
}

/// The bound knob in force: `feedback.max_rounds` from the pinned policy.
fn max_rounds(policy: &DecisionPolicy) -> Result<u32, String> {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(rounds) => u32::try_from(rounds)
            .map_err(|_| invalid_parameter("feedback.max_rounds is out of range")),
        _ => Err(invalid_parameter(
            "feedback.max_rounds has the wrong value type",
        )),
    }
}

impl DecisionFamily for FeedbackLoopFamily {
    /// The two candidates are `continue` and `stop`.
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        let mut ids: Vec<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
        ids.sort_unstable();
        if ids == [CONTINUE, STOP] {
            Ok(())
        } else {
            Err(invalid_parameter(format!(
                "a feedback_loop question declares exactly the options {CONTINUE} and {STOP}"
            )))
        }
    }

    /// The request carries one round and a bound; an ungraded output is
    /// refused here, before any store read.
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let record = record_of(request.params.as_slice())?;
        bound_of(request.params.as_slice())?;
        if record.checker_report_digest == Digest256::from_bytes([0u8; 32]) {
            return Err(coded(
                FEEDBACK_UNGRADED_OUTPUT,
                "the round carries no checker report digest",
            ));
        }
        Ok(())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude(ctx, request, policy))
    }

    /// The record's evidence (MEDULLA-DECISION-ENGINE-R169.5): the findings
    /// digest and count, the next-instruction digest and the round. All are
    /// `Claim` premises, because the caller supplies the findings.
    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let Ok(record) = record_of(request.params.as_slice()) else {
            return Vec::new();
        };
        let claim = |fact: String| PremiseRef {
            subject: record.loop_id.clone(),
            fact,
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: PARAM_FEEDBACK.to_string(),
            },
        };
        vec![
            claim(format!(
                "feedback:findings digest={} count={}",
                record.findings_digest.to_hex(),
                record.finding_count
            )),
            claim(format!(
                "feedback:instruction digest={}",
                record.next_instruction_digest.to_hex()
            )),
            claim(format!("feedback:round={}", record.round)),
        ]
    }

    /// A continue or stop answer is a constraint.
    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => None,
            _ => Some(ResolutionKind::Constraint),
        }
    }
}

/// The outcome of one round: the caller's bound is checked against the
/// `feedback.max_rounds` knob and the continue-or-stop rule decides. Pure over
/// the typed params and the pinned policy, so the log's verify-replay
/// re-derives it exactly (MEDULLA-DECISION-ENGINE-R169.3).
pub(crate) fn round_outcome(
    params: &[TypedParam],
    policy: &DecisionPolicy,
) -> Result<StatisticalOutcome, String> {
    let record = record_of(params)?;
    let knob = max_rounds(policy)?;
    let bound = u32::try_from(bound_of(params)?)
        .ok()
        .filter(|bound| (1..=knob).contains(bound))
        .ok_or_else(|| {
            coded(
                FEEDBACK_BOUND_INVALID,
                format!("the round bound must be within [1, {knob}]"),
            )
        })?;
    let continuation = next_round(
        &record.loop_id,
        record.finding_count,
        record.round,
        effective_bound(bound, knob),
    );
    let stops = continuation.stop.is_some();
    Ok(StatisticalOutcome::Advisory {
        scores: BoundedVec::new(vec![scored(CONTINUE, !stops), scored(STOP, stops)])
            .expect("two scores fit the bound"),
        calibrated: false,
    })
}

fn conclude(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let params = request.params.as_slice();
    let record = record_of(params)?;
    let outcome = round_outcome(params, policy)?;
    // A round already committed under this loop id with other content is a
    // replayed key; the same content is an idempotent repeat.
    if let Some(reader) = ctx.reader {
        let conflicting = visible_entries(ctx.store, reader)?.iter().any(|entry| {
            feedback_of(&entry.record).is_some_and(|committed| {
                committed.loop_id == record.loop_id
                    && committed.round == record.round
                    && committed != record
            })
        });
        if conflicting {
            return Err(coded(
                FEEDBACK_REPLAYED_KEY,
                format!(
                    "loop {} round {} already holds a committed record with different content",
                    record.loop_id, record.round
                ),
            ));
        }
    }
    Ok(executed(outcome))
}
