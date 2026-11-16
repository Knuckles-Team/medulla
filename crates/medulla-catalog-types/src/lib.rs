//! medulla-catalog-types: Connector pack, schema, catalog, source ingestion, lineage and release record types.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: typed-packs-and-catalog (types), ingest-worker (types).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-UNIFIED-DATA-PLANE-R019.1 — typed inferred-shape catalog for an attached
// MongoDB/DocumentDB source. Pure data; the native-driver connection and
// change-stream capture are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod document_source_catalog;
// MEDULLA-TYPED-PACKS-R110.2 — open metadata record model: instance header,
// classifications and relationships. Pure serde.
pub mod open_metadata;
// MEDULLA-UNIFIED-DATA-PLANE-R702 and R763.1 — dataset and column lineage.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lineage;
// MEDULLA-UNIFIED-DATA-PLANE-R038.1 — typed metadata contract and acceleration
// policy, refusing a hot-subset predicate declared without source copy
// capability and falling a missing freshness watermark back to a live
// read. Pure data; real discovery, the copy pipeline, and EXPLAIN's
// per-fragment reporting are later children.
pub mod metadata_acceleration;
// MEDULLA-UNIFIED-DATA-PLANE-R027.1 — typed Postgres compatibility-feature
// inventory and fix-proof shape, refusing a fix recorded without a complete
// captured-traffic/failing-test/passing-test proof. Pure data;
// implementing each proven feature is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pg_compat_feature;
// MEDULLA-UNIFIED-DATA-PLANE-R028.1 — typed Postgres-equivalence differential
// deviation baseline, refusing a baseline update that drops a deviation
// without an explicit resolution record (no self-updating baseline). Pure
// data; running the regression/SQLancer/captured-traffic corpora is a
// later child.
pub mod pg_diff_baseline;
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
// MEDULLA-UNIFIED-DATA-PLANE-R023.2.1 — typed PITR schedule contract to object
// storage, refusing an archiving mode mismatched to its engine, zero
// retention, or a blank cron/destination. No operator wired; installing the
// real schedule is a later child.
pub mod pitr_schedule;
// RF-ADR-009 — connector MCP pack import: index, entries, framed digests, ops,
// results and the durable import record. Pure serde; the importer and its
// validation rules live in the server.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod connector_pack;
// MEDULLA-UNIFIED-DATA-PLANE-R031.1 — typed consolidation-isolation admission:
// per-application quota and upgrade-coupling policy, with the empty-identity
// and all-zero-quota refusal. Pure data; real fault isolation and quota
// metering are later children.
pub mod consolidation_isolation;
// MEDULLA-TYPED-PACKS-R802.1/R805.1 — governance record models: stewardship and
// data products. Pure data and field validation; served wiring lands in later children.
pub mod data_product;
// MEDULLA-UNIFIED-DATA-PLANE-R022.1 — typed per-dialect conformance report entry
// and deviation shape, refusing an auto-suppressed (unreviewed) deviation.
// Pure data; running the containerized matrix is a later child.
pub mod dialect_conformance;
// MEDULLA-TYPED-PACKS-R097.1 — one typed, versioned domain vocabulary (classes,
// enumerations, properties, alignment to an upper ontology), published through
// the pack import boundary above `connector_pack`. Pure data; validation lives
// here, import wiring lands in a later child requirement.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod domain_pack;
// MEDULLA-UNIFIED-DATA-PLANE-R040.1/.2 — per-modality embedding promotion gate.
pub mod embedding_promotion;
// MEDULLA-ENGINE-PARITY-R020.1 — relational engine capability report.
pub mod engine_capabilities;
// MEDULLA-UNIFIED-DATA-PLANE-R030.1 — typed cross-application entity-link
// proposal, refusing an owl:sameAs assertion before approval or for a
// non-Match classification. Pure data; blocking, field-similarity scoring
// and the real Fellegi-Sunter model are later children.
pub mod entity_link_proposal;
pub mod external_schema;
// EH-345 — the fleet catalog: discovery observations and operator overrides
// beside the server registry, and the typed projection that joins them with
// connector-pack `AgentComponent`s. Pure serde; the handler lives in the server.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod fleet_catalog;
// MEDULLA-FEDERATED-QUERY-R724 — authorization-segmented schema boundary records
// and the `Method::SchemaProjections` ops.
pub mod schema_boundary;
// MEDULLA-UNIFIED-DATA-PLANE-R029.1 — typed schema_context answer shape and
// catalog, refusing to answer for a table absent from the catalog. Pure
// data; wiring the real query entry point is a later child.
pub mod schema_context;
// MEDULLA-UNIFIED-DATA-PLANE-R004 — bounded, policy-gated column-sampling budget
// for deterministic schema inference.
pub mod schema_inference;
// MEDULLA-UNIFIED-DATA-PLANE-R023.1 — typed shared CloudNativePG/MariaDB cluster
// group and per-application role-set shape, refusing a Postgres group
// without logical wal_level and an aliased application role. Pure data;
// operator wiring, PITR scheduling and the restore drill are later children.
pub mod shared_db_platform;
// MEDULLA-FEDERATED-QUERY-R722.3 — host-driven bounded foreign-read lifecycle DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod foreign_read;
// MEDULLA-TYPED-PACKS-R801.1 — business glossary term model (SKOS projection).
pub mod glossary;
// MEDULLA-UNIFIED-DATA-PLANE-R026.2.1 — adapter skeleton mapping a typed P0
// baseline-content-digest record to the P0 phase exit artifact, refusing a
// zero record count or an empty digest. The real baseline capture is a
// later child.
pub mod gramps_baseline_digest;
// MEDULLA-UNIFIED-DATA-PLANE-R033.1 — typed Gramps production-cutover go-ahead
// (pilot phase P6), reusing gramps_pilot_phase's P5 exit and refusing a
// go-ahead that cites the wrong phase, an invalid P5 exit, no approver, or
// a fallback window shorter than the required 30 days. Pure data; the real
// cutover (Postgres replica, nightly export, rehearsed failover) is a
// later child.
pub mod gramps_cutover_gate;
// MEDULLA-UNIFIED-DATA-PLANE-R026.3.1 — adapter skeleton mapping a typed P3
// proven-gap-fix record (failing test before, passing test after) to the P3
// phase exit artifact, refusing an identical before/after test name. The
// real restore verification (P4) and schema-understanding demonstration
// (P5) are later children.
pub mod gramps_gap_fix;
// MEDULLA-UNIFIED-DATA-PLANE-R026.1 — typed Gramps native-hosting pilot phase
// gate (P0-P5), refusing a wrong or unreviewed exit artifact and skipping
// ahead of an un-exited earlier phase. Pure data; running each phase is a
// later child.
pub mod gramps_pilot_machine;
pub mod gramps_pilot_phase;
// MEDULLA-UNIFIED-DATA-PLANE-R013.1 — Postgres attached-source adapter config and
// type-mapping typed model (connect/catalog/capture wiring is a later slice).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod postgres_adapter;
// CONCEPT:MEDULLA-KG.compute.uncertainty-values — probabilistic / uncertainty VALUE (distribution-valued
// properties). A stored value at the bottom of the DAG, NOT a wire `Op`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod distribution;
// MEDULLA-UNIFIED-DATA-PLANE-R019.3.1 — typed conformance-entry constructor for the
// MongoDB/DocumentDB adapter, over the generic R022.1 report entry. Pure
// data; actually running the comparison is a later child.
pub mod document_conformance;
// GOC-10 — canonical SQL table / lake catalog authority wire types
// (`CatalogEntry`/`TableSchemaVersion`/`PartitionManifest`/`LakeSnapshot`/
// `QualityReportRef`/`TableChange`). Pure serde, no dep — unconditional like
// `acl`/`mutation_batch`, since it adds no `protocol::Method` variant (the durable
// store + REST projection that would carry these records over the wire is
// GOC-10-W03/W05, not yet implemented).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lake_catalog;
// X9 — keyed schema sources (shapes + ontology) on one request graph, and the
// composed digest that identifies the set in force.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_schema;
// MEDULLA-UNIFIED-DATA-PLANE-R032.1 — typed Immich connector pilot phase gate
// (I0-I6), refusing a wrong or unreviewed exit artifact and skipping ahead
// of an un-exited earlier phase. Pure data; running each phase is a later
// child.
pub mod immich_pilot_machine;
pub mod immich_pilot_phase;
// MEDULLA-UNIFIED-DATA-PLANE-R760 — the served incident records.
pub mod incident;
// RF-ADR-009 — raw connector records, exact manifest mapping reference, cursor
// CAS and the terminal native ingestion receipt. Runtime authority lives in the
// server; these are bounded pure wire types.
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod catalog_fixtures;
// MEDULLA-UNIFIED-DATA-PLANE-R003 — the typed, hashed attached-source catalog graph
// (tables/columns) and its ambiguous-table refusal.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod attached_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R014.1 — typed model distinguishing the MySQL and
// MariaDB attached-source dialect entries. See module docs.
pub mod attached_source_dialect;
// MEDULLA-UNIFIED-DATA-PLANE-R012.1 — typed model for governed attached-source write-back.
pub mod attached_source_governance;
// MEDULLA-UNIFIED-DATA-PLANE-R014.2.3 -- the catalog probe state machine.
pub mod probe_machine;
// MEDULLA-UNIFIED-DATA-PLANE-R761.1 — column profile record.
pub mod profile;
// MEDULLA-UNIFIED-DATA-PLANE-R762.1 — quality rules and scores.
pub mod quality;
// MEDULLA-UNIFIED-DATA-PLANE-R015.1 — typed SQLite attached-source catalog model:
// WITHOUT ROWID handling and its WAL-tailing/watermark capture-mode
// refusal. Pure data; the live read-only attach and real capture are later
// children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sqlite_attached_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R015.2.1 — SQLite live-attach adapter skeleton:
// typed attach config → read-only driver open config, refusing an
// immutable open mode paired with a change-capture mode. No file I/O; real
// WAL-frame tailing/watermark polling is R015.2.2+.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sqlite_live_attach;
pub mod stewardship;
// MEDULLA-UNIFIED-DATA-PLANE-R038.6.1 — read-only property-graph view over mappings.
pub mod virtual_graph_view;
// MEDULLA-UNIFIED-DATA-PLANE-R020.3.1 — typed conformance-entry constructor for the
// Snowflake/BigQuery/DuckDB/Iceberg warehouse adapters, over the generic
// R022.1 report entry. Pure data; actually running the comparison is a later
// child.
pub mod warehouse_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R020.1 — typed federated warehouse/lake source-kind
// model (Snowflake, BigQuery, DuckDB, Iceberg) and its per-kind required-field
// refusal. Pure data; query pushdown and the conformance entry are later
// children.
pub mod warehouse_federation;
pub mod web_source;
// MEDULLA-UNIFIED-DATA-PLANE-R016.1 — typed SQL Server attached-source catalog
// model: two-part schema.table entries and the bracket-quoting identifier
// refusal. Pure data; the tiberius connection and T-SQL rendering are later
// children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mssql_attached_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R016.2.1 — MSSQL adapter skeleton: typed connection
// config → tiberius-shaped driver config, with a connect refusal path that
// needs no live server. T-SQL rendering and LSN-polled CDC/Change Tracking
// capture are R016.2.2+.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mssql_driver_connection;
// MEDULLA-UNIFIED-DATA-PLANE-R014.2.1 — MySQL/MariaDB adapter skeleton: typed
// connection config → sqlx-shaped driver config, with a connect refusal
// path that needs no live database. Binlog/GTID capture and the per-engine
// conformance entries are R014.2.2+.
pub mod mysql_mariadb_connection;
pub mod ontology_diff;
// MEDULLA-TYPED-PACKS-R651.1 — append-only ontology edit chain records.
pub mod ontology_edit;
// Typed RDF input and the ontology inspection view, so orchestration callers
// never build or parse RDF text themselves.
pub mod ontology_inspection;
// MEDULLA-UNIFIED-DATA-PLANE-R017.1 — typed ClickHouse attached-source catalog
// model, with its change-capture capability always reported as the
// explicit Unsupported value. Pure data; the HTTP/native client connection
// is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod clickhouse_attached_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R018.1 — typed Oracle/Db2 attached-source catalog
// model: double-quoted identifier refusal and the Debezium-captured-table
// primary-key refusal. Pure data; the driver/ODBC connection and Debezium
// bridge wiring are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod relational_attached_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R022.3.1 — pure comparison of a change-capture
// replay's resulting row state against a source snapshot, building a
// `dialect_conformance::DialectConformanceEntry`. No live replay stream or
// source connection; running the comparison against a real replay is a
// later child.
pub mod replay_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R023.3.1 — typed restore-drill result, refusing a
// `data_matched` flag that disagrees with the actual pre/post-restore
// digest comparison. No restore is run yet; automating the real drill is a
// later child.
pub mod restore_drill;
// MEDULLA-CQ-ORCH-R010.1 — pure data model for graph-derived architecture views.
pub mod architecture_view;
// MEDULLA-UNIFIED-DATA-PLANE-R764.1/.2 — asset usage and catalog search hits.
pub mod asset_usage;
