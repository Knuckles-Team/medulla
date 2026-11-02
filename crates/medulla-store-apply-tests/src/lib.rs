//! medulla-store-apply-tests: whole-batch store tests for the store apply composition.
//!
//! Level F2c in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build. It is a dev-dependency only.

#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;
use medulla_store_apply::redb_store::control;
use medulla_store_apply::redb_store::checkpoint::*;
use medulla_store_apply::redb_store::crossmodal::*;
use medulla_store_apply::redb_store::dump::*;
use medulla_store_apply::redb_store::store_cleanup::*;
use medulla_store_apply::redb_store::store_control::*;
use medulla_store_apply::redb_store::store_mutation::*;
use medulla_store_apply::redb_store::store_retry::*;
use medulla_lane_store::redb_store::development_lane;
use medulla_resource_store::redb_store::resource::*;
use medulla_resource_store::redb_store::resource_rows::*;
use medulla_work_store::redb_store::work_item;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod commit_ops_phase_measurement;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod development_lane_tests;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod mutation_batch_tests;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod resource_reservation_tests;
#[cfg(feature = "security")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod security_tests;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod shard_control_tests;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod work_item_capability_tests;
