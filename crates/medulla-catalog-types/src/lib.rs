//! medulla-catalog-types: Connector pack, schema, catalog, source ingestion, lineage and release record types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: typed-packs-and-catalog (types), ingest-worker (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-UNIFIED-DATA-PLANE-R019.1 — typed inferred-shape catalog for an attached
// MongoDB/DocumentDB source. Pure data; the native-driver connection and
// change-stream capture are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod document_source_catalog;
// MEDULLA-UNIFIED-DATA-PLANE-R702 and R763.1 — dataset and column lineage.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lineage;
// MEDULLA-UNIFIED-DATA-PLANE-R027.1 — typed Postgres compatibility-feature
// inventory and fix-proof shape, refusing a fix recorded without a complete
// captured-traffic/failing-test/passing-test proof. Pure data;
// implementing each proven feature is a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pg_compat_feature;
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
// MEDULLA-FEDERATED-QUERY-R722.3 — host-driven bounded foreign-read lifecycle DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod foreign_read;
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
// X9 — keyed schema sources (shapes + ontology) on one request graph, and the
// composed digest that identifies the set in force.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_schema;
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
