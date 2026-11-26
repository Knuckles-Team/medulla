//! The refusal codes of the served program optimization job
//! (MEDULLA-DECISION-ENGINE-R810.4, R804.4).
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

use super::record::outcome::RefusalCode;

pub use super::codes::EVAL_SET_NOT_INDEPENDENT;

/// The corpus has no test-split example, so there is nothing held out to
/// evaluate a program revision against.
pub const PROGRAM_HELDOUT_MISSING: RefusalCode = RefusalCode::new("PROGRAM_HELDOUT_MISSING");
/// The optimizer name is outside the closed optimizer list.
pub const PROGRAM_OPTIMIZER_UNKNOWN: RefusalCode = RefusalCode::new("PROGRAM_OPTIMIZER_UNKNOWN");
/// A budget field is above the optimizer's ceiling.
pub const PROGRAM_BUDGET_EXCEEDED: RefusalCode = RefusalCode::new("PROGRAM_BUDGET_EXCEEDED");

/// The codes only the program job declares. [`EVAL_SET_NOT_INDEPENDENT`] is the
/// promotion table's; the job reuses it.
pub const REFUSALS: &[RefusalCode] = &[
    PROGRAM_HELDOUT_MISSING,
    PROGRAM_OPTIMIZER_UNKNOWN,
    PROGRAM_BUDGET_EXCEEDED,
];

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R810.4
    #[test]
    fn program_codes_are_registered_and_do_not_repeat_the_promotion_code() {
        for code in REFUSALS {
            assert!(
                super::super::codes::resolve(code.as_str()).is_some(),
                "{} is not registered",
                code.as_str()
            );
        }
        assert!(!REFUSALS.contains(&EVAL_SET_NOT_INDEPENDENT));
        assert!(super::super::codes::resolve(EVAL_SET_NOT_INDEPENDENT.as_str()).is_some());
    }
}
