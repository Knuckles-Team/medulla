//! Tests of the Postgres-family extension planners (they run SQL through the executor).

use medulla_sql_lang::sql::pgfamily::*;
use datafusion::sql::sqlparser::dialect::PostgreSqlDialect;
use datafusion::sql::sqlparser::parser::Parser;
use medulla_table_store::tables::ann_plan::{AnnIndexPlan, AnnMethod, VectorMetric};
use serde_json::Value;

// ── MEDULLA-116: pgvector ANN pushdown (the CREATE INDEX parser lives in
// `pgvector_ddl`, with its own tests) ──────────────────────────────────────
#[test]
fn eg116_ann_pushdown_chooses_ann_when_index_present() {
    let indexes = vec![AnnIndexPlan {
        name: None,
        table: "items".into(),
        column: "embedding".into(),
        method: AnnMethod::Hnsw,
        metric: VectorMetric::L2,
        if_not_exists: false,
    }];
    let sql = "SELECT id FROM items ORDER BY embedding <-> '[1,2,3]' LIMIT 5";
    let plan = plan_ann_search(sql, &indexes).expect("chooses ANN");
    assert_eq!(plan.table, "items");
    assert_eq!(plan.column, "embedding");
    assert_eq!(plan.metric, VectorMetric::L2);
    assert_eq!(plan.k, 5);
    assert_eq!(plan.query, "'[1,2,3]'");
}

#[test]
fn eg116_ann_pushdown_falls_back_without_index() {
    // No registered index ⇒ do NOT choose ANN (brute-force MEDULLA-115 path applies).
    let sql = "SELECT id FROM items ORDER BY embedding <-> '[1,2,3]' LIMIT 5";
    assert!(plan_ann_search(sql, &[]).is_none());
}

// ── MEDULLA-117: TimescaleDB hypertable + continuous aggregate ──────────────────
#[test]
fn eg117_detect_create_hypertable() {
    let stmts = Parser::parse_sql(
        &PostgreSqlDialect {},
        "SELECT create_hypertable('conditions', 'ts')",
    )
    .unwrap();
    let plan = detect_create_hypertable(&stmts[0]).expect("recognized");
    assert_eq!(plan.table, "conditions");
    assert_eq!(plan.time_column, "ts");
}

#[test]
fn eg117_plain_select_is_not_a_hypertable() {
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, "SELECT 1").unwrap();
    assert!(detect_create_hypertable(&stmts[0]).is_none());
}

#[test]
fn eg117_parse_continuous_aggregate() {
    let plan = parse_continuous_aggregate(
        "CREATE MATERIALIZED VIEW cagg WITH (timescaledb.continuous) AS \
         SELECT time_bucket('1 hour', ts) AS bucket, avg(v) FROM conditions GROUP BY bucket",
    )
    .expect("recognized");
    assert_eq!(plan.name, "cagg");
    assert!(plan
        .select_sql
        .to_ascii_lowercase()
        .starts_with("select time_bucket"));
    assert!(!plan.if_not_exists);
}

#[test]
fn eg117_plain_materialized_view_is_not_continuous() {
    assert!(parse_continuous_aggregate(
        "CREATE MATERIALIZED VIEW v AS SELECT * FROM conditions"
    )
    .is_none());
}

// ── MEDULLA-119: ParadeDB BM25 @@@ search ───────────────────────────────────────
#[test]
fn eg119_plan_bm25_search_filter_and_limit() {
    let plan =
        plan_bm25_search("SELECT id FROM docs WHERE body @@@ 'quantum computing' LIMIT 10")
            .expect("recognized");
    assert_eq!(plan.table, "docs");
    assert_eq!(plan.column, "body");
    assert_eq!(plan.query, "quantum computing");
    assert_eq!(plan.k, Some(10));
}

#[test]
fn eg119_plan_bm25_search_within_and() {
    let plan = plan_bm25_search("SELECT id FROM docs WHERE lang = 'en' AND body @@@ 'rust'")
        .expect("recognized");
    assert_eq!(plan.column, "body");
    assert_eq!(plan.query, "rust");
    assert_eq!(plan.k, None);
}

#[test]
fn eg119_non_bm25_where_is_not_recognized() {
    assert!(plan_bm25_search("SELECT id FROM docs WHERE lang = 'en'").is_none());
}

// ── MEDULLA-117 + MEDULLA-119: end-to-end basic exec of the new UDFs through DataFusion ──
#[test]
fn eg117_eg119_time_bucket_and_bm25_match_execute() {
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    let schema = Arc::new(Schema::new(vec![
        Field::new("ts", DataType::Int64, false),
        Field::new("body", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![0i64, 3_700, 7_300])),
            Arc::new(StringArray::from(vec![
                "rust is great",
                "go lang",
                "rust rocks",
            ])),
        ],
    )
    .unwrap();

    // CONCEPT:MEDULLA-KG.query.paradedb-bm25 `@@@` desugars to bm25_match (filter); CONCEPT:MEDULLA-KG.query.continuous-aggregate-lowering
    // time_bucket floors the timestamp — both run in one DataFusion plan.
    let out = crate::sql::exec_sql_over_tables(
        vec![("events".to_string(), schema, vec![batch])],
        "SELECT time_bucket(3600, ts) AS bucket FROM events \
         WHERE body @@@ 'rust' ORDER BY bucket",
    )
    .unwrap();
    assert_eq!(out.rows.len(), 2, "only the two 'rust' rows match");
    assert_eq!(out.rows[0][0], Value::from(0i64));
    assert_eq!(out.rows[1][0], Value::from(7_200i64));
}

/// CONCEPT:MEDULLA-KG.query.bm25-ranking-snippets — REAL BM25 ranking + highlighted snippets through DataFusion.
/// The 2-arg `bm25_score(body, 'query')` / `bm25_snippet(body, 'query', n)` forms
/// carry the query into the per-row UDF, so `ORDER BY bm25_score(...) DESC` puts the
/// more-relevant document FIRST (not the constant-1.0 placeholder order) and the
/// snippet wraps matched terms in `<b>…</b>`.
#[test]
fn eg311_bm25_real_score_orders_and_snippet_highlights() {
    use arrow::array::StringArray;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("body", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec!["low", "high"])),
            Arc::new(StringArray::from(vec![
                "the quick brown fox jumps over the lazy dog just once",
                "the lazy dog naps while another lazy dog guards the dog house",
            ])),
        ],
    )
    .unwrap();

    let out = crate::sql::exec_sql_over_tables(
        vec![("docs".to_string(), schema, vec![batch])],
        "SELECT id, bm25_snippet(body, 'lazy dog', 40) AS snip \
         FROM docs WHERE body @@@ 'lazy dog' \
         ORDER BY bm25_score(body, 'lazy dog') DESC",
    )
    .unwrap();

    assert_eq!(out.rows.len(), 2, "both docs contain the terms");
    // The doc with more 'lazy dog' occurrences ranks FIRST — real BM25, not 1.0.
    assert_eq!(
        out.rows[0][0],
        Value::from("high"),
        "more-relevant doc ranked first: {:?}",
        out.rows
    );
    assert_eq!(out.rows[1][0], Value::from("low"));
    // The snippet highlights the matched terms.
    let snip = out.rows[0][1].as_str().unwrap_or_default();
    assert!(
        snip.contains("<b>lazy</b>") || snip.contains("<b>dog</b>"),
        "snippet highlights a matched term: {snip:?}"
    );
}
