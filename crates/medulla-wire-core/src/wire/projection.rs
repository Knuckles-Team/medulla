//! Read-only graph-backed projections on the SQL wire (MEDULLA-UNIFIED-DATA-PLANE-R026.4.2).
//!
//! A session that names a mapping in the startup parameter `medulla.mapping`
//! (`id@revision`) is a projection session: every statement that is not a read is
//! refused with SQLSTATE `25006` and the engine code `PROJECTION_READ_ONLY`, before any
//! access check, so a separate graph mutation grant never makes a projection a writer.
//! Graph changes reach a projection only through the graph `MutationBatch` path.

use medulla_sql_lang::sql::classify::StatementKind;

use super::WireError;

/// The SQLSTATE of a write on a read-only projection (`25006` read_only_sql_transaction).
pub const PROJECTION_READ_ONLY_SQLSTATE: &str = "25006";
/// The engine code carried in the message of the refusal.
pub const PROJECTION_READ_ONLY_CODE: &str = "PROJECTION_READ_ONLY";
/// The startup parameter that names the mapping of a session.
pub const MAPPING_STARTUP_PARAMETER: &str = "medulla.mapping";

/// The refusal of a write on a projected table.
pub fn projection_read_only_error(mapping: &str) -> WireError {
    WireError {
        code: PROJECTION_READ_ONLY_SQLSTATE.to_owned(),
        message: format!(
            "{PROJECTION_READ_ONLY_CODE}: the session reads mapping `{mapping}`; a projection is \
             read only, change the graph through a MutationBatch"
        ),
    }
}

/// Whether a statement kind reads only.
pub fn is_read_kind(kind: &StatementKind) -> bool {
    matches!(
        kind,
        StatementKind::Read | StatementKind::GraphTableReadRequiresCatalogAdmission(_)
    )
}

/// The refusal for `kind` when the session names `bound_mapping`, `None` when the
/// session is not a projection session or the statement reads. Pure: it never
/// inspects grants or principals.
pub fn refuse_projection_write(
    kind: &StatementKind,
    bound_mapping: Option<&str>,
) -> Option<WireError> {
    let mapping = bound_mapping?;
    (!is_read_kind(kind)).then(|| projection_read_only_error(mapping))
}

/// The mapping named by a startup `options` value (`-c medulla.mapping=id@revision`),
/// the form drivers that cannot send arbitrary startup parameters use.
pub fn mapping_from_options(options: &str) -> Option<String> {
    let mut words = options.split_whitespace();
    while let Some(word) = words.next() {
        let setting = if word == "-c" {
            words.next()?
        } else {
            word.strip_prefix("-c").unwrap_or(word)
        };
        if let Some(value) = setting.strip_prefix(&format!("{MAPPING_STARTUP_PARAMETER}=")) {
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_sql_lang::sql::classify::classify;

    fn kind(sql: &str) -> StatementKind {
        classify(sql).expect("classifies")
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R026.4.2
    #[test]
    fn every_write_kind_is_refused_when_a_mapping_is_bound() {
        for sql in [
            "INSERT INTO people (id, name) VALUES (1, 'a')",
            "UPDATE people SET name = 'b' WHERE id = 1",
            "DELETE FROM people WHERE id = 1",
            "CREATE TABLE extra (id INT)",
            "DROP TABLE people",
        ] {
            let refusal = refuse_projection_write(&kind(sql), Some("gramps@3")).expect("refused");
            assert_eq!(refusal.code, "25006", "{sql}");
            assert!(refusal.message.starts_with("PROJECTION_READ_ONLY"), "{sql}");
        }
    }

    #[test]
    fn reads_and_unbound_sessions_are_not_refused() {
        assert!(refuse_projection_write(&kind("SELECT 1"), Some("gramps@3")).is_none());
        // A write the classifier accepts (an unscoped DELETE is refused before
        // any session check) is still not refused when no mapping is bound.
        assert!(refuse_projection_write(&kind("DELETE FROM people WHERE id = 1"), None).is_none());
    }

    #[test]
    fn the_mapping_is_read_from_the_options_value() {
        assert_eq!(
            mapping_from_options("-c medulla.mapping=gramps@3").as_deref(),
            Some("gramps@3")
        );
        assert_eq!(
            mapping_from_options("-c search_path=x -cmedulla.mapping=m@1").as_deref(),
            Some("m@1")
        );
        assert_eq!(mapping_from_options("-c search_path=x"), None);
    }
}
