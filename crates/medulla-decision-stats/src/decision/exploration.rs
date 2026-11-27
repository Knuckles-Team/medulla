//! Exploration and audit sampling with exact propensities (EH-020, EH-026).
//!
//! Exploration is OFF unless the policy declares a budget that names the
//! question, and it is REFUSED -- not silently skipped -- for a security,
//! policy, write-back or irreversible question the budget names. Randomisation
//! happens only inside the legal set the earlier steps left, every draw comes
//! from the keyed seed, and the executed option's probability under the
//! randomised policy is recorded as an exact rational: `(1-f) + f/n` for the
//! greedy option, `f/n` for any other, with `f = a/b` the budget fraction.

use medulla_decision_types::decision::statistical::keyed::uniform_draw;
use medulla_decision_types::{
    decision::statistical::AuditDraw, decision::statistical::QuestionSafety,
    decision::statistical::StatisticalQuestion,
};
use medulla_decision_types::{
    decision::ColdStart, decision::DecisionErrorCode, decision::DecisionPolicy,
    decision::UnitRationalWire,
};

use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_decision_types::decision::statistical::CandidatePool;

use super::quant::exact_wire;

const DAY_MS: u64 = 86_400_000;

/// The knobs of the context-matched exploitation pool
/// (MEDULLA-DECISION-ENGINE-R020.1.1).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "exploration.match_window",
        unit: "ms",
        default: KnobValue::DurationMs(30 * DAY_MS),
        min: KnobValue::DurationMs(DAY_MS),
        max: KnobValue::DurationMs(365 * DAY_MS),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "exploration.max_matched",
        unit: "records",
        default: KnobValue::Integer(256),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(512),
        safety: KnobSafety::Tunable,
    },
];
use {
    medulla_decision_stats_records::decision::refusal::Refusal,
    medulla_decision_stats_records::decision::refusal::RefusalResult,
};

/// Whether this decision may explore, and how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorationPermit {
    Off,
    Budget(UnitRationalWire),
}

/// The exploration the policy permits for `question`.
pub fn permit(
    policy: &DecisionPolicy,
    question: &StatisticalQuestion,
) -> RefusalResult<ExplorationPermit> {
    let ColdStart::Explore { budget } = &policy.cold_start else {
        return Ok(ExplorationPermit::Off);
    };
    if !budget.questions.iter().any(|q| q == &question.question_id) {
        return Ok(ExplorationPermit::Off);
    }
    if question.safety != QuestionSafety::Ordinary {
        return Err(Refusal::decision(
            DecisionErrorCode::ExplorationForbidden,
            format!(
                "question {} is {:?}: exploration is never permitted for security, policy, \
                 write-back or irreversible questions",
                question.question_id, question.safety
            ),
        ));
    }
    if budget.fraction.numerator() == 0 {
        return Ok(ExplorationPermit::Off);
    }
    Ok(ExplorationPermit::Budget(budget.fraction))
}

/// Split the visible options by what earlier decisions in the same context
/// executed (MEDULLA-DECISION-ENGINE-R020.1.2). `matched` pairs each matched
/// record id with the option it executed. The exploitation pool holds every
/// visible option executed in a matched record; the exploration pool holds the
/// rest. Both are sorted by option id ascending, and an option executed in
/// several records appears once. A matched record whose option is no longer
/// visible joins no pool, but stays in `matched_records`.
pub fn build_pool(
    context_digest: &str,
    visible: &[String],
    matched: &[(String, String)],
) -> CandidatePool {
    let executed: std::collections::BTreeSet<&str> =
        matched.iter().map(|(_, option)| option.as_str()).collect();
    let mut ids: Vec<&String> = visible.iter().collect();
    ids.sort();
    ids.dedup();
    let (exploitation, exploration): (Vec<&String>, Vec<&String>) = ids
        .into_iter()
        .partition(|id| executed.contains(id.as_str()));
    let mut matched_records: Vec<String> =
        matched.iter().map(|(record, _)| record.clone()).collect();
    matched_records.sort();
    matched_records.dedup();
    CandidatePool {
        context_digest: context_digest.to_string(),
        exploitation: exploitation.into_iter().cloned().collect(),
        exploration: exploration.into_iter().cloned().collect(),
        matched_records,
    }
}

/// What the randomised policy executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorationPlan {
    /// True when the exploration branch chose, not the greedy one.
    pub explored: bool,
    pub chosen: usize,
    /// The executed policy's exact probability of `chosen`.
    pub propensity: UnitRationalWire,
}

fn reduced(numerator: u128, denominator: u128) -> RefusalResult<UnitRationalWire> {
    let (mut a, mut b) = (numerator, denominator);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let divisor = a.max(1);
    let (n, d) = (numerator / divisor, denominator / divisor);
    let too_fine = || {
        Refusal::decision(
            DecisionErrorCode::PolicyLoosening,
            "the exploration fraction is too fine to record an exact propensity",
        )
    };
    exact_wire(
        u64::try_from(n).map_err(|_| too_fine())?,
        u64::try_from(d).map_err(|_| too_fine())?,
    )
}

/// Draw the exploration branch for a legal set of `set_len` options whose
/// greedy choice is `greedy` (`None` when there is no score to be greedy on).
/// `None` means the policy did not act: no exploration, and nothing greedy.
pub fn plan(
    seed: &[u8; 32],
    fraction: UnitRationalWire,
    set_len: usize,
    greedy: Option<usize>,
) -> RefusalResult<Option<ExplorationPlan>> {
    if set_len == 0 {
        return Ok(None);
    }
    let (a, b, n) = (
        u128::from(fraction.numerator()),
        u128::from(fraction.denominator()),
        set_len as u128,
    );
    let explored = u128::from(uniform_draw(seed, "explore", fraction.denominator())) < a;
    let chosen = match (explored, greedy) {
        (true, _) => uniform_draw(seed, "option", set_len as u64) as usize,
        (false, Some(g)) => g,
        (false, None) => return Ok(None),
    };
    let numerator = if Some(chosen) == greedy {
        n * (b - a) + a
    } else {
        a
    };
    Ok(Some(ExplorationPlan {
        explored,
        chosen,
        propensity: reduced(numerator, n * b)?,
    }))
}

/// The audit-sampling draw of an acted decision at the policy's rate.
pub fn audit(seed: &[u8; 32], rate: UnitRationalWire) -> AuditDraw {
    let draw = uniform_draw(seed, "audit", rate.denominator());
    AuditDraw {
        inclusion_probability: rate,
        sampled: draw < rate.numerator(),
    }
}

/// The executed policy's full distribution over a legal set of `set_len`
/// options, conditional on an option being executed: one-hot on `greedy`
/// without a budget; `(1-f) + f/n` on `greedy` and `f/n` elsewhere with one;
/// uniform when there is no greedy option (the exploration branch alone
/// executes). Every vector sums to exactly one.
pub fn logging_vector(
    set_len: usize,
    greedy: Option<usize>,
    fraction: Option<UnitRationalWire>,
) -> RefusalResult<Vec<UnitRationalWire>> {
    let n = set_len as u128;
    (0..set_len)
        .map(|i| match (greedy, fraction) {
            (Some(g), None) => reduced(u128::from(i == g), 1),
            (Some(g), Some(f)) => {
                let (a, b) = (u128::from(f.numerator()), u128::from(f.denominator()));
                let numerator = if i == g { n * (b - a) + a } else { a };
                reduced(numerator, n * b)
            }
            (None, _) => reduced(1, n),
        })
        .collect()
}
