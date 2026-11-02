//! medulla-federation: Federated planning across sources: federation optimizer, SSRF guard and remote engines.
//!
//! Level L5 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (federation).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-UNIFIED-DATA-PLANE-R007 — named pushdown operations and a dialect's
// declared capability set, refusing an unknown operation name.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_capabilities;
// The federation foreign-source seam (CONCEPT:MEDULLA-KG.query.query-federation) — the `ForeignSource` trait +
// the remote-engine / HTTP-JSON kinds backing `Op::ForeignScan`. Implies `query`.
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation;
/// AGE graph reads bound to the existing guarded PostgreSQL federation transport
/// (MEDULLA-DURABLE-KERNEL-R024.1).
#[cfg(feature = "federation-sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_age;
/// The brain-guarded backend source (MEDULLA-DURABLE-KERNEL-R024.6): typed shape
/// validation plus an explicit, named refusal, since no bound guard-policy
/// evaluation driver exists yet.
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_brain_guarded;
/// The Neo4j/FalkorDb halves of the Cypher transpiler source
/// (MEDULLA-DURABLE-KERNEL-R024.3): typed shape validation plus an explicit,
/// named refusal, since no bound Bolt/RESP driver exists yet. AGE (the
/// third Cypher dialect) is bound in `federation_age` behind
/// `federation-sql`.
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_cypher_unbound;
/// The federation optimizer (EH-563): capability-driven pushdown of keys, limits and pages
/// into foreign sources, bind joins, the per-query network budget and the fragment trace.
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_opt;
/// The Spark job backend source's explicit-registration half
/// (MEDULLA-DURABLE-KERNEL-R024.8): typed `artifact://`-scheme shape validation
/// plus an explicit, named refusal, since no bound Spark artifact-store
/// driver exists yet. `federation::source_for`'s generic dispatch already
/// refuses an inline `SparkBatch` spec through `Oq2Unbound`; this is the
/// per-registration counterpart `federation_age`/`federation_trino` give
/// their own backend.
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_spark;
/// The one outbound-destination gate for federation (HTTP URLs, SQL DSN hosts).
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_ssrf;
/// Learned, provenanced join cardinality statistics for ordering joins across foreign
/// sources (MEDULLA-FEDERATED-QUERY-R048.1 — typed-model slice of R048).
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_stats;
/// Explicitly bound, probed Trino client protocol source (EH-580).
#[cfg(feature = "federation-sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation_trino;
/// The write-side mirror-target seam (MEDULLA-DURABLE-KERNEL-R024.2) — the
/// `MirrorTarget` trait binding a [`medulla_types::wire::MirrorTargetSpec`] to a
/// driver. Mirrors `federation`'s read-side `ForeignSource` seam.
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mirror_target;
#[cfg(all(any(test, feature = "test-support"), feature = "federation"))]
pub mod test_guard;
// The federation foreign-scan + compose-join-with-local proofs (CONCEPT:MEDULLA-KG.query.query-federation):
// a mock HTTP/JSON source joined with the local graph in ONE plan == the manual join.
#[cfg(all(test, feature = "federation"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod federation_tests;
// The federation NAME-REGISTRY proofs (CONCEPT:MEDULLA-KG.query.closure-backed-source): a registered source resolves by
// name (`Named` `Op::ForeignScan` + `Op::Foreign`), composes with a local Join, an
// unbound name errors cleanly, and an empty/absent registry keeps the prior behavior.
#[cfg(all(test, feature = "federation"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod federation_registry_tests;
// The external-SQL federation proofs (CONCEPT:MEDULLA-KG.query.feature): an external relational-SQL
// source (`ForeignSourceSpec::Sql`) joined with the local graph in ONE plan == the
// manual join, plus the real sqlx DSN path errors cleanly when unreachable.
#[cfg(all(test, feature = "federation-sql"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod federation_sql_tests;
