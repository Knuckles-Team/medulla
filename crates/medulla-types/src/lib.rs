//! medulla-types — the wire protocol + graph data model. Bottom of the engine crate
//! DAG: it depends only on serde. Everything else (`medulla-core`, `medulla-compute`,
//! `medulla-server`, the `medulla` facade) depends on it, never the reverse.
//!
//! Domain wire types live in their slices: finance DTOs in `medulla-finance-types`,
//! ingestion records in `medulla-ingestion-types`, the access-control vocabulary
//! (`AgentRole`/`AgentIdentity`) in `medulla-identity-types`.

pub mod row_change;
// MEDULLA-DECISION-ENGINE-R656.1 — the assistant profile and document-set wire model.
pub mod semantic_enrichment;
// MEDULLA-FINANCE-PRIMITIVES-R002.2.5 — served numeric kernel evaluation wire types.
pub mod numeric_kernel;
// MEDULLA-FINANCE-PRIMITIVES-R002.11 — the regime forecast, calibration gate and sizing input.
pub mod regime_forecast;
// MEDULLA-ORCH-VOCAB-R811 — the core state-machine contract: machine definition,
// the one generic transition function and the typed `TransitionRefused`.
pub mod profile_grant_machine;
pub mod refusal_code;
pub mod reservation_machine;
pub mod schedule_machine;
// MEDULLA-INCREMENTAL-VIEWS-R016.1 — the materialization status as one explicit state machine.
pub mod materialization_status_machine;
pub mod edit_machine;
pub mod gap_machine;
// MEDULLA-DATAPLAT-VOCAB-R009 — the data-platform lifecycles as explicit state machines.
pub mod access_request_machine;
pub mod change_request_machine;
pub mod durable_unit_machine;
pub mod glossary_term_machine;
pub mod ontology_proposal_machine;
pub mod transition_refused;
// MEDULLA-FEDERATED-QUERY-R005.700 — graph integrity shapes and their report.
pub mod shape_report;
// MEDULLA-INGEST-WORKER-R002.1/.2 — induced ontology proposals and admission scores.
pub mod ontology_base;
// MEDULLA-INGEST-WORKER-R604.1 — ingest lanes, queue classes and lane metrics.
pub mod ingest_lanes;
// MEDULLA-INGEST-WORKER-R900.4/R901.4/R902.4/R903.4/R904.4/R701.2/R701.4/R002.4/R003.4 — pure ingest refusal rules.
pub mod dashboard;
pub mod corpus_retrieval;
pub mod ingest_materialize;
// MEDULLA-FEDERATED-QUERY-R654.1 — served CodeContext and Diagnose wire records.
pub mod code_context;
pub mod participant;
pub mod media_tag;
// MEDULLA-FEDERATED-QUERY-R039.2.7 — chunk-encoding receipt and the served encoded-document types.
pub mod retrieval;
// MEDULLA-FEDERATED-QUERY-R081.101.1 — the reusable compressed document state.
pub mod retrieval_round;
// MEDULLA-FEDERATED-QUERY-R081.103.1 — viewport query wire types.
pub mod evidence_class;
pub mod unified_row;
pub mod viewport;
// RF-RULING-004 — bounded scalar/collection primitives shared by kernel DTOs.
pub mod contract;
pub mod contract_catalog;
// MEDULLA-INCREMENTAL-VIEWS-R002.1 — the signed-row Z-set delta of incremental views.
pub mod delta;
// MEDULLA-DURABLE-KERNEL-R038 — the declared durability class (ephemeral/async/sync)
// a KV namespace or table carries. Pure serde, no dep.
pub mod deployment_profile;
pub mod durability_class;

// MEDULLA-DURABLE-KERNEL-R094.1 — hierarchical concept-ID grammar and path claims.
pub mod concept_id;

#[cfg(feature = "modality-serving")]
pub mod modality;
pub mod msgpack;
// X10 — the operator view of one owner's mutation outbox: standing, dead
// letters and bounded re-delivery.
pub mod lease_vocab;
// MEDULLA-FINANCE-PRIMITIVES-R902 — probabilistic forecast samples, never a kernel input.
pub mod finance_forecast;
// MEDULLA-FINANCE-PRIMITIVES-R013.2.6 — the accounting projection read by
// `Method::FinanceAccountProject`.
pub mod finance_accounting;
pub mod runtime_limit;
// MEDULLA-FEDERATED-QUERY-R607.1 — topology analytics result model.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod types;
// The opaque tenant-bound keyset cursor shared by every paged read
// (`AgentComponent.Search`, `ListWorkItems`).
pub mod tenant_cursor;
// The three-bound keyset page shared by the tenant-bound native listings.
pub mod keyset_page;
pub mod projection_read;

// CONCEPT:MEDULLA-KG.query.compound-predicate-decode — the serializable compound-WHERE predicate AST lives at the
// bottom of the DAG so `medulla-core` can evaluate it; `medulla-query` decodes SQL into it.
// CONCEPT:MEDULLA-KG.compute.uncertainty-values — surface the distribution VALUE at the crate root for callers.
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
// MEDULLA-ORCH-VOCAB-R009.700 — a durable run unit mirrored as a run-trace node.
pub mod durable_unit;
// MEDULLA-REPO-INGEST-R002.100: independent ingest readback receipt.
pub mod code_map;
pub mod ingest_throughput;
// MEDULLA-INGEST-WORKER-R911.3: the ingest stage machine.
pub mod ingest_machine;
// MEDULLA-INGEST-WORKER-R911.4: the throughput window trust machine.
pub mod throughput_machine;
// MEDULLA-CQ-ORCH-R701.1: canonical workflow records of every process source.
pub mod process_import;
// MEDULLA-CQ-ORCH-R702.1: process document records (BPMN 2.0, DMN 1.3).
pub mod process_exchange;
// MEDULLA-CQ-ORCH-R705.2: architecture exchange records (ArchiMate Open Exchange Format).
pub mod archimate_exchange;
// MEDULLA-FEDERATED-QUERY-R738.1 — the capability matrix entry status (planned until its backend lands, then shipped) as an explicit state machine.
pub mod capability_machine;
// MEDULLA-FEDERATED-QUERY-R738.3 — the lifecycle of a research claim as an explicit state machine.
pub mod claim_machine;
// MEDULLA-FEDERATED-QUERY-R738.5 — the status of a tracked contradiction: open, proposed, resolved in that order only as an explicit state machine.
pub mod contradiction_machine;
// MEDULLA-FEDERATED-QUERY-R738.7 — the status of a failure-pattern gap topic as an explicit state machine.
pub mod gap_topic_machine;
pub mod quality_rule_machine;
// MEDULLA-FEDERATED-QUERY-R738.8 — the lifecycle status of a geolocated task as an explicit state machine.
pub mod geo_task_machine;
