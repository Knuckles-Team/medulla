//! SQL surface (CONCEPT:MEDULLA-KG.query.read-only-sql-query, edges/joins + UDFs CONCEPT:MEDULLA-KG.query.version-keyed-cache): read-only
//! `SELECT ... FROM nodes [JOIN edges ...]` over one graph via DataFusion.
//! Schema-on-read — node property MessagePack blobs are scanned into Arrow
//! RecordBatches with a union-of-keys inferred schema plus a raw `props: Binary`
//! escape hatch; `json_get*` UDFs reach fields the inferred schema widened or
//! dropped. The `edges` table exposes the topology (`src/dst/rel/props`) for joins,
//! `epistemic_decay` salience-weights facts in-query, and `pagerank()`/
//! `betweenness()` table functions plus `var`/`cvar` aggregates (feature `finance`)
//! bring graph + finance kernels into SQL.
//!
//! ## Postgres arrays, ranges & common functions (CONCEPT:MEDULLA-KG.query.greatest-least-int4range-tsrange)
//! Rounds out the Postgres drop-in surface ORMs/BI tools emit:
//!   * **Arrays** — DataFusion's native `List` support (enabled via the crate's
//!     `nested_expressions` feature) covers `array[…]` literals, `unnest`, `array_agg`,
//!     `array_length`, the `||` concat, `array_has`/`array_has_all`/`array_has_any`, and
//!     `= ANY(array)`. No engine array reimplementation — we register the feature + test
//!     it. The pg containment/overlap *operators* map to functions: `@>` ⇒
//!     `array_has_all`, `&&` ⇒ `array_has_any` (the `@>` token itself stays bound to
//!     JSONB containment on node columns, CONCEPT:MEDULLA-KG.compute.json-deep-indexing). `> ALL(array)` is **not**
//!     supported by DataFusion 54's SQL planner (deferred).
//!   * **Scalar/aggregate functions** — most are already in DataFusion 54
//!     (`split_part`, `regexp_replace`/`regexp_match`, `date_trunc`, `date_part`,
//!     `to_timestamp`, `coalesce`, `nullif`, `string_agg`, `array_agg`, `concat`,
//!     `substr`, …); MEDULLA-104 ADDS `greatest`/`least` (absent in 43) and desugars
//!     `EXTRACT(f FROM x)` → `date_part('f', x)` (no ExprPlanner lowers `EXTRACT` in this
//!     build). `to_char` is native for date/time/duration; numeric `to_char(numeric,fmt)`
//!     is **deferred** (overriding the name would drop the native temporal variants).
//!   * **`generate_series(start, stop[, step])`** — a table function (integer series /
//!     calendar spine), registered here because the lean DataFusion build ships none.
//!   * **Range types** — a PRAGMATIC subset (full pg range OIDs/multiranges are heavier
//!     than DataFusion's type system): a range is its canonical text `[lo,hi)` with
//!     `int4range`/`tsrange` constructors and `range_contains`/`range_overlaps`/
//!     `range_contains_range` (`@>`)/`range_contained_by` (`<@`) predicate UDFs over a
//!     discrete i64 interval. Sub-second `tsrange` precision and `numrange` are deferred.
//!
//! ## SQL window functions (CONCEPT:MEDULLA-KG.temporal.columnar-schema-inference)
//! `<fn>() OVER (PARTITION BY … ORDER BY … <ROWS|RANGE frame>)` is DataFusion-backed:
//! DataFusion 54 provides the window operator + the full function set natively
//! (ranking `ROW_NUMBER`/`RANK`/`DENSE_RANK`/`NTILE`/`PERCENT_RANK`/`CUME_DIST`;
//! offset `LAG`/`LEAD`/`FIRST_VALUE`/`LAST_VALUE`/`NTH_VALUE`; aggregate
//! `SUM`/`AVG`/`MIN`/`MAX`/`COUNT OVER`; `ROWS`/`RANGE` frame specs with the standard
//! default frame). A window `SELECT` classifies as [`StatementKind::Read`] and runs
//! through the SAME `exec_sql` → `SessionContext::sql` path as any other query — no
//! separate planner/evaluator, no classify change needed. See
//! `tests/window_functions.rs` for the partition → order → frame coverage.

/// Real pgvector ANN top-k pushdown execution (CONCEPT:MEDULLA-KG.query.real-pgvector-ann-top) — build/consult a real
/// medulla-ann HNSW/IVF index over a vector column and return the true nearest-k, replacing
/// the MEDULLA-115 brute-force `vector_l2()` scan when a matching MEDULLA-116 index is registered.
mod ann;
/// The row-level shape a maintained ANN pushdown must honour (`WHERE`, `OFFSET`,
/// row-ranked projection) — RF-019.
#[cfg(test)]
mod ann_authority_tests;
mod ann_filter;
/// The ANN pushdown decision: user tables narrow through the maintained ANN
/// authority, `nodes` through its batch slice — RF-019.
mod ann_pushdown;
mod catalog;
/// `medulla_embed(text)` — the server-side text→vector SQL scalar function (design
/// `plans/semantic-indexing/DESIGN-embedding-bindings.md` §3.1, §9 phase 2). Returns the
/// `List<Float32>` a `vector(n)` column materializes as, so it drops straight into the
/// existing pgvector distance operators (`col <=> medulla_embed('leaky pump')`) and a
/// psql/ORM caller never pre-embeds client-side. Fail-closed when unbound.
mod embed_udf;
mod exec;
pub(crate) mod filter_shape;
/// CA-19 (GOC-77 W01-W05, BUG-224) — the `iceberg('namespace.table'[, snapshot_id])`
/// federated table function over the official apache/iceberg-rust REST-catalog client.
/// Gated `iceberg-federation` (implied by `sql`, see that feature's Cargo.toml comment).
#[cfg(feature = "iceberg-federation")]
mod iceberg_federation;
// `pub(crate)` (not `mod providers;`) so `crate::tables::provider::UserTableProvider`
// (CONCEPT:MEDULLA-KG.query.register-each-user-table) can reuse `NodesTableProvider`'s generic equality-index
// pushdown for its own full-scan fallback, instead of duplicating that machinery.
// This does NOT widen the crate's EXTERNAL API: nothing outside `medulla-query` reaches
// a module path directly — external consumers only ever see the explicit `pub use`
// re-exports below (`SqlCache`), unchanged.
pub(crate) mod providers;
mod spill;
// The catalog objects of one table store a SQL read registers (EH-352 status rows).
mod store_catalog;
// `information_schema.medulla_index_status` (EH-352), synthesized like the other catalogs.
mod index_status;
// EH-387: the read-only gate of the graph-free SQL path.
mod read_only;
#[cfg(test)]
mod pgfamily_tests;
pub use read_only::require_single_read;

pub use ann::ColumnVectorBinding;
pub use ann_filter::{AnnDeclineReason, AnnQueryShape};
pub use ann_pushdown::{user_ann_decision, UserAnnDecision, UserAnnPushdown};
// Design §9 phase 2 — the `medulla_embed(text)` scalar function plus the seam a facade binds
// the engine's existing `medulla_plan::TextEmbedder` through (`medulla-query` cannot name that
// trait directly: `medulla-plan` depends on `medulla-query`, so the reverse edge is a package
// cycle — see `embed_udf`'s module doc).
pub use embed_udf::{bind_text_embedder, medulla_embed_udf, medulla_embed_udf_with, EmbedFn};
pub use exec::{
    exec_graph_table_typed_with_tables, exec_graph_table_typed_with_tables_cancellable, exec_sql,
    exec_sql_arrow, exec_sql_arrow_cancellable, exec_sql_cached, exec_sql_over_tables,
    exec_sql_typed, exec_sql_typed_cancellable, exec_sql_typed_with_tables,
    exec_sql_typed_with_tables_cached_cancellable, exec_sql_typed_with_tables_cancellable,
    QueryResult, SqlContextCache,
};
pub use spill::{default_spill_rows, CancellationToken, StreamOutcome};
// Postgres-family extension parity plans + planners.
pub use providers::SqlCache;
// CA-19 (GOC-77 W01-W05, BUG-224) — federated Iceberg read pushdown stats, exposed so a
// test can assert real manifest-level file skipping (P1's proof shape) rather than
// trusting an unobservable internal count.
#[cfg(feature = "iceberg-federation")]
pub use iceberg_federation::{
    build_iceberg_provider, IcebergPushdownStats, IcebergTableProvider,
    ICEBERG_FEDERATION_CATALOG_URI_ENV, ICEBERG_FEDERATION_CREDENTIAL_ENV,
    ICEBERG_FEDERATION_OAUTH2_URI_ENV, ICEBERG_FEDERATION_SCOPE_ENV, ICEBERG_FEDERATION_TOKEN_ENV,
    ICEBERG_FEDERATION_WAREHOUSE_ENV,
};
