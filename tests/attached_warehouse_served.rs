//! Warehouse and lake sources read through the served path (MEDULLA-UNIFIED-DATA-PLANE-R020.2.2).
//!
//! Each source is registered with `Method::RegisterForeignSource` (a `Warehouse` spec) and read
//! with `Method::QueryForeignColumns`, both through the real `dispatch` surface. The Iceberg
//! table is the partitioned fixture the `iceberg('namespace.table')` test serves from an
//! in-process REST catalog and object store; the DuckDB file is written by the `duckdb` crate and
//! read natively by it for the comparison. A `snowflake` source refuses `CAPABILITY_UNSUPPORTED`
//! and a DuckDB file outside `foreign.duckdb.allowed_roots` refuses `DUCKDB_FILE_DENIED`.
#![cfg(all(
    feature = "federation",
    feature = "source-duckdb",
    feature = "server",
    feature = "security"
))]

mod common;
#[path = "../crates/medulla-sql-tests/tests/iceberg_lake/mod.rs"]
mod iceberg_lake;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::knob_config::{install_allowed_roots, FOREIGN_DUCKDB_ALLOWED_ROOTS};
use medulla_types::warehouse_federation::{WarehouseSourceConfig, WarehouseSourceKind};
use medulla_types::wire::{
    ForeignColumnComparison, ForeignColumnPredicate, ForeignColumnRow, ForeignSourceSpec,
};

use iceberg_lake::{region_rows, write_fixture, Lake, REGIONS};

const SECRET: &str = "medulla-unified-data-plane-r020-2-2-warehouse-secret";

/// The directory `foreign.duckdb.allowed_roots` names for this test process (the knob installs
/// once per process).
fn allowed_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::fs::canonicalize(test_support::fresh_dir("r020-2-2-duckdb-root"))
            .expect("canonical allowed root");
        install_allowed_roots(FOREIGN_DUCKDB_ALLOWED_ROOTS, vec![root.clone()]);
        medulla_plan::warehouse_reader::set_duckdb_allowed_roots(vec![root.clone()]);
        medulla_plan::warehouse_reader::register();
        root
    })
}

fn config(kind: WarehouseSourceKind, fields: &[(&str, &str)]) -> WarehouseSourceConfig {
    WarehouseSourceConfig {
        kind: Some(kind),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn warehouse(config: WarehouseSourceConfig, table: &str, columns: &[&str]) -> ForeignSourceSpec {
    ForeignSourceSpec::Warehouse {
        config,
        table: table.to_string(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        id_column: "id".to_string(),
        prefer_flight_sql: false,
        snapshot_id: None,
    }
}

async fn register(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    spec: ForeignSourceSpec,
) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::RegisterForeignSource {
                name: name.to_string(),
                source: spec,
                quota: None,
            },
        ),
    )
    .await
}

fn predicate(
    column: &str,
    comparison: ForeignColumnComparison,
    value: serde_json::Value,
) -> ForeignColumnPredicate {
    ForeignColumnPredicate {
        column: column.to_string(),
        comparison,
        value,
    }
}

async fn read(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    columns: &[&str],
    predicates: Vec<ForeignColumnPredicate>,
) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::QueryForeignColumns {
                name: name.to_string(),
                columns: columns.iter().map(|c| c.to_string()).collect(),
                predicates,
            },
        ),
    )
    .await
}

/// `(id, columns)` of the rows of an ok read, ordered by id.
fn rows(response: &Response) -> Vec<(String, BTreeMap<String, serde_json::Value>)> {
    assert!(
        response.error.is_none(),
        "read refused: {:?}",
        response.error
    );
    let Some(ResultPayload::Raw(bytes)) = &response.result else {
        panic!("unexpected read payload: {:?}", response.result);
    };
    let mut rows: Vec<ForeignColumnRow> = rmp_serde::from_slice(bytes).expect("msgpack rows");
    rows.sort_by_key(|row| row.id.parse::<i64>().unwrap_or(i64::MAX));
    rows.into_iter().map(|row| (row.id, row.columns)).collect()
}

fn refusal(response: &Response) -> &str {
    response.error.as_deref().unwrap_or_else(|| {
        panic!("expected a refusal, got {:?}", response.result);
    })
}

/// The native Iceberg read: the table scanned by the `iceberg` crate itself, every column,
/// no filter, so the served read is compared with what the engine of record returns.
async fn native_iceberg_rows(table: &iceberg::table::Table) -> Vec<(i64, i64, String)> {
    use arrow::array::{Int64Array, StringArray};
    use futures_util::TryStreamExt;
    let batches: Vec<arrow::record_batch::RecordBatch> = table
        .scan()
        .build()
        .expect("native scan")
        .to_arrow()
        .await
        .expect("native arrow stream")
        .try_collect()
        .await
        .expect("native batches");
    let mut rows = Vec::new();
    for batch in &batches {
        let region = batch.column_by_name("region").unwrap();
        let id = batch.column_by_name("id").unwrap();
        let amount = batch.column_by_name("amount").unwrap();
        let region = region.as_any().downcast_ref::<StringArray>().unwrap();
        let id = id.as_any().downcast_ref::<Int64Array>().unwrap();
        let amount = amount.as_any().downcast_ref::<Int64Array>().unwrap();
        for row in 0..batch.num_rows() {
            rows.push((
                id.value(row),
                amount.value(row),
                region.value(row).to_string(),
            ));
        }
    }
    rows.sort();
    rows
}

fn number(value: &serde_json::Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| panic!("not an integer: {value}"))
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R020.2.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn iceberg_and_duckdb_reads_match_native() {
    common::configure_authority();
    let root = allowed_root().clone();
    let table = write_fixture().await;
    let lake = Lake::serve(table.clone());
    let origin_allow = medulla_federation::federation_ssrf::HTTP_JSON_FEDERATION_ALLOW_ENV;
    std::env::set_var(origin_allow, &lake.base);
    let state = test_support::durable_state(SECRET, common::current_isolation());

    // ── Iceberg: a partition-pruned, projected read equals the native scan ───────────────
    let spec = warehouse(
        config(WarehouseSourceKind::Iceberg, &[("catalog_uri", &lake.base)]),
        "sales.facts",
        &["region", "id", "amount"],
    );
    let registered = register(&state, 1, "lake_facts", spec).await;
    assert!(
        registered.error.is_none(),
        "register: {:?}",
        registered.error
    );

    let native = native_iceberg_rows(&table).await;
    assert_eq!(native.len(), 9, "three partitions of three rows");

    let east = read(
        &state,
        2,
        "lake_facts",
        &["id", "amount"],
        vec![predicate(
            "region",
            ForeignColumnComparison::Eq,
            serde_json::json!("east"),
        )],
    )
    .await;
    let served: Vec<(i64, i64)> = rows(&east)
        .into_iter()
        .map(|(_, columns)| (number(&columns["id"]), number(&columns["amount"])))
        .collect();
    let expected: Vec<(i64, i64)> = native
        .iter()
        .filter(|(_, _, region)| region == "east")
        .map(|(id, amount, _)| (*id, *amount))
        .collect();
    assert_eq!(served, expected, "the served read equals the native scan");
    assert_eq!(served, region_rows(0).into_iter().collect::<Vec<_>>());
    let asked = lake.data_files_asked();
    assert!(!asked.is_empty(), "the east data file was read");
    assert!(
        asked.iter().all(|key| key.contains("region=east")),
        "partition pruning skipped the other {} data files: {asked:?}",
        REGIONS.len() - 1
    );

    // An ordered numeric filter on an unpartitioned column: the residual keeps rows exact.
    let ranged = read(
        &state,
        3,
        "lake_facts",
        &["id"],
        vec![predicate(
            "amount",
            ForeignColumnComparison::Ge,
            serde_json::json!(2200),
        )],
    )
    .await;
    let served_ids: Vec<i64> = rows(&ranged)
        .iter()
        .map(|(id, _)| id.parse().unwrap())
        .collect();
    let native_ids: Vec<i64> = native
        .iter()
        .filter(|(_, amount, _)| *amount >= 2200)
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(served_ids, native_ids);

    // A column the table lacks is schema drift, not an empty column.
    let spec = warehouse(
        config(WarehouseSourceKind::Iceberg, &[("catalog_uri", &lake.base)]),
        "sales.facts",
        &["region", "id", "missing_column"],
    );
    let drift_registered = register(&state, 4, "lake_drift", spec).await;
    assert!(
        drift_registered.error.is_none(),
        "register drift: {:?} {:?}",
        drift_registered.error,
        drift_registered.error_detail
    );
    let drift = read(&state, 5, "lake_drift", &["missing_column"], vec![]).await;
    assert!(
        refusal(&drift).starts_with("SCHEMA_DRIFT"),
        "{:?}",
        drift.error
    );

    // A catalog that is not there is SOURCE_UNAVAILABLE, never an empty result.
    let spec = warehouse(
        config(WarehouseSourceKind::Iceberg, &[("catalog_uri", &lake.base)]),
        "sales.no_such_table",
        &["id"],
    );
    assert!(register(&state, 6, "lake_gone", spec).await.error.is_none());
    let gone = read(&state, 7, "lake_gone", &["id"], vec![]).await;
    assert!(
        refusal(&gone).starts_with("SOURCE_UNAVAILABLE"),
        "{:?}",
        gone.error
    );

    // ── DuckDB: a file under the allowed root equals the native engine ──────────────────
    let db = root.join("facts.duckdb");
    let native_duck: Vec<(i64, String, f64)> = {
        let conn = duckdb::Connection::open(&db).expect("create the duckdb file");
        conn.execute_batch(
            "CREATE TABLE facts (id BIGINT, name VARCHAR, score DOUBLE);
             INSERT INTO facts VALUES (1,'ada',0.5),(2,'bea',1.5),(3,'cam',2.5),(4,'ada',3.5);",
        )
        .expect("fixture rows");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, score FROM facts WHERE name = 'ada' AND score > 1.0 ORDER BY id",
            )
            .expect("native query");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("native rows")
            .map(|row| row.expect("native row"))
            .collect()
    };
    let db_path = db.to_str().expect("utf-8 path");
    let spec = warehouse(
        config(WarehouseSourceKind::DuckDb, &[("database_path", db_path)]),
        "facts",
        &["id", "name", "score"],
    );
    let registered = register(&state, 8, "duck_facts", spec).await;
    assert!(
        registered.error.is_none(),
        "register: {:?}",
        registered.error
    );
    let served = read(
        &state,
        9,
        "duck_facts",
        &["id", "name", "score"],
        vec![
            predicate(
                "name",
                ForeignColumnComparison::Eq,
                serde_json::json!("ada"),
            ),
            predicate("score", ForeignColumnComparison::Gt, serde_json::json!(1.0)),
        ],
    )
    .await;
    let served: Vec<(i64, String, f64)> = rows(&served)
        .into_iter()
        .map(|(_, c)| {
            (
                number(&c["id"]),
                c["name"].as_str().unwrap().to_string(),
                c["score"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(served, native_duck);
    assert_eq!(served.len(), 1);

    // A column the DuckDB table lacks is schema drift.
    let spec = warehouse(
        config(WarehouseSourceKind::DuckDb, &[("database_path", db_path)]),
        "facts",
        &["id", "nope"],
    );
    assert!(register(&state, 10, "duck_drift", spec)
        .await
        .error
        .is_none());
    let drift = read(&state, 11, "duck_drift", &["nope"], vec![]).await;
    assert!(
        refusal(&drift).starts_with("SCHEMA_DRIFT"),
        "{:?}",
        drift.error
    );

    // A DuckDB file outside foreign.duckdb.allowed_roots is refused at registration.
    let outside = test_support::fresh_dir("r020-2-2-duckdb-outside").join("other.duckdb");
    duckdb::Connection::open(&outside).expect("create the outside file");
    let denied = register(
        &state,
        12,
        "duck_outside",
        warehouse(
            config(
                WarehouseSourceKind::DuckDb,
                &[("database_path", outside.to_str().expect("utf-8 path"))],
            ),
            "facts",
            &["id"],
        ),
    )
    .await;
    assert!(
        refusal(&denied).starts_with("DUCKDB_FILE_DENIED"),
        "{:?}",
        denied.error
    );

    // ── Snowflake is a connector's: the engine refuses CAPABILITY_UNSUPPORTED ───────────
    let spec = warehouse(
        config(
            WarehouseSourceKind::Snowflake,
            &[("account", "acme"), ("database", "db"), ("warehouse", "wh")],
        ),
        "facts",
        &["id"],
    );
    assert!(register(&state, 13, "snow_facts", spec)
        .await
        .error
        .is_none());
    let unsupported = read(&state, 14, "snow_facts", &["id"], vec![]).await;
    assert!(
        refusal(&unsupported).starts_with("CAPABILITY_UNSUPPORTED"),
        "{:?}",
        unsupported.error
    );
    std::env::remove_var(origin_allow);
}
