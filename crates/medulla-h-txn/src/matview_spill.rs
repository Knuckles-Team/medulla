//! The redb-backed store for spilled incremental view state (MEDULLA-INCREMENTAL-VIEWS-R007.4).
//!
//! `put` writes the blob to the `MATVIEW_OPERATOR_STATE` row off the caller's thread and
//! releases the manager's pending copy once the row is durable. `get` reads one row.

use std::sync::Arc;

use medulla_persistence::AsRedb;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_cdc::matview::{self, ViewStateSpill};

/// Spills view state into the durable operator-state table.
pub struct RedbViewStateSpill {
    backend: Arc<dyn PersistenceBackend>,
}

impl RedbViewStateSpill {
    /// A spill store over `backend`; it spills nothing when the backend is not redb.
    pub fn new(backend: Arc<dyn PersistenceBackend>) -> Self {
        Self { backend }
    }
}

impl ViewStateSpill for RedbViewStateSpill {
    fn put(&self, name: &str, bytes: Arc<Vec<u8>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(view = %name, "matview spill write skipped: no async runtime");
            return;
        };
        let backend = self.backend.clone();
        let name = name.to_string();
        runtime.spawn(async move {
            let Some(redb) = backend.as_redb() else {
                return;
            };
            match redb
                .matview_operator_state_put(&name, bytes.as_ref().clone())
                .await
            {
                Ok(()) => matview::manager().spill_acknowledged(&name, &bytes),
                Err(error) => tracing::warn!(view = %name, "matview spill write failed: {error}"),
            }
        });
    }

    fn get(&self, name: &str) -> Option<Vec<u8>> {
        match self.backend.as_redb()?.matview_operator_state_get(name) {
            Ok(blob) => blob,
            Err(error) => {
                tracing::warn!(view = %name, "matview spill read failed: {error}");
                None
            }
        }
    }
}
