//! Refusal codes of the participant memory family
//! (MEDULLA-DECISION-ENGINE-R010.100.4).
//!
//! Collected by `medulla_decision_types::decision::codes::module_tables` and the
//! uniqueness test, so no code here repeats a shared or foreign code.

use crate::refusal_code::RefusalCode;

/// Every refusal code of the participant memory family.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("PARTICIPANT_KIND_INVALID"),
    RefusalCode::new("MEMBERSHIP_SESSION_UNKNOWN"),
    RefusalCode::new("PARTICIPANT_NOT_VISIBLE"),
    RefusalCode::new("OBSERVER_NOT_PERMITTED"),
    RefusalCode::new("SCOPE_NOT_FOUND"),
    RefusalCode::new("SCOPE_NOT_VISIBLE"),
    RefusalCode::new("SCOPE_SESSION_LIMIT"),
    RefusalCode::new("NO_GROUNDED_CLAIMS"),
    RefusalCode::new("CARD_ABSENT"),
    RefusalCode::new("DERIVE_BACKLOG_FULL"),
    RefusalCode::new("DERIVE_ORDER_HELD"),
    RefusalCode::new("PROFILE_NOT_FOUND"),
    RefusalCode::new("PROFILE_NOT_VISIBLE"),
    RefusalCode::new("PROFILE_REVISION_CONFLICT"),
    RefusalCode::new("PROFILE_TOOL_NOT_ALLOWED"),
    RefusalCode::new("DOCUMENT_SET_NOT_FOUND"),
    RefusalCode::new("DOCUMENT_SET_QUERY_NOT_READ_ONLY"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_every_participant_code_once() {
        assert_eq!(REFUSALS.len(), 17);
        let mut codes: Vec<&str> = REFUSALS.iter().map(|c| c.as_str()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 17);
    }
}
