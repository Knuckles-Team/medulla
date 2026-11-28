//! The refusal codes of the served program optimization job
//! (MEDULLA-DECISION-ENGINE-R810.4, R804.4, R813.4, R800.4).
//!
//! `submit_job_program_optimize` (`crates/medulla-h-jobs/src/jobs/submit.rs`)
//! rejects an invalid request with one of these codes before a job is stored,
//! so a refusal stores no job and no revision. A refusal renders as
//! `<CODE>: <detail>`.
//!
//! A test split whose graders cannot decide a promotion refuses with the shared
//! promotion code [`EVAL_SET_NOT_INDEPENDENT`]. That code is registered once, in
//! the promotion table; this module re-exports it so the program job names it
//! beside its own codes, and does not list it a second time in [`REFUSALS`].

use medulla_types::refusal_code::RefusalCode;

pub use medulla_decision_statistical_types::decision::codes::EVAL_SET_NOT_INDEPENDENT;

/// The corpus has no test-split example, so there is nothing held out to
/// evaluate a program revision against.
pub const PROGRAM_HELDOUT_MISSING: RefusalCode = RefusalCode::new("PROGRAM_HELDOUT_MISSING");
/// The optimizer name is outside the closed optimizer list.
pub const PROGRAM_OPTIMIZER_UNKNOWN: RefusalCode = RefusalCode::new("PROGRAM_OPTIMIZER_UNKNOWN");
/// A budget field is above the optimizer's ceiling.
pub const PROGRAM_BUDGET_EXCEEDED: RefusalCode = RefusalCode::new("PROGRAM_BUDGET_EXCEEDED");

/// A reflective job's test split holds fewer examples than
/// `knob front.min_eval_examples` (MEDULLA-DECISION-ENGINE-R800.4).
pub const FRONT_EVAL_TOO_SMALL: RefusalCode = RefusalCode::new("FRONT_EVAL_TOO_SMALL");
/// A reflective job's evaluation carries no `benchmark_score`, so it cannot
/// place the candidate on both objectives (MEDULLA-DECISION-ENGINE-R800.4).
pub const FRONT_SCORE_LENGTH: RefusalCode = RefusalCode::new("FRONT_SCORE_LENGTH");
/// A stored `:Score` names a grader kind outside the five kinds
/// (MEDULLA-DECISION-ENGINE-R813.4).
pub const SCORE_GRADER_UNKNOWN: RefusalCode = RefusalCode::new("SCORE_GRADER_UNKNOWN");
/// A stored `:Score` has a value outside `[range_min, range_max]`, no value or
/// range, or a `range_max` that is not above `range_min`
/// (MEDULLA-DECISION-ENGINE-R813.4).
pub const SCORE_OUT_OF_RANGE: RefusalCode = RefusalCode::new("SCORE_OUT_OF_RANGE");
/// A stored `:Score` does not decode or names an unknown split
/// (MEDULLA-DECISION-ENGINE-R813.4).
pub const SCORE_INVALID: RefusalCode = RefusalCode::new("SCORE_INVALID");
/// The graph holds more scores than one corpus carries
/// (MEDULLA-DECISION-ENGINE-R813.4).
pub const SCORE_CORPUS_TOO_LARGE: RefusalCode = RefusalCode::new("SCORE_CORPUS_TOO_LARGE");

/// The codes only the program job declares. [`EVAL_SET_NOT_INDEPENDENT`] is the
/// promotion table's; the job reuses it.
pub const REFUSALS: &[RefusalCode] = &[
    PROGRAM_HELDOUT_MISSING,
    PROGRAM_OPTIMIZER_UNKNOWN,
    PROGRAM_BUDGET_EXCEEDED,
    FRONT_EVAL_TOO_SMALL,
    FRONT_SCORE_LENGTH,
    SCORE_GRADER_UNKNOWN,
    SCORE_OUT_OF_RANGE,
    SCORE_INVALID,
    SCORE_CORPUS_TOO_LARGE,
];

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R810.4
    #[test]
    fn program_codes_are_registered_and_do_not_repeat_the_promotion_code() {
        for code in REFUSALS {
            assert!(
                super::super::code_registry::resolve(code.as_str()).is_some(),
                "{} is not registered",
                code.as_str()
            );
        }
        assert!(!REFUSALS.contains(&EVAL_SET_NOT_INDEPENDENT));
        assert!(super::super::code_registry::resolve(EVAL_SET_NOT_INDEPENDENT.as_str()).is_some());
    }
}
