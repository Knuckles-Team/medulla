//! The installed seam implementations, one slot per trait.

use std::sync::{Arc, OnceLock};

use super::reentry::DispatchApi;
use crate::state::ErasedHandle;

/// One slot per seam trait. A slot holds `Arc::new(Arc<dyn XApi>)`; the typed accessor
/// (`x_seam`) and installer (`install_x_seam`) sit beside each trait. The server installs every slot once at startup; a slot read before
/// installation returns `None`, which callers map to their existing internal error.
#[derive(Default)]
pub struct SeamRegistry {
    /// Admin handler seam.
    pub admin: OnceLock<ErasedHandle>,
    /// Decide handler seam.
    pub decide: OnceLock<ErasedHandle>,
    /// Ingest handler seam.
    pub ingest: OnceLock<ErasedHandle>,
    /// Graph handler seam.
    pub graph: OnceLock<ErasedHandle>,
    /// Jobs handler seam.
    pub jobs: OnceLock<ErasedHandle>,
    /// Compute handler seam.
    pub compute: OnceLock<ErasedHandle>,
    /// Query handler seam.
    pub query: OnceLock<ErasedHandle>,
    /// Transaction handler seam.
    pub txn: OnceLock<ErasedHandle>,
    /// Wire protocol seam.
    pub wire: OnceLock<ErasedHandle>,
    /// Method-router re-entry seam.
    pub dispatch: OnceLock<Arc<dyn DispatchApi>>,
}

impl std::fmt::Debug for SeamRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeamRegistry").finish_non_exhaustive()
    }
}
