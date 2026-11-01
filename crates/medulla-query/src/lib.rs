//! medulla-query — the SQL/Cypher query surface (CONCEPT:MEDULLA-KG.query.read-only-sql-query).
//!
//! Read-only relational query over a single graph's nodes. ALL DataFusion/Arrow
//! code lives behind the `sql` cargo feature so a default build links none of it
//! (the engine stays lean for a Raspberry Pi).
//!
//! The `cypher` feature (CONCEPT:MEDULLA-KG.query.dep-free-behind) is a DEP-FREE Cypher subset — a
//! hand-written recursive-descent parser compiled to the engine's own primitives
//! (the medulla-core label index, `vf2_subgraph_match`, petgraph BFS). It pulls NO
//! DataFusion, so it ships in the lean Pi build.
//!
//! ## ModalityContract (CONCEPT:E4) — deliberately NOT retrofitted
//! `medulla-query` is a pure QUERY-EXECUTOR surface: it parses SQL/Cypher and compiles it to
//! scans/BFS over `medulla-core`'s graph primitives, producing result rows — it owns no
//! persisted modality VALUE type of its own (its public items are query planners,
//! `exec_sql*` entry points, and column/plan descriptors, all transient). The stored
//! objects a query READS already carry `ModalityContract` in their own crates
//! (`medulla-core::NodeChange`, `medulla-tensor`, `medulla-tsdb::SeriesMeta`, `medulla-rdf::owl::ProofNode`,
//! …). Per the `medulla-modality` README's own note that "pure protocol/executor crates with
//! no modality VALUE type of their own — `ModalityContract` may simply not apply, which
//! is a legitimate outcome, not a gap", this crate is a documented SKIP.

pub mod plan_scope;

pub use plan_scope::{PlanScope, PlanScopeError, ScopePredicate};


/// Arbitrary user-defined relational tables (CONCEPT:MEDULLA-KG.query.register-user-tables-alongside) — the durable redb
/// table store + DataFusion materialization. Behind `sql` (needs Arrow/DataFusion/redb).
#[cfg(feature = "sql")]
pub mod tables;


#[cfg(feature = "sql")]
pub use tables::AnnIndexPlan;
#[cfg(feature = "sql")]
pub use tables::AnnMethod;
#[cfg(feature = "sql")]
pub use tables::HypertablePlan;
#[cfg(feature = "sql")]
pub use tables::VectorMetric;
#[cfg(feature = "sql")]
pub use migration::conversion_may_be_lossy;
#[cfg(feature = "sql")]
pub use migration::MigrationPolicy;
#[cfg(feature = "sql")]
pub use migration::MigrationState;
#[cfg(feature = "sql")]
pub use migration::RollbackMetadata;
#[cfg(feature = "sql")]
pub use migration::SchemaMigration;
#[cfg(feature = "sql")]
pub use migration::SchemaMigrationApply;
#[cfg(feature = "sql")]
pub use migration::SchemaMigrationOperation;
#[cfg(feature = "sql")]
pub use migration::SchemaMigrationRecord;
#[cfg(feature = "sql")]
pub use migration::SchemaSnapshot;
#[cfg(feature = "sql")]
pub use migration::SecondaryIndexPolicy;
#[cfg(feature = "sql")]
pub use tables::AnnFallbackReason;
#[cfg(feature = "sql")]
pub use tables::AnnIndexStatus;
#[cfg(feature = "sql")]
pub use tables::AnnLimits;
#[cfg(feature = "sql")]
pub use tables::AnnRefreshOutcome;
#[cfg(feature = "sql")]
pub use tables::AnnRefreshPolicy;
#[cfg(feature = "sql")]
pub use tables::AnnServeReceipt;
#[cfg(feature = "sql")]
pub use tables::AnnServingPath;
#[cfg(feature = "sql")]
pub use tables::AnnTopK;
#[cfg(feature = "sql")]
pub use tables::AnnTopKRequest;
#[cfg(feature = "sql")]
pub use schema::Cell;
#[cfg(feature = "sql")]
pub use schema::CmpOp;
#[cfg(feature = "sql")]
pub use schema::ColCheck;
#[cfg(feature = "sql")]
pub use schema::Column;
#[cfg(feature = "sql")]
pub use schema::ColumnType;
#[cfg(feature = "sql")]
pub use tables::ConflictAction;
#[cfg(feature = "sql")]
pub use schema::FunctionArg;
#[cfg(feature = "sql")]
pub use schema::FunctionReturns;
#[cfg(feature = "sql")]
pub use tables::IndexCatalogTxnOp;
#[cfg(feature = "sql")]
pub use tables::PropertyGraphGrantee;
#[cfg(feature = "sql")]
pub use tables::PropertyGraphPrivilegeOperation;
#[cfg(feature = "sql")]
pub use tables::PropertyGraphPrivilegeStatement;
#[cfg(feature = "sql")]
pub use tables::PropertyGraphTxnOp;
#[cfg(feature = "sql")]
pub use index::SecondaryIndexColumn;
#[cfg(feature = "sql")]
pub use index::SecondaryIndexKind;
#[cfg(feature = "sql")]
pub use index::SecondaryIndexLookup;
#[cfg(feature = "sql")]
pub use index::SecondaryIndexOrder;
#[cfg(feature = "sql")]
pub use index::SecondaryIndexSpec;
#[cfg(feature = "sql")]
pub use schema::StoredFunction;
#[cfg(feature = "sql")]
pub use schema::TableSchema;
#[cfg(feature = "sql")]
pub use tables::TableStore;
#[cfg(feature = "sql")]
pub use tables::TableTxn;
#[cfg(feature = "sql")]
pub use tables::TxnOp;
#[cfg(feature = "sql")]
pub use tables::UserAnnAuthority;

#[cfg(feature = "sql")]
/// Resolve parsed SQL column declarations into the durable table-schema columns
/// consumed by every SQL transport. Keeping this conversion beside the two
/// types it joins prevents the embedded, native, and wire paths from drifting.
pub fn columns_from_defs(cols: &[ColumnDef]) -> Result<Vec<Column>, String> {
    cols.iter()
        .map(|column| {
            let ty = ColumnType::parse(&column.type_name)?;
            Ok(Column {
                name: column.name.clone(),
                ty,
                nullable: column.nullable,
                primary_key: column.primary_key,
                unique: column.unique,
                serial: column.serial,
                default: column.default.clone(),
                check: column.check.clone(),
            })
        })
        .collect()
}


#[cfg(feature = "sql")]
pub mod schema;

#[cfg(feature = "sql")]
pub mod index;

#[cfg(feature = "sql")]
pub mod migration;


/// CONCEPT:MEDULLA-KG.query.surface-b-numeric-operators — Analytics-Program Surface-B numeric SQL operators (`cosine_sim`,
/// `l2_normalize`, `zscore`, `covariance`) backed by the `medulla-numeric` kernel. Feature
/// `numeric` (out of `pi`); no pyo3 in the engine build.
#[cfg(feature = "numeric")]
pub mod numeric;

/// EH-522 — `eg_<func>(…) OVER (…)` window functions over the series kernels.
#[cfg(feature = "numeric")]
pub mod series_udwf;

/// Sketch-backed SQL aggregate UDFs (CONCEPT:MEDULLA-KG.query.approx-distinct-cardinality, W4.5/N5):
/// `approx_distinct`/`approx_frequency`/`minhash_signature` + the `minhash_similarity` scalar.
/// Always-on under base `sql` — the underlying sketches (`medulla_compute::sketch`) are pure-Rust
/// with zero heavy deps, already-linked `medulla-compute` unconditionally carries them.
#[cfg(feature = "sql")]
pub mod sketch_udfs;

#[cfg(feature = "sql")]
pub mod tablefuncs;

#[cfg(feature = "sql")]
pub mod udfs;
