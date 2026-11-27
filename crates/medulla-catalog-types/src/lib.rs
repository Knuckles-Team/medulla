//! medulla-catalog-types: Connector pack, schema, catalog, source ingestion, lineage and release record types.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: typed-packs-and-catalog (types), ingest-worker (types).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-UNIFIED-DATA-PLANE-R023.2.1 — typed PITR schedule contract to object
// storage, refusing an archiving mode mismatched to its engine, zero
// retention, or a blank cron/destination. No operator wired; installing the
// real schedule is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pitr_schedule;
// RF-ADR-009 — connector MCP pack import: index, entries, framed digests, ops,
// results and the durable import record. Pure serde; the importer and its
// validation rules live in the server.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod connector_pack;
// MEDULLA-TYPED-PACKS-R097.1 — one typed, versioned domain vocabulary (classes,
// enumerations, properties, alignment to an upper ontology), published through
// the pack import boundary above `connector_pack`. Pure data; validation lives
// here, import wiring lands in a later child requirement.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod domain_pack;
// EH-345 — the fleet catalog: discovery observations and operator overrides
// beside the server registry, and the typed projection that joins them with
// connector-pack `AgentComponent`s. Pure serde; the handler lives in the server.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod fleet_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R029.1 — typed schema_context answer shape and
// catalog, refusing to answer for a table absent from the catalog. Pure
// data; wiring the real query entry point is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod schema_context;
// MEDULLA-FEDERATED-QUERY-R722.3 — host-driven bounded foreign-read lifecycle DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod foreign_read;
// MEDULLA-UNIFIED-DATA-PLANE-R026.2.1 — adapter skeleton mapping a typed P0
// baseline-content-digest record to the P0 phase exit artifact, refusing a
// zero record count or an empty digest. The real baseline capture is a
// later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod gramps_baseline_digest;
// MEDULLA-UNIFIED-DATA-PLANE-R033.1 — typed Gramps production-cutover go-ahead
// (pilot phase P6), reusing gramps_pilot_phase's P5 exit and refusing a
// go-ahead that cites the wrong phase, an invalid P5 exit, no approver, or
// a fallback window shorter than the required 30 days. Pure data; the real
// cutover (Postgres replica, nightly export, rehearsed failover) is a
// later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod gramps_cutover_gate;
// MEDULLA-UNIFIED-DATA-PLANE-R026.3.1 — adapter skeleton mapping a typed P3
// proven-gap-fix record (failing test before, passing test after) to the P3
// phase exit artifact, refusing an identical before/after test name. The
// real restore verification (P4) and schema-understanding demonstration
// (P5) are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod gramps_gap_fix;
// MEDULLA-UNIFIED-DATA-PLANE-R019.3.1 — typed conformance-entry constructor for the
// MongoDB/DocumentDB adapter, over the generic R022.1 report entry. Pure
// data; actually running the comparison is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod document_conformance;
// X9 — keyed schema sources (shapes + ontology) on one request graph, and the
// composed digest that identifies the set in force.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_schema;
// MEDULLA-UNIFIED-DATA-PLANE-R760 — the served incident records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod incident;
// RF-ADR-009 — raw connector records, exact manifest mapping reference, cursor
// CAS and the terminal native ingestion receipt. Runtime authority lives in the
// server; these are bounded pure wire types.
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod catalog_fixtures;
// MEDULLA-ORCH-VOCAB-R815.6 -- the served incident record status machine.
pub mod incident_record_machine;

// MEDULLA-UNIFIED-DATA-PLANE-R020.3.1 — typed conformance-entry constructor for the
// Snowflake/BigQuery/DuckDB/Iceberg warehouse adapters, over the generic
// R022.1 report entry. Pure data; actually running the comparison is a later
// child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod warehouse_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R022.3.1 — pure comparison of a change-capture
// replay's resulting row state against a source snapshot, building a
// `dialect_conformance::DialectConformanceEntry`. No live replay stream or
// source connection; running the comparison against a real replay is a
// later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod replay_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R023.3.1 — typed restore-drill result, refusing a
// `data_matched` flag that disagrees with the actual pre/post-restore
// digest comparison. No restore is run yet; automating the real drill is a
// later child.
pub mod catalog_read;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod restore_drill;
