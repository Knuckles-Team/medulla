//! medulla-retrieval: Retrieval and expansion: knowledge assembly, LeanRAG, verdicts and stop rules.
//!
//! Level L5 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (retrieval).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Structured hierarchical retrieval (CONCEPT:MEDULLA-KG.retrieval.bounded-drill) — the LeanRAG method as a
/// library API over the MEDULLA-220 summary tier: retrieve at the summary/abstraction
/// level, then drill down `SUMMARIZES`/`CONSOLIDATES` provenance to the supporting
/// leaves, assembling a de-duplicated multi-level context that beats flat top-k on
/// coverage. Pure-Rust + dep-free (Pi-tier clean): it reads the graph and ANN index
/// through the small [`leanrag::GraphTopology`] / [`leanrag::AnnIndex`] accessor
/// traits, so it links no DataFusion and edits no medulla-core. It is a retrieval helper
/// a caller composes ABOVE the planner, NOT a wire `Op`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod leanrag;
/// RowSet v2, additive (CONCEPT:MEDULLA-KG.query.knowledge-set): [`knowledge::KnowledgeSet`] is the
/// enriched shape (kind/confidence/bitemporal window/column-projection/provenance+policy
/// frames) a caller builds from a FINISHED `RowSet` + the `GraphView` it ran over — a
/// terminal step above the op loop, exactly like `leanrag`. `RowSet` and `Op::execute`
/// are unchanged; this module is only exercised when a caller explicitly asks for it.
/// Sits under the existing `query` feature (no new cargo feature) because it borrows
/// `GraphView::node_row_object`'s decode.
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod knowledge;
/// The Arrow-columnar `KnowledgeBatch` (CONCEPT:MEDULLA-P1-2, P1 roadmap feedback) — a
/// RecordBatch-backed columnar projection of `KnowledgeSet`: entity/artifact id,
/// N named score columns, calibrated confidence, typed evidence-ref columns,
/// bitemporal valid/tx-time, provenance/policy-label list columns, lazy
/// blob-handle columns and native bounded result streams. Behind the
/// `knowledge-batch` feature (implies `query` and is enabled by facade `full`) so a
/// default/Pi build still links no Arrow. Family adapters cover graph, SQL, RDF,
/// vector, time-series, jobs, and cross-modal results.
#[cfg(feature = "knowledge-batch")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod knowledge_batch;
#[cfg(feature = "knowledge-batch")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod result_stream;
/// Document boosts from retrieval feedback (MEDULLA-FEDERATED-QUERY-R705).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod feedback_boost;
/// Agent knowledge scopes applied before truncation (MEDULLA-FEDERATED-QUERY-R704).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod knowledge_scope;
/// Retrieval sufficiency, selection and hierarchy decisions (MEDULLA-FEDERATED-QUERY-R081).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod retrieval_verdict;
