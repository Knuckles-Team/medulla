//! The installed seam implementations, one slot per trait.

use std::sync::{Arc, OnceLock};

use super::admin::{AdminApi, DecideApi};
use super::ingest::{ComputeApi, GraphApi, IngestApi, JobsApi};
use super::query_txn::{QueryApi, TxnApi, WireApi};
use super::reentry::DispatchApi;

/// One slot per seam trait. The server installs every slot once at startup; a slot read before
/// installation returns `None`, which callers map to their existing internal error.
#[derive(Default)]
pub struct SeamRegistry {
    /// Admin handler seam.
    pub admin: OnceLock<Arc<dyn AdminApi>>,
    /// Decide handler seam.
    pub decide: OnceLock<Arc<dyn DecideApi>>,
    /// Ingest handler seam.
    pub ingest: OnceLock<Arc<dyn IngestApi>>,
    /// Graph handler seam.
    pub graph: OnceLock<Arc<dyn GraphApi>>,
    /// Jobs handler seam.
    pub jobs: OnceLock<Arc<dyn JobsApi>>,
    /// Compute handler seam.
    pub compute: OnceLock<Arc<dyn ComputeApi>>,
    /// Query handler seam.
    pub query: OnceLock<Arc<dyn QueryApi>>,
    /// Transaction handler seam.
    pub txn: OnceLock<Arc<dyn TxnApi>>,
    /// Wire protocol seam.
    pub wire: OnceLock<Arc<dyn WireApi>>,
    /// Method-router re-entry seam.
    pub dispatch: OnceLock<Arc<dyn DispatchApi>>,
}

impl std::fmt::Debug for SeamRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeamRegistry").finish_non_exhaustive()
    }
}
