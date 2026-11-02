//! medulla-wire-msg: Messaging and object listeners: AMQP, MQTT, STOMP, Redis, DDS, S3 and the KV cache endpoint.
//!
//! Level F9 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: engine-parity (messaging wire), object-storage-backend (S3).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![deny(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "amqp-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod amqp_wire;
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
#[cfg(feature = "stomp-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod stomp_wire;
