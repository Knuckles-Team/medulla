//! `iceberg('namespace.table')` end to end over a real partitioned table
//! (MEDULLA-FEDERATED-QUERY-R051): the provider is built by a sync caller, then scanned by
//! DataFusion on a different runtime, and the manifest planner prunes data files from the
//! filter DataFusion pushes at scan time.
//!
//! The table is written with the `iceberg` crate's own writers — a table partitioned by
//! `region`, one Parquet data file per partition, committed through a transaction — into
//! an in-memory store under `s3://` locations. An in-process server then plays the two
//! remote parties the provider talks to: the Iceberg REST catalog (`/v1/config` and the
//! table load) and the S3-compatible object store holding the table's files (`HEAD` and
//! ranged `GET`). Nothing is mocked inside the provider: it uses its real REST client and
//! its real object-store client against that server.
#![cfg(all(feature = "sql", feature = "iceberg-federation"))]

use std::sync::Arc;

use arrow::array::Int64Array;
use arrow::record_batch::RecordBatch;
use datafusion::catalog::TableProvider;
use datafusion::prelude::SessionContext;
use medulla_sql::{sql::build_iceberg_provider, sql::ICEBERG_FEDERATION_CATALOG_URI_ENV};

#[path = "iceberg_lake/mod.rs"]
mod iceberg_lake;
use iceberg_lake::{region_rows, write_fixture, Lake, REGIONS};

// ── the scan ────────────────────────────────────────────────────────────────────────

/// `(id, amount)` rows of `sql`, in result order.
async fn id_amount(ctx: &SessionContext, sql: &str) -> Vec<(i64, i64)> {
    let batches = ctx
        .sql(sql)
        .await
        .expect("plan the query")
        .collect()
        .await
        .expect("run the query");
    let column = |batch: &RecordBatch, index: usize| -> Vec<i64> {
        let values = batch.column(index).as_any().downcast_ref::<Int64Array>();
        values.expect("an Int64 column").values().to_vec()
    };
    batches
        .iter()
        .flat_map(|batch| column(batch, 0).into_iter().zip(column(batch, 1)))
        .collect()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("runtime")
}

// spec: MEDULLA-FEDERATED-QUERY-R051
#[test]
fn a_partitioned_table_is_scanned_through_the_provider_with_files_pruned() {
    let writer = runtime();
    let lake = Lake::serve(writer.block_on(write_fixture()));
    std::env::set_var(ICEBERG_FEDERATION_CATALOG_URI_ENV, &lake.base);

    // Built by a sync caller outside any runtime, as the table function is at plan time.
    let provider = build_iceberg_provider("sales.facts", None).expect("load the table");
    let before = provider.pushdown_stats();
    assert_eq!(before.total_data_files, 3, "one data file per region");
    assert_eq!(before.columns_total, 3);
    assert!(
        lake.data_files_asked().is_empty(),
        "loading reads metadata only"
    );

    // Scanned on DataFusion's runtime: a different runtime, started after the load.
    let ctx = SessionContext::new();
    ctx.register_table("facts", provider.clone() as Arc<dyn TableProvider>)
        .expect("register the provider");
    let queries = runtime();
    let east = queries.block_on(id_amount(
        &ctx,
        "SELECT id, amount FROM facts WHERE region = 'east' ORDER BY id",
    ));
    assert_eq!(east, vec![(1, 100), (2, 200), (3, 300)]);
    let pruned = provider.pushdown_stats();
    assert_eq!(
        pruned.files_scanned, 1,
        "the pushed filter leaves one partition"
    );
    assert_eq!(pruned.files_skipped(), 2, "two data files were pruned");
    let asked = lake.data_files_asked();
    assert!(!asked.is_empty(), "the east data file was read");
    assert!(
        asked.iter().all(|key| key.contains("region=east")),
        "no pruned data file was requested from the store: {asked:?}"
    );

    // No filter: every file, and only the projected columns.
    let all = queries.block_on(id_amount(&ctx, "SELECT id, amount FROM facts ORDER BY id"));
    let expected: Vec<(i64, i64)> = (0..REGIONS.len()).flat_map(region_rows).collect();
    assert_eq!(all, expected, "nine rows over three partitions");
    let full = provider.pushdown_stats();
    assert_eq!((full.files_scanned, full.files_skipped()), (3, 0));
    assert_eq!(
        (full.columns_projected, full.columns_total),
        (2, 3),
        "the projection reached the scan"
    );

    // The provider outlives the runtime that first scanned it, and may be dropped on one.
    drop(queries);
    let later = runtime();
    let west = later.block_on(id_amount(
        &ctx,
        "SELECT id, amount FROM facts WHERE region = 'west' AND id > 21 ORDER BY id LIMIT 5",
    ));
    assert_eq!(west, vec![(22, 2200), (23, 2300)]);
    assert_eq!(provider.pushdown_stats().files_skipped(), 2);
    later.block_on(async move {
        drop(ctx);
        drop(provider);
    });
}
