//! medulla-wire: wire protocol listeners. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 5 in `scripts/crate_split/manifest.toml`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "amqp-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod amqp_wire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "bolt-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod bolt_wire;


/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "ros2-bridge", feature = "ros2-dds", feature = "ros2-rmw"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dds;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "kvcache-server")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod kvcache_http;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "mqtt-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mqtt_wire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "mssql-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mssql_wire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "mysql-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mysql_wire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "pgwire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pgwire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redis-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redis_wire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "ros2-bridge", feature = "ros2-dds", feature = "ros2-rmw"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod ros2_bridge;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "s3-api")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod s3;


/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "sqlite-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sqlite_wire;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "stomp-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod stomp_wire;

