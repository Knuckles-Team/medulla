//! Pure-Rust SQLite `.db` file-format reader + bulk-load writer — no C, no libsqlite3.
//!
//! This is a leaf crate at the bottom of the medulla DAG (positioned like
//! `medulla-ann`): it knows nothing about `TableStore`/`Cell`/`ColumnType`, only SQLite bytes.
//! The facade `src/server/handlers/sqlite_file.rs` is the sole translator between the
//! engine's typed model and this crate's [`Value`]/[`Row`]/[`ColumnDef`] vocabulary.
//!
//! Scope: ordinary rowid table b-trees only. WITHOUT ROWID tables and index b-trees are
//! rejected as [`Error::Unsupported`], never mis-decoded. The [`Writer`] is a one-shot
//! bulk serializer (rows known in full, in advance, sorted by rowid) — deliberately NOT a
//! general mutable SQLite database. Format primitives reimplement the public SQLite file
//! format (the technique used by `open-source-libraries/turso/core/storage`).

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them

mod error;
mod freelist;
mod header;
mod overflow;
mod page;
mod reader;
mod record;
mod schema;
mod value;
mod varint;
mod writer;

pub use error::{Error, Result};
pub use reader::Reader;
pub use value::{ColumnDef, Row, Value};
pub use writer::Writer;
