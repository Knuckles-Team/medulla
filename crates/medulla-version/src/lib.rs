//! The engine release version, in one place.
//!
//! Every workspace crate under `crates/` carries its own placeholder package
//! version, so `env!("CARGO_PKG_VERSION")` inside any of them names that
//! placeholder, not the release. This crate's package version is the engine
//! release version: `.config/bumpversion.cfg` moves it together with the root
//! `medulla-engine` package. Code that reports or records the engine version
//! (the `Health` report, the store migration journal, job and mining lineage,
//! backup manifests) reads [`VERSION`] from here and never calls
//! `env!("CARGO_PKG_VERSION")` itself; `tests/test_engine_release_version.py`
//! enforces both rules.

/// The engine release version, for example `3.1.1`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
