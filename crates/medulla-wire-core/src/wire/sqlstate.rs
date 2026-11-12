//! The one SQLSTATE table (MEDULLA-ENGINE-PARITY-R006.1).
//!
//! A store or engine error string that starts with `<CODE>: ` carries a typed
//! engine code. The table maps each code to the SQLSTATE a client sees. It is
//! closed: a code that is not listed keeps the system-error state `58000`.

/// The system-error SQLSTATE an unknown code keeps.
pub const SYSTEM_ERROR: &str = "58000";

/// Every `(engine code, SQLSTATE)` pair, in one place.
const TABLE: &[(&str, &str)] = &[
    ("TXN_CONFLICT", "40001"),
    ("TXN_SNAPSHOT_EXPIRED", "40001"),
    ("TXN_RETRY_EXHAUSTED", "40001"),
    ("COMMIT_QUEUE_FULL", "53000"),
    ("UNIQUE_VIOLATION", "23505"),
    ("FOREIGN_KEY_VIOLATION", "23503"),
    ("CHECK_VIOLATION", "23514"),
    ("SHACL_VIOLATION", "23514"),
    ("NOT_NULL_VIOLATION", "23502"),
    ("FEATURE_NOT_SUPPORTED", "0A000"),
    ("INVALID_LOGIN", "28000"),
    ("INDEX_EXPR_NONDETERMINISTIC", "42P17"),
    ("SEQUENCE_REWIND", "55000"),
    ("SEQUENCE_UNKNOWN", "42P01"),
    ("SEQUENCE_INVALID", "22023"),
    ("TRIGGER_DEPTH", "54001"),
];

/// The SQLSTATE for an engine code, or `None` when the code is not in the table.
pub fn sqlstate_for(code: &str) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, state)| *state)
}

/// The engine code an error string carries: the text before the first `": "`,
/// when it is a non-empty run of `A-Z`, `0-9` and `_`.
pub fn code_of(message: &str) -> Option<&str> {
    let (code, _) = message.split_once(": ")?;
    let valid = !code.is_empty()
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    valid.then_some(code)
}

/// The SQLSTATE a client sees for an error string: the table entry for the code
/// it carries, else `58000`.
pub fn sqlstate_of_message(message: &str) -> &'static str {
    code_of(message)
        .and_then(sqlstate_for)
        .unwrap_or(SYSTEM_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // spec: MEDULLA-ENGINE-PARITY-R006.1
    #[test]
    fn table_is_closed_and_unique() {
        let mut codes = HashSet::new();
        for (code, state) in TABLE {
            assert!(codes.insert(*code), "duplicate engine code {code}");
            assert_eq!(state.len(), 5, "SQLSTATE of {code} is five characters");
            assert!(
                state
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
                "SQLSTATE of {code} is upper-case alphanumeric"
            );
            assert_ne!(*state, SYSTEM_ERROR, "{code} must not map to the fallback");
        }
        assert_eq!(sqlstate_for("TXN_CONFLICT"), Some("40001"));
        assert_eq!(sqlstate_for("COMMIT_QUEUE_FULL"), Some("53000"));
        assert_eq!(sqlstate_for("UNIQUE_VIOLATION"), Some("23505"));
        assert_eq!(sqlstate_for("NOT_A_CODE"), None);
    }

    // spec: MEDULLA-ENGINE-PARITY-R006.1
    #[test]
    fn prefix_carries_the_code() {
        assert_eq!(sqlstate_of_message("TXN_CONFLICT: key moved"), "40001");
        assert_eq!(sqlstate_of_message("COMMIT_QUEUE_FULL: retry"), "53000");
        assert_eq!(sqlstate_of_message("SEQUENCE_REWIND: setval(1)"), "55000");
        assert_eq!(sqlstate_of_message("UNKNOWN_CODE: x"), "58000");
        assert_eq!(sqlstate_of_message("plain failure"), "58000");
        assert_eq!(sqlstate_of_message("lower: case"), "58000");
        assert_eq!(code_of("A_1: rest"), Some("A_1"));
        assert_eq!(code_of(": rest"), None);
    }
}
