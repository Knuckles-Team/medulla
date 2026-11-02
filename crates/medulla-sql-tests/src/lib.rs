//! medulla-sql-tests: integration tests of the SQL engine over tables, user tables, UDFs, PGQ and federation scans.
//!
//! Level L4e in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); the tests live in
//! `tests/`, this library is intentionally empty.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]
