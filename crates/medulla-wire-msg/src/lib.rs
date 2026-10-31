//! medulla-wire-msg: Messaging and object listeners: AMQP, MQTT, STOMP, Redis, DDS, S3 and the KV cache endpoint.
//!
//! Level F9 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: engine-parity (messaging wire), object-storage-backend (S3).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
