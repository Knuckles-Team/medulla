//! The refusals of the `context_budget` question family
//! (MEDULLA-DECISION-ENGINE-R029.2.2).
//!
//! Each code is also a `StatisticalErrorCode`, so a served `Decide` refuses
//! with it. `CONTEXT_BUDGET_MISSING` is the one code `ContextCompile` also
//! answers (a bundle with no window), so both entry points name this constant.

use super::record::outcome::RefusalCode;

/// A source is named twice.
pub const CONTEXT_SOURCE_DUPLICATE: RefusalCode = RefusalCode::new("CONTEXT_SOURCE_DUPLICATE");
/// The question carries no positive token budget.
pub const CONTEXT_BUDGET_MISSING: RefusalCode = RefusalCode::new("CONTEXT_BUDGET_MISSING");
/// A source declares no usable token cost.
pub const CONTEXT_TOKENS_UNKNOWN: RefusalCode = RefusalCode::new("CONTEXT_TOKENS_UNKNOWN");

/// The `REFUSALS` table of the family, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    CONTEXT_SOURCE_DUPLICATE,
    CONTEXT_BUDGET_MISSING,
    CONTEXT_TOKENS_UNKNOWN,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::statistical::StatisticalErrorCode;

    // spec: MEDULLA-DECISION-ENGINE-R029.2.2
    #[test]
    fn declared_refusals_are_registered_statistical_codes() {
        for code in REFUSALS {
            assert_eq!(super::super::codes::resolve(code.as_str()), Some(*code));
            assert!(StatisticalErrorCode::ALL
                .iter()
                .any(|known| known.as_str() == code.as_str()));
        }
    }
}
