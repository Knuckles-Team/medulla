//! medulla-ingest: Ingest worker loop, lanes and queues.
//!
//! Layer L5 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: ingest-worker.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
