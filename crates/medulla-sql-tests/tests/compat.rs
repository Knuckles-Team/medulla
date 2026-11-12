//! SQL dialect compatibility matrix (MEDULLA-ENGINE-PARITY-R009.1): every
//! `StatementKind` has an entry, and the matrix is closed (an absent form
//! resolves to `Refused`).

#![cfg(feature = "sql")]

use medulla_sql_lang::sql::classify::{classify, StatementKind};
use medulla_types::pg_operational_surface::{PgCompatibilityMatrix, SqlFormKind, SqlFormState};

// spec: MEDULLA-ENGINE-PARITY-R009.1
#[test]
fn matrix_loads_and_is_closed() {
    let matrix = PgCompatibilityMatrix::with_served_forms();

    // Every statement variant has an entry (FORM_NAMES is paired with the
    // exhaustive `form_name` match, so a new variant cannot omit its name).
    for name in StatementKind::FORM_NAMES {
        let entry = matrix
            .forms
            .iter()
            .find(|f| f.name == *name)
            .unwrap_or_else(|| panic!("StatementKind form `{name}` has no matrix entry"));
        assert_eq!(entry.kind, SqlFormKind::Statement);
        assert_ne!(entry.state, SqlFormState::Refused);
    }
    // No stale statement entry: the matrix holds exactly the variants.
    let statement_entries = matrix
        .forms
        .iter()
        .filter(|f| f.kind == SqlFormKind::Statement)
        .count();
    assert_eq!(statement_entries, StatementKind::FORM_NAMES.len());

    // A statement the classifier produces resolves through its form name.
    let kind = classify("SELECT 1").expect("classifies");
    assert_eq!(kind.form_name(), "read");
    assert_eq!(
        matrix.form_state(kind.form_name()),
        Some(SqlFormState::Supported)
    );

    // Closed: an unlisted form is Refused, with no entry to point at.
    assert_eq!(matrix.form_state("create_trigger"), None);
    assert_eq!(matrix.resolve_form("create_trigger"), SqlFormState::Refused);
    assert_eq!(
        PgCompatibilityMatrix::fail_closed().resolve_form("read"),
        SqlFormState::Refused
    );
}
