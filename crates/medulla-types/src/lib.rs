//! medulla-types — the wire protocol + graph data model. Bottom of the engine crate
//! DAG: it depends only on serde. Everything else (`medulla-core`, `medulla-compute`,
//! `medulla-server`, the `medulla` facade) depends on it, never the reverse.
//!
//! It also owns the pure-data types that the protocol enum embeds but whose
//! behavior lives upstream: `wire` (finance/datascience DTOs) and `acl`
//! (`AgentRole`/`AgentIdentity`). The upstream modules re-export these
//! (`pub use medulla_types::wire::Order;`) so their algorithm code is unchanged —
//! the data lives at the bottom of the DAG, the logic stays where it belongs.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them

pub mod acl;
// MEDULLA-CQ-ORCH-R010.1 — pure data model for graph-derived architecture views.
pub mod architecture_view;
// MEDULLA-UNIFIED-DATA-PLANE-R012.1 — typed model for governed attached-source write-back.
pub mod attached_source_governance;
// MEDULLA-UNIFIED-DATA-PLANE-R014.1 — typed model distinguishing the MySQL and
// MariaDB attached-source dialect entries. See module docs.
pub mod attached_source_dialect;
// MEDULLA-UNIFIED-DATA-PLANE-R014.2.1 — MySQL/MariaDB adapter skeleton: typed
// connection config → sqlx-shaped driver config, with a connect refusal
// path that needs no live database. Binlog/GTID capture and the per-engine
// conformance entries are R014.2.2+.
pub mod mysql_mariadb_connection;
// CONCEPT:MEDULLA-KG.compute.native-asr-whisper-provider — the native-ASR agent-facing wire op
// (`AsrOp`), gated `asr-native`. Lives here (not in `medulla-asr-whisper`), for the
// SAME reason `quantum.rs` lives here rather than in `medulla-quantum-core`: `medulla-types`
// is the bottom-of-DAG crate every wire consumer already depends on, while the
// heavy provider crate sits above the facade. Pure serde — no dep on `medulla-audio`/
// `medulla-asr-whisper`.
#[cfg(feature = "asr-native")]
pub mod asr_wire;
// MEDULLA-DURABLE-KERNEL-R076.1 — authoritative scheduler policy data and validation.
pub mod scheduler_config;
// MEDULLA-INGEST-WORKER-R002.1/.2 — induced ontology proposals and admission scores.
pub mod ontology_proposal;
// MEDULLA-INGEST-WORKER-R003.1 — typed knowledge atoms with mandatory provenance.
pub mod knowledge_atom;
// MEDULLA-INGEST-WORKER-R701.1 — extracted facts, candidate claims and evidence spans.
pub mod extracted_fact;
// MEDULLA-INGEST-WORKER-R900.1/R902.1/R903.1/R904.1 — source governance records.
pub mod source_governance;
// MEDULLA-INGEST-WORKER-R900.4/R901.4/R902.4/R903.4/R904.4/R701.2/R701.4/R002.4/R003.4 — pure ingest refusal rules.
pub mod ingest_rules;
// MEDULLA-TYPED-PACKS-R803.1 — typed sensitive-data detector and detection records.
pub mod sensitive_detect;
// MEDULLA-TYPED-PACKS-R804.1 — ontology revision difference and impact records.
pub mod code_health;
pub mod ontology_diff;
// MEDULLA-UNIFIED-DATA-PLANE-R019.3.1 — typed conformance-entry constructor for the
// MongoDB/DocumentDB adapter, over the generic R022.1 report entry. Pure
// data; actually running the comparison is a later child.
pub mod document_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R020.1 — typed federated warehouse/lake source-kind
// model (Snowflake, BigQuery, DuckDB, Iceberg) and its per-kind required-field
// refusal. Pure data; query pushdown and the conformance entry are later
// children.
pub mod warehouse_federation;
// MEDULLA-UNIFIED-DATA-PLANE-R020.3.1 — typed conformance-entry constructor for the
// Snowflake/BigQuery/DuckDB/Iceberg warehouse adapters, over the generic
// R022.1 report entry. Pure data; actually running the comparison is a later
// child.
pub mod warehouse_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R022.1 — typed per-dialect conformance report entry
// and deviation shape, refusing an auto-suppressed (unreviewed) deviation.
// Pure data; running the containerized matrix is a later child.
pub mod dialect_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R022.3.1 — pure comparison of a change-capture
// replay's resulting row state against a source snapshot, building a
// `dialect_conformance::DialectConformanceEntry`. No live replay stream or
// source connection; running the comparison against a real replay is a
// later child.
pub mod replay_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R023.1 — typed shared CloudNativePG/MariaDB cluster
// group and per-application role-set shape, refusing a Postgres group
// without logical wal_level and an aliased application role. Pure data;
// operator wiring, PITR scheduling and the restore drill are later children.
pub mod shared_db_platform;
// MEDULLA-UNIFIED-DATA-PLANE-R023.2.1 — typed PITR schedule contract to object
// storage, refusing an archiving mode mismatched to its engine, zero
// retention, or a blank cron/destination. No operator wired; installing the
// real schedule is a later child.
pub mod pitr_schedule;
// MEDULLA-UNIFIED-DATA-PLANE-R023.3.1 — typed restore-drill result, refusing a
// `data_matched` flag that disagrees with the actual pre/post-restore
// digest comparison. No restore is run yet; automating the real drill is a
// later child.
pub mod restore_drill;
// MEDULLA-UNIFIED-DATA-PLANE-R025.1 — typed pgrx companion-extension spike
// decision record, refusing an outcome recorded without evidence for every
// declared scope area. Pure data; running the spike and the ADR review are
// later children.
pub mod pgrx_spike;
// MEDULLA-UNIFIED-DATA-PLANE-R025.3.1 — adapter skeleton reviewing a recorded
// pgrx spike decision into an approved ADR record, refusing an invalid
// decision, an empty reviewer, or an empty ADR title. Filing the ADR
// document itself is a later child.
pub mod pgrx_spike_review;
// MEDULLA-UNIFIED-DATA-PLANE-R026.1 — typed Gramps native-hosting pilot phase
// gate (P0-P5), refusing a wrong or unreviewed exit artifact and skipping
// ahead of an un-exited earlier phase. Pure data; running each phase is a
// later child.
pub mod gramps_pilot_phase;
// MEDULLA-UNIFIED-DATA-PLANE-R026.2.1 — adapter skeleton mapping a typed P0
// baseline-content-digest record to the P0 phase exit artifact, refusing a
// zero record count or an empty digest. The real baseline capture is a
// later child.
pub mod gramps_baseline_digest;
// MEDULLA-UNIFIED-DATA-PLANE-R026.3.1 — adapter skeleton mapping a typed P3
// proven-gap-fix record (failing test before, passing test after) to the P3
// phase exit artifact, refusing an identical before/after test name. The
// real restore verification (P4) and schema-understanding demonstration
// (P5) are later children.
pub mod gramps_gap_fix;
// MEDULLA-UNIFIED-DATA-PLANE-R033.1 — typed Gramps production-cutover go-ahead
// (pilot phase P6), reusing gramps_pilot_phase's P5 exit and refusing a
// go-ahead that cites the wrong phase, an invalid P5 exit, no approver, or
// a fallback window shorter than the required 30 days. Pure data; the real
// cutover (Postgres replica, nightly export, rehearsed failover) is a
// later child.
pub mod gramps_cutover_gate;
// MEDULLA-UNIFIED-DATA-PLANE-R038.1 — typed metadata contract and acceleration
// policy, refusing a hot-subset predicate declared without source copy
// capability and falling a missing freshness watermark back to a live
// read. Pure data; real discovery, the copy pipeline, and EXPLAIN's
// per-fragment reporting are later children.
pub mod metadata_acceleration;
// MEDULLA-UNIFIED-DATA-PLANE-R040.1/.2 — per-modality embedding promotion gate.
pub mod embedding_promotion;
// MEDULLA-UNIFIED-DATA-PLANE-R038.6.1 — read-only property-graph view over mappings.
pub mod virtual_graph_view;
// MEDULLA-UNIFIED-DATA-PLANE-R761.1 — column profile record.
pub mod profile;
// MEDULLA-UNIFIED-DATA-PLANE-R762.1 — quality rules and scores.
pub mod quality;
// MEDULLA-UNIFIED-DATA-PLANE-R764.1/.2 — asset usage and catalog search hits.
pub mod asset_usage;
// MEDULLA-ENGINE-PARITY-R020.1 — relational engine capability report.
pub mod engine_capabilities;
// MEDULLA-UNIFIED-DATA-PLANE-R028.1 — typed Postgres-equivalence differential
// deviation baseline, refusing a baseline update that drops a deviation
// without an explicit resolution record (no self-updating baseline). Pure
// data; running the regression/SQLancer/captured-traffic corpora is a
// later child.
pub mod pg_diff_baseline;
// MEDULLA-UNIFIED-DATA-PLANE-R029.1 — typed schema_context answer shape and
// catalog, refusing to answer for a table absent from the catalog. Pure
// data; wiring the real query entry point is a later child.
pub mod schema_context;
// MEDULLA-FEDERATED-QUERY-R058.2.2.1 — the disclosed NL query result (executed UQL + rows).
pub mod nl_query;
// MEDULLA-UNIFIED-DATA-PLANE-R030.1 — typed cross-application entity-link
// proposal, refusing an owl:sameAs assertion before approval or for a
// non-Match classification. Pure data; blocking, field-similarity scoring
// and the real Fellegi-Sunter model are later children.
pub mod entity_link_proposal;
// RF-RULING-004 — bounded scalar/collection primitives shared by kernel DTOs.
pub mod contract;
// MEDULLA-DURABLE-KERNEL-R032 — the native primary-key fast-path routing decision
// (pgwire/MySQL/MSSQL point operations that bypass DataFusion planning).
// Pure serde, no dep.
pub mod fast_path;
// MEDULLA-DURABLE-KERNEL-R033 — the hot-store engine evaluation's typed model
// (redb/fjall/RocksDB candidates, workload profiles, validated benchmark
// results, winner selection). Pure serde, no dep.
pub mod hot_store_engine;
// MEDULLA-DURABLE-KERNEL-R034 — cross-store atomicity's typed transaction-scope
// model: a single-store scope structurally can never map to the cross-store
// commit path. Pure serde, no dep.
pub mod cross_store_scope;
// MEDULLA-DURABLE-KERNEL-R035 — the PostgreSQL wire protocol's required
// operational surfaces and a fail-closed compatibility matrix. Pure serde,
// no dep.
pub mod pg_operational_surface;
// MEDULLA-DURABLE-KERNEL-R039 — in-memory structure/atomic-primitive kinds and a
// durability declaration that always names redb as sole durability
// authority. Pure serde, no dep.
pub mod atomic_primitive;
// MEDULLA-DURABLE-KERNEL-R040 — pipelined RESP/pgwire-extended batch admission:
// one admission maps to exactly one commit with order-preserving replies.
// Pure serde, no dep.
pub mod pipeline_batch;
// MEDULLA-DURABLE-KERNEL-R038 — the declared durability class (ephemeral/async/sync)
// a KV namespace or table carries. Pure serde, no dep.
pub mod durability_class;
// MEDULLA-DURABLE-KERNEL-R084.1 — typed optimistic-write conflict and the closed
// set of scope-level merge strategies. Resolution and served wiring live above
// this pure-data crate.
pub mod write_conflict;
pub use write_conflict::{MergeStrategy, WriteConflict};
// MEDULLA-DURABLE-KERNEL-R037 — the workload classification (point/structure/sql)
// every wire listener routes an operation by. Pure, no dep.
pub mod workload_class;

// MEDULLA-DURABLE-KERNEL-R089.1 — per-step verification verdict. Pure, no dep.
pub mod step_verdict;
// MEDULLA-DURABLE-KERNEL-R088.1 — rule-output correction with two-principal approval.
pub mod rule_correction;
// MEDULLA-DURABLE-KERNEL-R852.1/.2/.4 — health anomalies and correlated incidents.
pub mod health_incident;
// MEDULLA-DURABLE-KERNEL-R700.1 — the retention-cleanup policy shape.
pub mod retention_policy;
// MEDULLA-DURABLE-KERNEL-R094.1 — hierarchical concept-ID grammar and path claims.
pub mod concept_id;

// MEDULLA-DURABLE-KERNEL-R041 — the SQL plan cache key (statement digest + schema
// version) and its lookup decision. Pure, no dep beyond `contract::Digest256`.
pub mod plan_cache_key;
// MEDULLA-DURABLE-KERNEL-R042 — a published benchmark report's typed superiority
// claim and the refusal for an unpublished-configuration or non-passing one.
pub mod benchmark_claim;
// MEDULLA-DURABLE-KERNEL-R036 — the mirror-sink side: sink kind (fan-out/postgres)
// and durable per-mirror cursor with a non-empty-position guard. Pure serde, no dep.
pub mod ingestion_wire;
pub mod mirror_sink;
// CONCEPT:INT-P2-2 — the native statechart engine's wire op (`StatechartOp`), gated
// `statechart`. Lives here (not in `medulla-statechart`, which sits ABOVE this crate in the
// DAG) for the SAME reason `jobs` does: `protocol::Method::Statechart` carries it over
// the wire, and `protocol` is bottom-of-DAG. Pure serde — no dep.
#[cfg(feature = "knowledge-batch")]
pub mod knowledge_stream;
#[cfg(feature = "modality-serving")]
pub mod modality;
pub mod msgpack;
// X10 — the operator view of one owner's mutation outbox: standing, dead
// letters and bounded re-delivery.
pub mod lease_vocab;
// MEDULLA-FINANCE-PRIMITIVES-R011.1 — the typed finance alert envelope staged for
// the finance outbox topic family; net-new slice (model + validation +
// refusal tests only, no outbox-append wiring yet).
#[cfg(feature = "finance")]
pub mod finance_event_envelope;
pub mod solve_scalar;
// CONCEPT:MEDULLA-KG.compute.quantum-agent-api — the agent-facing quantum control-plane
// wire op (`QuantumOp`), gated `quantum`. Lives here (not in `medulla-quantum-core`,
// which sits ABOVE this crate in the DAG) for the SAME reason `jobs`/`statechart`
// do: `protocol::Method::Quantum` carries it over the wire, and `protocol` is
// bottom-of-DAG. Pure serde — no dep on `medulla-quantum-core`.
#[cfg(feature = "quantum")]
pub mod quantum;
// Typed RDF input and the ontology inspection view, so orchestration callers
// never build or parse RDF text themselves.
pub mod ontology_inspection;
// F3: RDF load/update/rule/shape-validation report bodies (medulla-rdf/medulla-shacl/medulla-shex results).
pub mod rdf_report;
pub mod runtime_limit;
// EH-522 — the series-expression algebra (`DERIVE` columns, derived series). Pure serde.
pub mod series_expr;
// MEDULLA-SCM-R002 — the source changeset record and its structured trailers.
pub mod source_changeset;
pub mod source_symbol_edit;
#[cfg(feature = "statechart")]
pub mod statechart;
pub mod telemetry_derive;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod types;
// The opaque tenant-bound keyset cursor shared by every paged read
// (`AgentComponent.Search`, `ListWorkItems`).
pub mod tenant_cursor;
// The three-bound keyset page shared by the tenant-bound native listings.
pub mod keyset_page;
// MEDULLA-TYPED-PACKS-R802.1/R805.1 — governance record models: stewardship and
// data products. Pure data and field validation; served wiring lands in later children.
pub mod data_product;
pub mod stewardship;
// MEDULLA-UNIFIED-DATA-PLANE-R004 — bounded, policy-gated column-sampling budget
// for deterministic schema inference.
pub mod schema_inference;
// MEDULLA-UNIFIED-DATA-PLANE-R031.1 — typed consolidation-isolation admission:
// per-application quota and upgrade-coupling policy, with the empty-identity
// and all-zero-quota refusal. Pure data; real fault isolation and quota
// metering are later children.
pub mod consolidation_isolation;
// MEDULLA-UNIFIED-DATA-PLANE-R032.1 — typed Immich connector pilot phase gate
// (I0-I6), refusing a wrong or unreviewed exit artifact and skipping ahead
// of an un-exited earlier phase. Pure data; running each phase is a later
// child.
pub mod immich_pilot_phase;
pub mod usage;
#[cfg(feature = "viz")]
pub mod viz;
pub mod wire;

// CONCEPT:MEDULLA-KG.query.compound-predicate-decode — the serializable compound-WHERE predicate AST lives at the
// bottom of the DAG so `medulla-core` can evaluate it; `medulla-query` decodes SQL into it.
// CONCEPT:MEDULLA-KG.compute.uncertainty-values — surface the distribution VALUE at the crate root for callers.
#[cfg(feature = "knowledge-batch")]
pub use knowledge_stream::{
    KnowledgeResultFamily, KnowledgeStreamBatch, KnowledgeStreamCursor, KnowledgeStreamProjection,
    KnowledgeStreamQuery, KnowledgeStreamRequest, KNOWLEDGE_STREAM_SCHEMA_VERSION,
};
#[cfg(feature = "modality-serving")]
pub use modality::{
    ServedModalityIngestItem, ServedModalityKind, ServedModalityOp, ServedNativePredicate,
    ServedSegmentKind,
};
// MutationBatch v1 replaces the (version_scope, source_graph_version) pair with the
// closed `CommittedVersion` enum, so a scope without its matching version is no longer
// representable. `MutationVersionScope` and `NON_GRAPH_SOURCE_VERSION` are therefore
// not re-exported: they have no v1 meaning, and keeping an alias would be exactly the
// compatibility seam RF-ADR-001 forbids.
