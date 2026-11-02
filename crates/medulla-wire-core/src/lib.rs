//! medulla-wire-core: Shared wire listener framework.
//!
//! Level F8 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(
    feature = "amqp-wire",
    feature = "mqtt-wire",
    feature = "stomp-wire",
    feature = "mssql-wire",
    feature = "redis-wire",
))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod broker_wire;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "mysql-wire", feature = "pgwire"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sql_wire_auth;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod wire;
