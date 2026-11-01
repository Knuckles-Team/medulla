//! Per-domain graph-op handlers. The dispatch shell (super::dispatch_graph_op)
//! routes each method here; cross-cutting write side-effects (dirty/WAL/gauge)
//! stay centralized in the shell. Each module exposes `try_handle(...) ->
//! Result<Response, Method>`: `Ok` = handled, `Err(method)` = not mine, try next.

// Engine-native WorkItem lifecycle transitions. The handler owns the six
// result-producing transition Methods explicitly; their durable effect remains
// the one MutationBatch commit in `mutation_batch::commit_work_item`.
// RF-020 — authenticated Agent Library delegation lowers through native WorkItem admission.
pub(crate) mod delegation;
pub(crate) mod work_item;
// EH-219 typed WorkItem reads (`GetWorkItem`/`ListWorkItems`): the verified
// carrier tenant rule plus the native redb snapshot read.
#[cfg(feature = "redb")]
pub(crate) mod work_item_read;
// Body-tenant binding for every WorkItem-kernel write and the fleet event
// stream authority (graph-os MEDULLA-5 security ruling).
pub(crate) mod native_write_authority;
// Native development-lane hold/quota authority. All six writes and both reads
// are enumerated in the handler; dispatch does not classify this domain or use
// a wildcard commit arm.
pub(crate) mod development_lane;
// Data-mining domain (CONCEPT:MEDULLA-KG.mining.frequent-itemset-mining, feature `mining`).
// Association-rule mining. GRAPH-SCOPED (unlike finance/datascience): the
// graph-derived source + write-back need the live graph core, so the handler takes
// it like query/rdf. A build without `mining` omits the module and the
// `MineAssociate` variant falls to the graph_ops "not available" catch-all.
#[cfg(feature = "mining")]
pub(crate) mod mining;
// M3 catalog-driven resharding admin (CONCEPT:MEDULLA-KG.backend.m3-admin-dispatch): the wire surface that DRIVES online
// resharding (MEDULLA-032), the tenant catalog (MEDULLA-031) and the rebalance planner (MEDULLA-035) +
// its execution (MEDULLA-039). Always declared; the real logic is `redb`-gated (the only build
// where the catalog/reshard/planner exist), and a non-redb build returns "not available".
pub(crate) mod admin;
// RF-ADR-010 — the Decide layer's four served methods.
pub(crate) mod decide;
// X10 — the mutation-outbox operator surface.
pub(crate) mod mutation_outbox;
// The general bounded 0-1 integer programme.
pub(crate) mod solve;
// Read-only query surface: SQL (CONCEPT:MEDULLA-KG.query.read-only-sql-query, DataFusion behind `query`),
// Cypher (CONCEPT:MEDULLA-KG.query.dep-free-behind, dep-free behind `cypher`), AND GraphQL (CONCEPT:MEDULLA-KG.query.sparql-completeness,
// dep-free behind `graphql`). Present when ANY of those features is on — a cypher- or
// graphql-only Pi build keeps the module WITHOUT DataFusion. With none of them the
// Sql/CypherQuery/GraphQl variants fall to the graph_ops "not available" catch-all.
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
pub(crate) mod query;
// Multi-op OCC ACID transactions (CONCEPT:MEDULLA-KG.txn.multi-op-occ-acid). Always present with the
// `server` feature — the Txn* methods are stateful (need `state`) and not
// feature-gated to a compute domain.
pub(crate) mod txn;
// Native time-series store + primitives (CONCEPT:AU-KG.retrieval.god-nodes-communities/211, feature `tsdb`). The
// Ts* methods are stateful (need the `SeriesStore` on `state`); a slim build without
// `tsdb` omits the module and the variants fall to the graph_ops not-built catch-all.
#[cfg(feature = "tsdb")]
pub(crate) mod timeseries;
// Streamed content-addressed BLOB substrate (CONCEPT:MEDULLA-KG.storage.blob-namespace). Behind `blob`
// (which pulls `redb` + `server`); the stateful Blob* methods drive the CAS store
// + cursors on ServerState. A build without `blob` drops both the module and the
// Method variants, so the dispatch chain never routes them.
#[cfg(feature = "blob")]
pub(crate) mod blob;
// Streaming / CDC / subscriptions / triggers (CONCEPT:MEDULLA-KG.query.streaming-cdc-subscriptions/230, feature
// `streaming`). The Cdc*/ContinuousQuery/Watch/Trigger methods are stateful (they
// drive the `CdcHub` on `state`); a build without `streaming` omits the module and
// the variants fall to the graph_ops not-built catch-all.
#[cfg(feature = "streaming")]
pub(crate) mod streaming;
// Native RDF/SPARQL surface (CONCEPT:MEDULLA-KG.ontology.kg-native-rdf-sparql/218, features `rdf`/`sparql`). The
// AddTriples/GetRdf/Sparql methods are graph-scoped (they target `req.graph`),
// so the handler takes the graph core like the query handler; AddTriples also reads
// the optional lossless quad store off `state`. A build without `rdf` omits the
// module and the variants fall to the graph_ops "not available" catch-all.
#[cfg(feature = "rdf")]
pub(crate) mod rdf;
// Distributed graph compute (CONCEPT:MEDULLA-KG.storage.feature, feature `compute-dist`). The
// DistributedCompute/*MatView methods drive the cross-shard Pregel engine + the
// matview store on ServerState; a non-cluster build omits the module and the variants
// fall to the graph_ops not-available catch-all.
// Present under EITHER `compute-dist` (the algo-only Pregel matview + DistributedCompute)
// OR `matview` (the plan-backed, single-node incremental matview) — the two families share
// this dispatch module but NOT a feature dependency (the plan-backed path uses no raft).
#[cfg(any(feature = "compute-dist", feature = "matview"))]
pub(crate) mod dist_compute;
// Query federation / foreign sources (CONCEPT:MEDULLA-KG.query.query-federation, feature `federation`).
// RegisterForeignSource records a named foreign source under the caller's verified owner
// (tenant+principal, EH-373); the unified-query handlers build the medulla-plan
// `ForeignSourceRegistry` a `Named` `Op::ForeignScan` / an `Op::Foreign` resolves through
// from the CALLER'S OWN entries only (`server::foreign_catalog`), so both the by-name and the
// inline-spec paths execute through the same federation machinery. A build without
// `federation` omits the module and the variant falls to the graph_ops not-available
// catch-all.
#[cfg(feature = "federation")]
pub(crate) mod federation;
// Closed typed SQL source batches append through the native SQL owner gate.
#[cfg(feature = "query")]
pub(crate) mod source_batch;
// RF-ADR-009 native raw-source authority. Present in every server build so a
// build lacking redb/blob can return a named refusal for the wire method.
pub(crate) mod source_ingestion;
// EH-280 — durable projection of a branch-aware IndexRepository batch,
// committed through the same ChangeEnvelope authority as source ingestion.
pub(crate) mod repository_index;
// MEDULLA-UNIFIED-DATA-PLANE-R651.7 / MEDULLA-DECISION-ENGINE-R802 — Document enrichment
// and web page selection; enrichment commits through the same envelope authority.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub(crate) mod web_ingest;

// SQLite `.db` FILE import/export (CONCEPT:MEDULLA-KG.query.medulla-feature/MEDULLA-332, feature `sqlite-file`). The
// ImportSqliteFile/ExportSqliteFile methods move rows between an on-disk `sqlite3` `.db`
// file and the verified caller's owner-scoped `medulla_query::TableStore` (behind `query`), via the pure-Rust
// `medulla-sqlite-format` (no C sqlite). NOT graph-scoped; a build without `sqlite-file` omits
// the module and the variants aren't in the enum (so the dispatch chain never routes them).
#[cfg(feature = "sqlite-file")]
pub(crate) mod sqlite_file;
// Durable analytics-job plane (CONCEPT:INT-P2-1, feature `jobs`). The
// `Method::AnalyticsJob { op }` surface (submit/status/cancel/resume) over
// `medulla-jobs`'s redb-backed `AnalyticsJob` state machine; a submitted job runs
// asynchronously (spawned off the request) and its success commits a provenance'd
// `:Claim`/`:Evidence` pair via `medulla_jobs::claim::commit_result_claim`. NOT
// graph-scoped (own `jobs.redb`, like `TsAppend`/`Kv*`) — self-routes in
// `dispatch.rs` before the per-graph chain, so it needs `state` directly rather
// than a resolved `GraphCore`.
#[cfg(feature = "jobs")]
pub(crate) mod jobs;
// Native finite-state-machine / statechart engine (CONCEPT:INT-P2-2, feature
// `statechart`). The `Method::Statechart { op }` surface (define/instantiate/
// send_event/get_state/list) over `medulla-statechart`'s redb-backed `StatechartDef` +
// `MachineInstance` store; a durable machine instance is just a `(state, context)`
// row, rehydrated on the next event. NOT graph-scoped (own `statecharts.redb`, like
// `AnalyticsJob`/`TsAppend`/`Kv*`) — self-routes in `dispatch.rs` before the per-graph
// chain, so it needs `state` directly (only for `persist_dir`) rather than a resolved
// `GraphCore`.
#[cfg(feature = "statechart")]
pub(crate) mod statechart;
// Native ASR provider surface (GOC-33, `OWNER-VOICE-ASR`, feature `asr-whisper`).
// `Method::Asr { op }` over the whisper-rs/whisper.cpp provider in `medulla-asr-whisper`
// — a direct, non-durable batch-file transcription call. NOT graph-scoped (reads
// no persisted graph state, commits no durable asr.result.v1) — self-routes in
// `dispatch.rs` before the per-graph chain, exactly like `quantum`/`viz`. Needs no
// `state` at all (no persistence of its own).
#[cfg(feature = "asr-whisper")]
pub(crate) mod asr;
// Governed document/image/audio/video serving. The wire carries an ephemeral
// source body, while this handler persists only an encrypted, opaque runtime
// snapshot through the graph mutation gateway.
#[cfg(feature = "modality-serving")]
pub(crate) mod modality;
// RF-019's S1-S6 tiered semantic ingestion queue. Always declared: the module
// carries its own `#![cfg(all(ann-redb, query))]`, the durable semantic owner
// plus the authorized SQL source read port it needs, and the router refuses
// `Method::SemanticIndex` by name in a build without them.
pub(crate) mod semantic_index;
// One governed, bounded Arrow KnowledgeBatch result plane for graph, SQL, RDF,
// vector, time-series, analytics jobs, and cross-modal plans.
#[cfg(feature = "knowledge-batch")]
pub(crate) mod knowledge_stream;
// Raft cluster-membership ADMIN RPC (CONCEPT:MEDULLA-KG.storage.kg-kg-2 — cluster_deployment.md
// §5 item 2): exposes `MultiRaft::add_group_learner`/`change_group_voters`
// (raft/multi.rs) over `Method::RaftAddLearner`/`Method::RaftChangeMembership`. Always
// declared (like `placement`); the real answer is `raft`-gated, and a non-raft build
// answers a clean typed error.
pub(crate) mod raft_admin;
