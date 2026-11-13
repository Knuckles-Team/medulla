//! Shared helpers of the served question families: typed parameter reads, the
//! one-declared-option gate and the advisory answer every plan-shaped family
//! returns.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, ScoredOption, StatisticalErrorCode, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{QuantScaleTag, QuantisedValue};
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::Executed;
use crate::decide::stat_support::refusal;

/// The Q32 fixed-point value for "chosen".
const CHOSEN_SCORE: i64 = 1i64 << 32;

/// A `PARAMETER_INVALID` refusal.
pub(super) fn invalid(detail: impl std::fmt::Display) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail)
}

/// The value of the parameter `name`, if the request carries it once.
pub(super) fn param<'a>(
    request: &'a DecideRequest,
    name: &str,
) -> Result<Option<&'a TypedValue>, String> {
    let mut found = request.params.iter().filter(|p| p.name == name);
    let first = found.next();
    if found.next().is_some() {
        return Err(invalid(format!("parameter {name} is given more than once")));
    }
    Ok(first.map(|p| &p.value))
}

/// The integer parameter `name`, if present; any other type refuses.
pub(super) fn int_param(request: &DecideRequest, name: &str) -> Result<Option<i64>, String> {
    match param(request, name)? {
        None => Ok(None),
        Some(TypedValue::Int(v)) => Ok(Some(*v)),
        Some(_) => Err(invalid(format!("parameter {name} is not an integer"))),
    }
}

/// The non-negative integer parameter `name`, if present.
pub(super) fn unsigned_param(request: &DecideRequest, name: &str) -> Result<Option<u64>, String> {
    match int_param(request, name)? {
        None => Ok(None),
        Some(v) => u64::try_from(v)
            .map(Some)
            .map_err(|_| invalid(format!("parameter {name} must not be negative"))),
    }
}

/// The required JSON text parameter `name`, decoded into `T`.
pub(super) fn json_param<T: serde::de::DeserializeOwned>(
    request: &DecideRequest,
    name: &str,
) -> Result<T, String> {
    let Some(value) = param(request, name)? else {
        return Err(invalid(format!("parameter {name} is required")));
    };
    let TypedValue::Text(text) = value else {
        return Err(invalid(format!("parameter {name} is not JSON text")));
    };
    serde_json::from_str(text).map_err(|e| invalid(format!("parameter {name}: {e}")))
}

/// The family declares exactly one option, named `id`.
pub(super) fn single_option(options: &[DeclaredOption], id: &str) -> Result<(), String> {
    match options {
        [only] if only.option_id == id => Ok(()),
        _ => Err(invalid(format!(
            "this question declares exactly one option, `{id}`"
        ))),
    }
}

/// An advisory answer naming each of `ids` as chosen. Nothing is written.
pub(in crate::decide) fn advisory(ids: Vec<String>) -> Result<Executed, String> {
    let scores: Vec<ScoredOption> = ids
        .into_iter()
        .map(|option_id| ScoredOption {
            option_id,
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: CHOSEN_SCORE,
            },
            probability: None,
        })
        .collect();
    let scores = BoundedVec::new(scores)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
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
        thought: None,
        family_premises: Vec::new(),
        sealed_premises: Vec::new(),
        context_selection: None,
        route: None,
        answered: None,
    })
}

/// An abstention that reports `refusal` under `kind`'s registered table.
pub(super) fn abstained(
    kind: medulla_decision_types::decision::statistical::QuestionKind,
    refusal: &impl medulla_decision_types::decision::record::outcome::DecisionRefusal,
) -> Executed {
    use medulla_decision_types::decision::record::outcome::RefusalOrigin;
    use medulla_decision_types::decision::AbstainReason;
    let reason = AbstainReason::from_refusal(RefusalOrigin::Decide(kind), refusal);
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome: StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![reason]).expect("one reason is inside the bound"),
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
        thought: None,
        family_premises: Vec::new(),
        sealed_premises: Vec::new(),
        context_selection: None,
        route: None,
        answered: None,
    }
}

/// An abstention that reports every one of `refusals` under `kind`'s registered
/// table, in the order given; `refusals` is never empty.
pub(super) fn abstained_all<R>(
    kind: medulla_decision_types::decision::statistical::QuestionKind,
    refusals: &[R],
) -> Result<Executed, String>
where
    R: medulla_decision_types::decision::record::outcome::DecisionRefusal,
{
    use medulla_decision_types::decision::record::outcome::RefusalOrigin;
    use medulla_decision_types::decision::AbstainReason;
    let reasons: Vec<AbstainReason> = refusals
        .iter()
        .map(|refusal| AbstainReason::from_refusal(RefusalOrigin::Decide(kind), refusal))
        .collect();
    let reasons = BoundedVec::new(reasons)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
    let mut executed = abstained(kind, &refusals[0]);
    executed.ladder.outcome = StatisticalOutcome::Abstained { reasons };
    Ok(executed)
}

/// The text parameter `name`, if present.
pub(super) fn text_param<'a>(
    request: &'a DecideRequest,
    name: &str,
) -> Result<Option<&'a str>, String> {
    match param(request, name)? {
        None => Ok(None),
        Some(TypedValue::Text(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(invalid(format!("parameter {name} is not text"))),
    }
}
