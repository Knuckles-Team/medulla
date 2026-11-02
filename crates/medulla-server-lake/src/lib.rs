//! medulla-server-lake: Lake serving, GraphQL subscriptions and policy export.
//!
//! Level F10 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// Shared request/auth/codec support for native wire adapters. Each adapter keeps
// its own protocol limits and wire state on top of this seam.
#[cfg(any(
    feature = "amqp-wire",
    feature = "mqtt-wire",
    feature = "stomp-wire",
    feature = "mssql-wire",
    feature = "redis-wire"
))]
// S3-compatible object-storage REST surface (CONCEPT:MEDULLA-KG.ontology.object-put-get-head). A hand-rolled HTTP/1.1
// listener exposing PutObject/GetObject/ListObjectsV2/CreateBucket/… over the
// content-addressed BLOB CAS (bytes) + the durable KV index (listing), with a
// SigV4-lite auth guard. Behind the `s3-api` cargo feature (pulls `blob` + `kv`).
// Default/pi builds compile NONE of it.
/// Current GraphQL subscriptions over authenticated Server-Sent Events
/// (CONCEPT:MEDULLA-KG.compute.cdc-event-emit, feature `graphql`): eg2 request binding,
/// graph ACL, and default-deny RLS guard every caller-visible re-resolution.
#[cfg(feature = "graphql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graphql_sub;
/// LTAP lakehouse materialization tier (CONCEPT:MEDULLA-KG.storage.lsn-as-snapshot-returns engine-side seam, feature
/// `lake`, INT-P2-3): converts real `medulla_tsdb` series into `medulla_lake::LakeBatch`es,
/// materializes them to Parquet on the blob CAS with Delta/Iceberg logs + an
/// Iceberg-REST catalog + OpenLineage run events. The `rest` submodule (feature
/// `lake-rest`) serves the standards Iceberg-REST catalog surface over it.
#[cfg(feature = "lake")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lake;
// RLS marking-predicate export (CA-16, feature `policy_export`, off by default).
// Reserved and EMPTY until CA-16 implements it; landed by CA-17's single feature-stub
// commit so CA-16 never edits this shared module list.
#[cfg(feature = "policy_export")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod policy_export;
