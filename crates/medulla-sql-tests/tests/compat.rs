//! SQL dialect compatibility matrix (MEDULLA-ENGINE-PARITY-R009.1): every
//! `StatementKind` has an entry, and the matrix is closed (an absent form
//! resolves to `Refused`), and a form outside it is refused with
//! `FEATURE_NOT_SUPPORTED` (MEDULLA-ENGINE-PARITY-R009.2).

#![cfg(feature = "sql")]

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphCore;
use medulla_mutation_types::pg_operational_surface::{
    PgCompatibilityMatrix, SqlFormKind, SqlFormState,
};
use medulla_sql::sql::exec_sql_typed_with_tables;
use medulla_sql_lang::sql::classify::{classify, StatementKind};

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
    assert_eq!(matrix.form_state("create_policy"), None);
    assert_eq!(matrix.resolve_form("create_policy"), SqlFormState::Refused);
    assert_eq!(
        PgCompatibilityMatrix::fail_closed().resolve_form("read"),
        SqlFormState::Refused
    );
}

/// One statement the classifier serves for every statement form the matrix lists as
/// `Supported` or `Partial`. Adding a form to the matrix without a sample here fails
/// `every_supported_entry_has_a_test`.
const STATEMENT_SAMPLES: &[(&str, &str)] = &[
    ("read", "SELECT 1"),
    ("insert_nodes", "INSERT INTO nodes (id, type) VALUES ('n1', 'Agent')"),
    ("insert_nodes_select", "INSERT INTO nodes (id, rank) SELECT sku, px FROM prices"),
    ("update_nodes", "UPDATE nodes SET rank = 5 WHERE id = 'n1'"),
    (
        "update_nodes_join",
        "UPDATE nodes SET rank = p.px FROM prices p WHERE nodes.symbol = p.sku",
    ),
    ("delete_nodes", "DELETE FROM nodes WHERE id = 'n1'"),
    (
        "delete_nodes_join",
        "DELETE FROM nodes USING prices p WHERE nodes.symbol = p.sku",
    ),
    ("create_table", "CREATE TABLE t (a INT)"),
    ("drop_table", "DROP TABLE t"),
    ("alter_table", "ALTER TABLE t ADD COLUMN b INT"),
    ("create_view", "CREATE VIEW v AS SELECT 1"),
    ("drop_view", "DROP VIEW v"),
    ("insert_table", "INSERT INTO t (a) VALUES (1)"),
    ("insert_select", "INSERT INTO t (a) SELECT a FROM u"),
    ("update_table", "UPDATE t SET a = 1 WHERE a = 2"),
    ("delete_table", "DELETE FROM t WHERE a = 1"),
    (
        "property_graph_ddl",
        "CREATE PROPERTY GRAPH shop VERTEX TABLES (customers KEY (customer_id))",
    ),
    (
        "property_graph_privilege",
        "GRANT SELECT ON PROPERTY GRAPH shop TO reader",
    ),
    (
        "graph_table_read",
        "SELECT * FROM GRAPH_TABLE (shop MATCH (c:customer) COLUMNS (c.name))",
    ),
    ("begin", "BEGIN"),
    ("commit", "COMMIT"),
    ("rollback", "ROLLBACK"),
    ("discard_all", "DISCARD ALL"),
    ("copy_in", "COPY prices FROM STDIN"),
    ("copy_out", "COPY prices TO STDOUT"),
    ("create_extension", "CREATE EXTENSION vector"),
    ("drop_extension", "DROP EXTENSION vector"),
    (
        "create_trigger",
        "CREATE TRIGGER audit AFTER INSERT ON t EXECUTE MACHINE m1",
    ),
    ("drop_trigger", "DROP TRIGGER audit ON t"),
    (
        "create_ann_index",
        "CREATE INDEX ON docs USING hnsw (emb vector_l2_ops)",
    ),
    ("create_index", "CREATE INDEX ON t (a)"),
    ("drop_index", "DROP INDEX i"),
    ("create_sequence", "CREATE SEQUENCE s START 1"),
    ("drop_sequence", "DROP SEQUENCE s"),
    ("create_hypertable", "SELECT create_hypertable('t', 'ts')"),
    (
        "create_continuous_aggregate",
        "CREATE MATERIALIZED VIEW daily WITH (timescaledb.continuous) AS SELECT time_bucket('1 day', ts) AS d, count(*) FROM t GROUP BY 1",
    ),
    ("create_matview", "CREATE MATERIALIZED VIEW mv AS SELECT a FROM t"),
    ("refresh_matview", "REFRESH MATERIALIZED VIEW mv"),
    ("drop_matview", "DROP MATERIALIZED VIEW mv"),
    (
        "create_function",
        "CREATE FUNCTION add_two(a int, b int) RETURNS int AS $$ SELECT a + b $$ LANGUAGE sql",
    ),
    ("drop_function", "DROP FUNCTION add_two"),
];

/// One call per served function. The first value is the matrix entry name; the
/// last says whether the call also runs against an empty graph (those whose
/// argument shapes the engine's own tests already pin).
const FUNCTION_SAMPLES: &[(&str, &str, bool)] = &[
    ("json_get", "SELECT json_get(props, 'k') FROM nodes", false),
    (
        "json_get_f64",
        "SELECT json_get_f64(props, 'x') FROM nodes",
        false,
    ),
    (
        "json_get_i64",
        "SELECT json_get_i64(props, 'ts') FROM nodes",
        false,
    ),
    (
        "time_bucket",
        "SELECT time_bucket('1 day', ts) FROM t",
        false,
    ),
    ("greatest", "SELECT greatest(1, 5, 3)", true),
    ("least", "SELECT least(4, 2, 9)", true),
    ("md5", "SELECT md5('a')", false),
    ("sha1", "SELECT sha1('a')", false),
    ("sha256", "SELECT sha256('a')", false),
    ("base64_encode", "SELECT base64_encode('a')", false),
    ("base64_decode", "SELECT base64_decode('YQ==')", false),
    ("int4range", "SELECT int4range(1, 5)", true),
    ("tsrange", "SELECT tsrange(1000, 2000)", true),
];

// spec: MEDULLA-ENGINE-PARITY-R009.2
#[test]
fn every_supported_entry_has_a_test() {
    let matrix = PgCompatibilityMatrix::with_served_forms();
    let served: BTreeSet<(SqlFormKind, String)> = matrix
        .forms
        .iter()
        .filter(|entry| entry.state != SqlFormState::Refused)
        .map(|entry| (entry.kind, entry.name.clone()))
        .collect();
    let tested: BTreeSet<(SqlFormKind, String)> = STATEMENT_SAMPLES
        .iter()
        .map(|(name, _)| (SqlFormKind::Statement, (*name).to_string()))
        .chain(
            FUNCTION_SAMPLES
                .iter()
                .map(|(name, _, _)| (SqlFormKind::Function, (*name).to_string())),
        )
        .collect();
    assert_eq!(
        served, tested,
        "every Supported or Partial matrix entry has exactly one sample, and no sample names an unserved form"
    );

    // Each statement sample classifies to the very form it is filed under.
    for (name, sql) in STATEMENT_SAMPLES {
        let kind = classify(sql).unwrap_or_else(|error| panic!("`{sql}` must classify: {error}"));
        assert_eq!(kind.form_name(), *name, "`{sql}`");
    }
    // Each function sample is a read, and the ones with pinned shapes run.
    let view = GraphCore::new().analysis_snapshot();
    let store = medulla_table_store::tables::TableStore::open_scoped(
        std::env::temp_dir().join(format!(
            "medulla_compat_{}_{}.redb",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
        "tenant-a",
        medulla_table_store::tables::store::dev_scope_grant::dev_verifier(),
        medulla_table_store::tables::store::dev_scope_grant::DEV_PRINCIPAL,
        medulla_table_store::tables::store::dev_scope_grant::DEV_PROOF,
    )
    .unwrap();
    for (name, sql, runs) in FUNCTION_SAMPLES {
        assert_eq!(classify(sql).unwrap().form_name(), "read", "{name}");
        if *runs {
            let result = exec_sql_typed_with_tables(&view, &store, sql)
                .unwrap_or_else(|error| panic!("`{sql}` must run: {error}"));
            assert_eq!(result.rows.len(), 1, "{name}");
        }
    }
}

// spec: MEDULLA-ENGINE-PARITY-R009.2
#[test]
fn a_form_outside_the_matrix_is_refused_with_feature_not_supported() {
    // The PostgreSQL `EXECUTE FUNCTION` trigger is not this engine's trigger (a trigger
    // runs a stored machine): the form is served, this shape is refused.
    let function_trigger =
        classify("CREATE TRIGGER t BEFORE INSERT ON a FOR EACH ROW EXECUTE FUNCTION f()")
            .unwrap_err();
    assert!(
        function_trigger
            .starts_with("FEATURE_NOT_SUPPORTED: create_trigger: unsupported statement shape"),
        "{function_trigger}"
    );
    for (sql, form) in [
        ("CREATE POLICY p ON a USING (true)", "create_policy"),
        ("DROP POLICY p ON a", "drop_policy"),
    ] {
        let error = classify(sql).unwrap_err();
        assert!(
            error.starts_with(&format!("FEATURE_NOT_SUPPORTED: {form} is not supported")),
            "{sql}: {error}"
        );
    }
    // A syntax error is still a syntax error, not a refused form.
    assert!(!classify("SELEC 1")
        .unwrap_err()
        .starts_with("FEATURE_NOT_SUPPORTED"));
}
