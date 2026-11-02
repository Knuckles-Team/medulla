//! medulla-wire-core: Shared wire listener framework.
//!
//! Layer F8 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.

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

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
