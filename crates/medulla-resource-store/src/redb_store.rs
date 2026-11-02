//! Modules moved here from `medulla_store::redb_store` (hard cutover, no re-exports).

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod capacity_lease;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod resource;
#[allow(missing_docs)]
pub mod resource_rows;

use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;
