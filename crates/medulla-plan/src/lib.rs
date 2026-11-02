//! # medulla-plan — the unified cross-modal query planner (CONCEPT:AU-KG.compute.vector/209)
//!
//! ONE query plan that *filters (relational) → traverses (graph) → ranks (vector)*
//! over the SAME off-lock snapshot in a single execution — instead of three siloed
//! surfaces (DataFusion SQL, petgraph BFS, vector kNN) stitched together by the
//! caller across three round-trips. This is the production increment of the Wave-7
//! feasibility spike (`repo://reports/spike-unified-findings.md`).
//!
//! ## The closed algebra
//!
//! The intermediate that flows between operators is a [`RowSet`] — an ordered id set
//! carrying optional scores. Graph, table and vector results are all reducible to
//! "ids + score", so that is the shared currency, and every operator is
//! `(RowSet) -> RowSet`. A plan is just an ordered `Vec<Op>`
//! ([`algebra::Op`]: `Scan | Filter | Traverse | Rank | Limit`), so any op's output
//! is a legal input to any op — a CLOSED algebra. That is what a SQL+graph+vector
//! unification needs and what a federation-of-three-engines cannot give you (it has
//! to round-trip through three result encodings).
//!
//! ## Reusing the existing legs (NOT a new engine)
//!
//! This is a thin BESPOKE planner SEQUENCING the legs the engine already has — it
//! does NOT reimplement SQL or the graph/vector engines:
//!  * `Filter`   runs **real DataFusion** (`medulla_sql::sql::exec_sql` over the
//!    schema-on-read `nodes` provider) — DataFusion stays the relational sub-engine.
//!  * `Traverse` is petgraph BFS over the `GraphView` topology (the relationship is
//!    read from the edge property blob, matching medulla-query/cypher).
//!  * `Rank`     is the `SemanticStore` vector kNN.
//!
//! ## Cost-based reorder (CONCEPT:MEDULLA-KG.query.concept-14)
//!
//! [`cost::CostModel`] orders an adjacent `(Filter, Rank)` pair filter-first vs
//! vector-first by selectivity — the cross-modal reorder a unified planner exists to
//! make. Both orders return the same set; only the work differs.
//!
//! ## Feature gating
//!
//! The DataFusion `Filter` leg — and [`Plan::execute`], which needs it — live behind
//! the `query` cargo feature. A default build links NO DataFusion: the algebra, the
//! cost model, the IR and the dep-free graph/vector legs all compile (the Pi
//! contract). The facade gates this crate's `query` exactly as it gates `medulla-query`.
//!
//! ## Explicitly deferred to later increments
//!
//! Per the spike's incremental binding order, these are NOT in this increment:
//!  * **Reason-in-plan** — a datalog fixpoint `Op::Reason` (transitive closure with
//!    iteration bounds / semi-naïve evaluation).
//!  * **Projected columns** — carrying full property rows across the boundary
//!    (projection pushdown) instead of re-materializing from the snapshot.
//!  * **Blob / object source ops** — another leaf source joined by id.
//!  * **Cross-modal write ACID** — running the plan inside one `GraphTxn` and writing
//!    back inferred edges/embeddings atomically.
//!  * **Cross-shard** — per-shard sub-plans + a coordinator merge + traversal
//!    frontier forwarding (the multi-quarter piece).

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them

// MEDULLA-UNIFIED-DATA-PLANE-R020.2.1 — warehouse federation pushdown-transport
// dispatch: Arrow Flight SQL vs. native per WarehouseSourceKind, with a
// refusal for an invalid config or an unmet Flight SQL preference. Pure
// `std` + `medulla-types`, no I/O; issuing the pushed-down query is a later
// child.
pub mod warehouse_pushdown;
// MEDULLA-UNIFIED-DATA-PLANE-R005 — mapping approval-state machine for named R2RML
// virtual graphs; an unapproved or model-only mapping refuses to be queried.
pub mod virtual_graph_mapping;
// MEDULLA-UNIFIED-DATA-PLANE-R011 — typed freshness route and a bounded
// read-your-writes wait, refusing an unbounded (zero-timeout) wait.
pub mod freshness_router;
// MEDULLA-FEDERATED-QUERY-R654.2 bounded source slice: deterministic hub ranking
// over an already-resolved code subgraph. Anchor resolution, bounded expansion,
// operations diagnosis and served CodeContext wiring remain later slices.
#[cfg(feature = "query")]
pub mod code_context;


/// The UQL text front-end (CONCEPT:AU-KG.query.top-nodes-by-degree) — `uql::parse(text) -> wire::Plan`. It
/// is a pure parser (lexer + recursive descent, NO DataFusion), so it ships in a
/// default/Pi build alongside the algebra/cost/IR; only EXECUTION of the resulting
/// Plan is `query`-gated.
pub mod uql;


// The natural-language → query planning seam (CONCEPT:MEDULLA-KG.query.core-query-input) + the concrete standalone
// `UreqNlPlanner` (CONCEPT:MEDULLA-KG.query.fence-stripper). The `NlPlanner` trait + the `plan_and_execute*`
// helpers are pure (only touch the existing `uql::parse` + `execute`), so they ride the
// `nl-query` feature which IMPLIES `query`; the concrete `UreqNlPlanner` inside is
// additionally gated on `nl-query` (it pulls the shared `ureq` rustls client). Kept OUT
// of pi.
/// ClickHouse attached-source driver config + connect/refusal path
/// (MEDULLA-UNIFIED-DATA-PLANE-R017.2.1). Pure `std`, always compiled: no heavy HTTP/native
/// client dependency is needed until the live slice (`R017.2.2`) issues real requests.
pub mod clickhouse_connection;
/// Oracle/Db2 attached-source driver config + connect/refusal path
/// (MEDULLA-UNIFIED-DATA-PLANE-R018.2.1). Pure `std`, always compiled: no driver/ODBC
/// client dependency is needed until the live slice (`R018.2.2`) issues real reads.
pub mod relational_connection;
/// Pushdown to a peer engine, end to end (they run `uql::serve::run_statement`, so they sit above federation).
#[cfg(all(test, feature = "federation"))]
mod engine_peer_tests;


// The hierarchical-retrieval surface (CONCEPT:MEDULLA-KG.retrieval.bounded-drill): the retriever + its accessor
// traits + the structured result + the flat-top-k baseline, so a caller names them
// through medulla-plan.
/// The Lane-0 shared cost traits A's optimizer + B's runtime both read
/// (CONCEPT:MEDULLA-KG.query.cost-cardinality-traits): the per-op [`cost::Cardinality`] estimator
/// and the [`cost::CostEstimate`] triple. Gated on `query` (the estimator borrows `PlanCtx`);
/// this tier only defines the shape — no estimator is wired, so behavior is unchanged.
/// The `PROPAGATE` cone as plain ids (EH-526): what a standing impact watch recomputes.
/// The in-txn tsdb read-your-own-writes staged-series overlay (CONCEPT:MEDULLA-KG.query.txn-tsdb-read-your).
/// The typed DAG plan surface (CONCEPT:MEDULLA-KG.query.plan-dag, E5): the node/edge shape plus
/// its physical executor, so a caller building a genuine multi-branch plan names them
/// through `medulla_plan` directly (mirroring how `Op`/`Plan` are re-exported from `algebra`).
/// The incremental-matview surface (CONCEPT:MEDULLA-KG.storage.incremental-matview): the Z-set
/// delta model + the circuit a caller compiles a `Plan` into and folds CDC deltas through.
/// The RowSet v2 surface (CONCEPT:MEDULLA-KG.query.knowledge-set): the enriched, ready-to-consume
/// row/set shape a caller builds from a finished `RowSet` + the `GraphView` it ran over.
/// The Arrow-columnar `KnowledgeBatch` surface (CONCEPT:MEDULLA-P1-2): the RecordBatch-
/// backed columnar projection of a `KnowledgeSet` and its row type.
/// The Lane-0 foundation seams the follow-on lanes hang off (append-only): the logical-plan
/// optimization hook `plan_optimize` (CONCEPT:MEDULLA-KG.query.plan-optimize-seam — Lane A's cost
/// optimizer fills its body) and the physical `Driver` execution trait + its `SerialDriver`
/// impl (CONCEPT:MEDULLA-KG.query.exec-driver-seam — Lane B swaps in a parallel driver). Both are
/// identity/serial in this tier, so `execute` is behavior-identical.
/// The Lane B physical runtime surface (CONCEPT:MEDULLA-KG.query.parallel-runtime): the
/// `execute_ops` driver-dispatch `execute` routes through (always, on `query`), plus — only
/// on the opt-in `par-runtime` feature — the rayon-morsel `ParallelDriver` and its
/// `RuntimeConfig` (pool width / spill budget, from `MEDULLA_EXEC_THREADS` /
/// `_SPILL_MB`). A default build has neither; `execute_ops` dispatches to `SerialDriver`,
/// byte-for-byte the prior fold.
/// Lane A's cross-modal optimizer surface (CONCEPT:MEDULLA-KG.query.xmodal-cost-optimizer): the
/// `optimize(plan, ctx) -> Plan` rule-engine entry point `plan_optimize` drives, its
/// `enabled()` runtime kill-switch, and the plan-time cardinality/cost estimators the rules
/// read (`ModalityCardinality` + the snapshot-memoized `PlanStats` catalog).
/// The DAG-aware optimizer (CONCEPT:MEDULLA-KG.query.dag-optimizer, E5 phase 3) — [`optimizer::optimize_dag`]
/// reorders ops within each maximal linear chain segment of a [`dag::PlanDag`] through the
/// SAME rule engine `optimize` drives, never crossing a branch/fan-out boundary (the DAG
/// generalization of the MEDULLA-405 adjacency guard).
/// The adaptive re-optimization hook (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization): re-cost and,
/// if warranted, re-order the not-yet-executed tail of a plan once an earlier op's ACTUAL
/// output cardinality diverges from what plan-time estimation predicted — the runtime feedback
/// loop beyond the static, once-per-`optimize()` cost decision. Auto-wired into
/// [`exec::run_with_adaptive_reopt`] (both [`exec::SerialDriver`] and the opt-in
/// `par-runtime` `runtime::ParallelDriver` share that one loop), so every ordinary
/// `Plan::execute` already calls this after each op; see [`optimizer::reoptimize_remaining`]
/// for the divergence threshold.
/// The learned-cost queryable-artifact surface (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization,
/// W4.12 phase 2): the process-wide [`learned_cost::LearnedCostStore`], its serializable
/// [`learned_cost::CostCorrectionArtifact`] snapshot, and the runtime opt-in flag —
/// re-exported at the crate root mirroring [`optimizer::enabled`]'s `cost_opt_enabled`
/// re-export above.
/// The server-side text→vector embedder seam (CONCEPT:MEDULLA-KG.compute.no-embedder-bound-op): the `TextEmbedder` trait
/// backing the UQL `RANK BY ~ "text"` (`Op::RankEmbed`) NL→vector resolver, plus the
/// deterministic `HashEmbedder` fallback for tests/offline use. A facade binds a concrete
/// model onto a `PlanCtx` via `with_embedder`.
/// The full, un-flattened E1 justification tree behind `EXPLAIN BELIEF` (E5 phase 4,
/// CONCEPT:MEDULLA-KG.epistemic.epistemic-substrate) — mirrors `Op::ExplainBelief`'s RowSet
/// projection but returns the verbatim `medulla_epistemic::JustificationGraph` a standalone
/// `Method::ExplainBelief` wire-projects (mirroring `Method::OwlExplain`'s `ProofNodeWire`).
// The NL→query seam surface (CONCEPT:MEDULLA-KG.query.core-query-input/MEDULLA-080): the trait + the LLM-optional
// `Option<&dyn NlPlanner>` entry point, and the concrete `UreqNlPlanner`.
// Re-export the federation surface so a caller naming a foreign source goes through
// medulla-plan: the trait + the spec-dispatcher.
// The federation NAME-RESOLUTION seam (CONCEPT:MEDULLA-KG.query.closure-backed-source): the by-name registry + the
// registerable owned source kinds (spec-backed / table / closure) + the shared handle.
// These complete `Op::Foreign` / a `Named` `Op::ForeignScan` — a name → live source →
// rows resolution the executor consults via `PlanCtx::with_foreign`.
// Re-export the lexical surface (CONCEPT:AU-KG.query.text-spatial-time) so a caller wiring a text plan
// names them through medulla-plan: the BM25 index, the hit row, and the RRF helper.
#[cfg(feature = "text")]
pub use medulla_text::{rrf_fuse, TextHit, TextIndex, RRF_K};
// The `PlanCtx::with_text` search-surface trait (CONCEPT:MEDULLA-KG.query.served-text-index-binding) — a facade
// implements this over its OWN maintained persistent index (rather than a plain
// `medulla_text::TextIndex`) to push a served `RankText`/`FuseRrf` leg down into it.
// The `PlanCtx::with_spatial` search-surface trait (CONCEPT:MEDULLA-KG.storage.incremental-spatial, L37) — mirrors
// `TextSource`: a facade implements this over its OWN maintained persistent spatial
// index to push a served `Op::SpatialScan` down into it.
#[cfg(all(test, feature = "query"))]
mod tests;

// The server-side embedder seam proofs (CONCEPT:MEDULLA-KG.compute.no-embedder-bound-op/412): `RANK BY ~ "text"`
// (`Op::RankEmbed`) resolves the text to a query vector via the embedder bound on the
// `PlanCtx` and ranks identically to the literal-vector oracle; an unbound embedder is a
// clean typed error; the `HashEmbedder` fallback is deterministic. Gated on `query`.
#[cfg(all(test, feature = "query"))]
mod embedder_tests;

// EH-564/EH-565: a candidate-restricted vector `Rank` is exact and complete.
#[cfg(all(test, feature = "query"))]
mod vector_rank_tests;

// The lexical BM25 `RankText` + RRF `FuseRrf` hybrid proofs (CONCEPT:AU-KG.query.text-spatial-time).
#[cfg(all(test, feature = "text"))]
mod text_tests;

// The OWL `Reason` + SPARQL `SparqlBgp` source-op compose-oracle proofs
// (CONCEPT:MEDULLA-KG.ontology.concept-12).
#[cfg(all(test, feature = "owl"))]
mod owl_tests;




// The spatial `SpatialScan` + `Spatial{Within,DWithin}` executor proofs
// (CONCEPT:MEDULLA-KG.ontology.singles-concept): a bbox R-tree scan + geometry filters compose with the graph/
// vector legs in ONE plan.
#[cfg(all(test, feature = "geo"))]
mod geo_tests;

// The document/JSON `Pred::JsonPath` executor proofs (CONCEPT:MEDULLA-KG.compute.json-deep-indexing): deep JSONPath
// existence / `->>`-equality / `@>`-containment filters apply per-row against the stored
// JSON and compose with the graph/vector legs in ONE plan.
#[cfg(all(test, feature = "query"))]
mod docjson_tests;

// The array/tensor `TensorScan` + `TensorOp` executor proofs (CONCEPT:MEDULLA-KG.storage.content-addressed-dedup): a
// layer scan seeds tensor-bearing nodes, then slice/reduce/elementwise ops apply
// per-row and compose with the graph/vector legs in ONE plan.
#[cfg(all(test, feature = "tensor"))]
mod tensor_tests;

// The event-stream / CEP `Op::Cep` executor proofs (CONCEPT:MEDULLA-KG.query.pipelined-execution): a layer scan
// seeds a time-ordered event stream, then the bounded NFA (sequence/within/absence over
// sliding/tumbling windows) narrows it — cross-modal graph→stream in ONE plan.
#[cfg(all(test, feature = "stream"))]
mod stream_tests;

// The multimodal sensor-fusion `Op::SensorFuse` executor proofs (CONCEPT:MEDULLA-KG.query.multi-rate-sensor-stream): three
// heterogeneous sensor layers (scalar + tensor-frame blob) at different rates are
// resolved off the snapshot, time-aligned to a common clock via medulla-tsdb's ASOF-backed
// `sensor_fuse`, and emitted as fused rows that compose with the graph/vector legs.
#[cfg(all(test, feature = "timeseries"))]
mod sensor_fuse_tests;

// Cross-modal SEAM regression proofs (CONCEPT:MEDULLA-KG.ingest.timeseries-source-mid-pipeline): mid-pipeline composition of a
// semantic (`Reason`), sensor (`SensorFuse`) or tensor (`TensorScan`) SOURCE op feeding
// a downstream `Traverse`/`Rank`/reorder in ONE plan == the hand-wired oracle. Gated at
// the module level on `query`; each proof is additionally `cfg`-gated on its modality
// feature (`owl`/`timeseries`/`tensor`), so the module compiles under any `query` subset.
#[cfg(all(test, feature = "query"))]
mod cross_modal_seam_tests;

// Vector ⇄ reasoning cross-txn write→read consistency (CONCEPT:MEDULLA-KG.compute.concept-6): a freshly
// written node (embedding + reasoner-subsumed type) is BOTH ANN-ranked AND
// reasoner-included by the next `[Reason |> Rank]` plan; an embedding update is reflected
// by the next ANN pass. Needs the OWL reasoner + the vector ranker, so it is gated on `owl`.
#[cfg(all(test, feature = "owl"))]
mod vector_reasoning_tests;

// The probabilistic `Op::Probabilistic` executor proofs (CONCEPT:MEDULLA-KG.compute.uncertainty-values): a layer scan
// seeds distribution-bearing nodes, then a closed-form probabilistic query (expectation /
// marginal / conditional posterior / seeded sample) scores + ranks each row's stored
// `Distribution`, composing uncertainty with the graph/vector legs in ONE plan.
#[cfg(all(test, feature = "probabilistic"))]
mod probabilistic_tests;

// The epistemic belief/evidence executor proofs (CONCEPT:MEDULLA-KG.epistemic.epistemic-substrate,
// E2): `EvidenceFor`/`Contradicts`/`SupportedBy` seed-or-filter by classified edge kind,
// `ConfidenceOp`/`SourceReliability` re-score by propagated belief, `BeliefAsOf` composes the
// bitemporal filter with propagation (diverging from the pure-alias `VALID AS OF` on a
// bitemporal fixture), and `ExplainBelief` flattens the justification tree — composing with
// the graph/vector legs in ONE plan (`[Scan, EvidenceFor, Rank]`).
#[cfg(all(test, feature = "epistemic"))]
mod epistemic_tests;

// Lane C proofs (CONCEPT:MEDULLA-KG.query.native-time-series): mid-pipeline OWL `Op::Reason` (a confidence-preserving
// FILTER, not only a leaf source — `Rank → Reason → Traverse`) + the native medulla-tsdb
// `Op::TsScan` SOURCE (tsdb-in-plan fusion), over a hand-built `PlanCtx`. Compiles when
// EITHER feature is on; each test is additionally gated on the one it needs.
#[cfg(all(test, any(feature = "owl", feature = "timeseries")))]
mod tsdb_scan_tests;

// EH-522 — UQL `DERIVE` end to end (parser → serve → executor) against the kernels.
#[cfg(all(test, feature = "timeseries"))]
mod derive_tests;

// The ADVANCED cross-modal seam proofs (CONCEPT:MEDULLA-KG.compute.asof-valid-t-sources..MEDULLA-389): the richest 3+-modality
// fused plans over a hand-built `PlanCtx` — bitemporal reason→vector→traverse (owl),
// federation fusion + fail-closed named source (federation), geo×vector×temporal (geo),
// tensor×graph×vector + CAS dedup (tensor), CEP×graph×tsdb window (stream+timeseries), and
// probabilistic×owl×vector + MMR (owl+probabilistic). Module-gated on `query`; each proof
// is additionally `cfg`-gated on the modality feature(s) it needs.
#[cfg(all(test, feature = "query"))]
mod advanced_crossmodal_tests;

pub mod document_connection;
