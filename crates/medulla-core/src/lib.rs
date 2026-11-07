//! medulla-core — the graph engine core: in-memory storage (`graph`), the multi-tenant
//! `registry`, the ACL/`isolation` layer, and HNSW `compute::semantic` search.
//! Depends only on `medulla-types`. Everything graph-mutating lives here; the wire
//! protocol and server transport sit in higher crates.

// Shared test-only serialization lock for the process-global indexed-property env
// vars (`MEDULLA_MAX_INDEXED_PROPERTIES` / `MEDULLA_INDEXED_PROPERTIES`).
// The `graph` property-index tests AND the `index` manager cap test both mutate these
// globals, so both acquire THIS one lock — otherwise a cap-mutating test races a
// default-cap test under parallel `--all-features` (a torn `nodes_by_property` read).
//
// Non-poisoning on purpose: it guards `()`, so a test that panicked while holding it
// leaves no state behind to distrust, and std's poison would carry no information.
// Bounded thread fan-out and start gates for this crate's concurrency tests.

// CONCEPT:MEDULLA-KG.compute.message-broker-exchanges — message-broker exchanges/routing on top of the KG-2.303 queue.
/// Graph-derived architecture view (MEDULLA-CQ-ORCH-R010.2).
pub mod architecture_view;
#[cfg(feature = "broker")]
pub mod broker;
/// Data contract checks (MEDULLA-TYPED-PACKS-R805.2).
pub mod data_product;
pub mod decay;
/// Canonical durable-mutation classification/application, hoisted down from the
/// facade's `src/mutation_apply.rs` (`plans/pyengine/MEDULLA-PYENGINE-PLAN.md` §4.2) so a
/// caller below the facade (`crates/medulla-pyengine`) can share it instead of
/// re-deriving a drifting copy. See the module's own doc for exactly what moved and
/// what stayed at the facade (DAG-forced: needs `medulla-compute`/`medulla-rdf`/`src/server`).
pub mod durable_apply;
/// Ecosystem package dependency risk propagation (MEDULLA-SUPPLY-VOCAB-R009).
pub mod ecosystem;
/// Declared freshness read from the graph (EH-400): the class volatility policy and foreign-source
/// watermarks. Its change detector is the dependency clock, so it is gated with it.
#[cfg(feature = "result-cache")]
pub mod freshness;
/// A small, dependency-free filesystem primitive shared by the query spill
/// owner and the server's durable commit-intent writer.
pub mod fs;
// MEDULLA-SDLC — derived lifecycle edges, defect intake, requirement state and trace.
pub mod lifecycle;
/// MEDULLA-MEDIA-VOCAB-R001.101.2 — the pure meeting transcript revision function.
pub mod meeting_transcript;
/// Memory snapshot head, revision, candidate and acknowledgment rows (MEDULLA-DURABLE-KERNEL-R085.3).
pub mod memory_snapshot_store;
/// CONCEPT:MEDULLA-KG.ontology.codegen-from-rust-vocabulary — deterministic ontology/SHACL-shape
/// text generated from a typed Rust vocabulary, with a byte-equality regeneration test
/// (MEDULLA-TYPED-PACKS-R090.1). The canonical ontology/shape corpus itself is owned by the
/// typed vocabulary, not hand-edited Turtle.
pub mod ontology;
pub mod ontology_codegen;

/// Single-writer-per-persist-dir guard, hoisted down from the facade's
/// `src/persist_lock.rs` (same `engine.lock` `flock` mechanism, unchanged) so a
/// caller below the facade (`crates/medulla-pyengine`) can take the SAME exclusive lock a
/// second embedded engine on the same directory would be refused. Also the home of
/// `PersistMode`, the explicit-choice persist-dir contract (`plans/pyengine/
/// MEDULLA-PYENGINE-PLAN.md` §9.2 Gate A / BUG-PE-003) that makes an unconfigured silent
/// fallback path structurally unrepresentable.
pub mod persist_lock;
// MEDULLA-CQ-ORCH-R701.2: canonical process form, admission and export.
pub mod process_import;
/// Quality rule compilation, scoring and failure tracking (MEDULLA-UNIFIED-DATA-PLANE-R762.2).
pub mod quality;
pub mod registry;
/// MEDULLA-INCREMENTAL-VIEWS-R005.1 — the process-global view status board.
pub mod view_status;
