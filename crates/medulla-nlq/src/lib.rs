//! medulla-nlq: Natural-language questions to UQL, with regressions.
//!
//! Level L5 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (natural language).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// DecideText (EH-073): the decision front end, kept out of UQL and `wire::Op`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide_text;
#[cfg(feature = "nl-query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod nl;
