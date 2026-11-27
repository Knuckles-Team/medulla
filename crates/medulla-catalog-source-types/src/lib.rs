//! medulla-catalog-source-types: Connector pack, schema, catalog, source ingestion, lineage and release record types.
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
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod metadata_acceleration;
// MEDULLA-UNIFIED-DATA-PLANE-R027.1 — typed Postgres compatibility-feature
// inventory and fix-proof shape, refusing a fix recorded without a complete
// captured-traffic/failing-test/passing-test proof. Pure data;
// implementing each proven feature is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pg_compat_feature;
// MEDULLA-DECISION-ENGINE-R167.1/.2 — the consumer-declared contract of a skill, tool or
// prompt and its additive-only comparison, shared by the decision family and skill admission.
pub mod consumer_contract;
// MEDULLA-UNIFIED-DATA-PLANE-R028.1 — typed Postgres-equivalence differential
// deviation baseline, refusing a baseline update that drops a deviation
// without an explicit resolution record (no self-updating baseline). Pure
// data; running the regression/SQLancer/captured-traffic corpora is a
// later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pg_diff_baseline;
// MEDULLA-UNIFIED-DATA-PLANE-R025.1 — typed pgrx companion-extension spike
// decision record, refusing an outcome recorded without evidence for every
// declared scope area. Pure data; running the spike and the ADR review are
// later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pgrx_spike;
// MEDULLA-UNIFIED-DATA-PLANE-R025.3.1 — adapter skeleton reviewing a recorded
// pgrx spike decision into an approved ADR record, refusing an invalid
// decision, an empty reviewer, or an empty ADR title. Filing the ADR
// document itself is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pgrx_spike_review;
// MEDULLA-TYPED-PACKS-R100.101 — the pure skill admission rule over consumer contracts.
pub mod skill_admission;
pub mod skill_edit;
// MEDULLA-UNIFIED-DATA-PLANE-R031.1 — typed consolidation-isolation admission:
// per-application quota and upgrade-coupling policy, with the empty-identity
// and all-zero-quota refusal. Pure data; real fault isolation and quota
// metering are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod consolidation_isolation;
// MEDULLA-TYPED-PACKS-R802.1/R805.1 — governance record models: stewardship and
// data products. Pure data and field validation; served wiring lands in later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod data_product;
// MEDULLA-UNIFIED-DATA-PLANE-R022.1 — typed per-dialect conformance report entry
// and deviation shape, refusing an auto-suppressed (unreviewed) deviation.
// Pure data; running the containerized matrix is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dialect_conformance;
// MEDULLA-UNIFIED-DATA-PLANE-R040.1/.2 — per-modality embedding promotion gate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod embedding_promotion;
// MEDULLA-ENGINE-PARITY-R020.1 — relational engine capability report.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod engine_capabilities;
// MEDULLA-UNIFIED-DATA-PLANE-R030.1 — typed cross-application entity-link
// proposal, refusing an owl:sameAs assertion before approval or for a
// non-Match classification. Pure data; blocking, field-similarity scoring
// and the real Fellegi-Sunter model are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod entity_link_proposal;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod external_schema;
// MEDULLA-FEDERATED-QUERY-R724 — authorization-segmented schema boundary records
// and the `Method::SchemaProjections` ops.
pub mod schema_boundary;
// MEDULLA-DURABLE-KERNEL-R878 — the served completion barrier's resource names and answer.
pub mod await_watermark;
// MEDULLA-UNIFIED-DATA-PLANE-R004 — bounded, policy-gated column-sampling budget
// for deterministic schema inference.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod schema_inference;
// MEDULLA-UNIFIED-DATA-PLANE-R023.1 — typed shared CloudNativePG/MariaDB cluster
// group and per-application role-set shape, refusing a Postgres group
// without logical wal_level and an aliased application role. Pure data;
// operator wiring, PITR scheduling and the restore drill are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod shared_db_platform;
// MEDULLA-TYPED-PACKS-R801.1 — business glossary term model (SKOS projection).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod glossary;
// MEDULLA-UNIFIED-DATA-PLANE-R026.1 — typed Gramps native-hosting pilot phase
// gate (P0-P5), refusing a wrong or unreviewed exit artifact and skipping
// ahead of an un-exited earlier phase. Pure data; running each phase is a
// later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod gramps_pilot_phase;
// MEDULLA-UNIFIED-DATA-PLANE-R013.1 — Postgres attached-source adapter config and
// type-mapping typed model (connect/catalog/capture wiring is a later slice).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod postgres_adapter;
// CONCEPT:MEDULLA-KG.compute.uncertainty-values — probabilistic / uncertainty VALUE (distribution-valued
// properties). A stored value at the bottom of the DAG, NOT a wire `Op`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod distribution;
// GOC-10 — canonical SQL table / lake catalog authority wire types
// (`CatalogEntry`/`TableSchemaVersion`/`PartitionManifest`/`LakeSnapshot`/
// `QualityReportRef`/`TableChange`). Pure serde, no dep — unconditional like
// `acl`/`mutation_batch`, since it adds no `protocol::Method` variant (the durable
// store + REST projection that would carry these records over the wire is
// GOC-10-W03/W05, not yet implemented).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lake_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R032.1 — typed Immich connector pilot phase gate
// (I0-I6), refusing a wrong or unreviewed exit artifact and skipping ahead
// of an un-exited earlier phase. Pure data; running each phase is a later
// child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod immich_pilot_phase;
// MEDULLA-UNIFIED-DATA-PLANE-R003 — the typed, hashed attached-source catalog graph
// (tables/columns) and its ambiguous-table refusal.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod attached_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R014.1 — typed model distinguishing the MySQL and
// MariaDB attached-source dialect entries. See module docs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod attached_source_dialect;
// MEDULLA-UNIFIED-DATA-PLANE-R012.1 — typed model for governed attached-source write-back.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod attached_source_governance;
// MEDULLA-UNIFIED-DATA-PLANE-R014.2.3 -- the catalog probe state machine.
/// MEDULLA-UNIFIED-DATA-PLANE-R765 — the served foreign source catalog views.
pub mod foreign_source_catalog;
pub mod probe_machine;

// MEDULLA-UNIFIED-DATA-PLANE-R761.1 — column profile record.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod profile;
// MEDULLA-UNIFIED-DATA-PLANE-R762.1 — quality rules and scores.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
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
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod stewardship;
// MEDULLA-UNIFIED-DATA-PLANE-R038.6.1 — read-only property-graph view over mappings.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod virtual_graph_view;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
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
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mysql_mariadb_connection;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod ontology_diff;
// MEDULLA-TYPED-PACKS-R651.1 — append-only ontology edit chain records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod ontology_edit;
// Typed RDF input and the ontology inspection view, so orchestration callers
// never build or parse RDF text themselves.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
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
// MEDULLA-CQ-ORCH-R010.1 — pure data model for graph-derived architecture views.
pub mod architecture_view;
// MEDULLA-UNIFIED-DATA-PLANE-R764.1/.2 — asset usage and catalog search hits.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod asset_usage;
pub mod catalog_relevance;
