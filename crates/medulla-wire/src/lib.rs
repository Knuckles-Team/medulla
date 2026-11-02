//! medulla-wire: wire protocol listeners. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 5 in `scripts/crate_split/manifest.toml`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]


/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "bolt-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod bolt_wire;





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
#[cfg(feature = "sqlite-wire")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sqlite_wire;
