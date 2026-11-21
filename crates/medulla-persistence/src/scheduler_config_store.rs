//! The durable scheduler policy store (MEDULLA-DURABLE-KERNEL-R076.2).
//!
//! One kernel-owned owner file, `scheduler_config.redb`, of layout
//! `OwnerLayout::SchedulerConfig`, modelled on the retrieval document-state store.
//! It holds one row: the JSON of the `SchedulerConfig` that `SetSchedulerConfig`
//! last validated. This type is the only caller of the `scheduler_config` table.

use std::path::Path;
use std::sync::Arc;

/// The owner file name under the persist dir.
pub const SCHEDULER_CONFIG_FILE: &str = "scheduler_config.redb";
/// The physical authority of [`SCHEDULER_CONFIG_FILE`].
pub const SCHEDULER_CONFIG_PHYSICAL_STORE: &str = "epistemic-graph:scheduler-config";
const SCHEDULER_CONFIG_SCOPE_RESOURCE: &str = "scheduler-config";
const SCHEDULER_CONFIG_SCOPE_INCARNATION: &str = "scheduler-config:v1";
/// The key of the one policy row.
const POLICY_ROW: &str = "scheduler_config";

type Durable = medulla_store::sidecar_store::SidecarStore<medulla_storage::SchedulerConfigOwner>;

/// The scheduler policy owner file.
pub struct SchedulerConfigStore {
    durable: Durable,
}

impl std::fmt::Debug for SchedulerConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerConfigStore")
            .finish_non_exhaustive()
    }
}

impl SchedulerConfigStore {
    /// Open (or create) `scheduler_config.redb` under `persist_dir`.
    pub fn open(persist_dir: &str) -> Result<Self, String> {
        let path = Path::new(persist_dir).join(SCHEDULER_CONFIG_FILE);
        let durable = Durable::open(
            &path,
            SCHEDULER_CONFIG_PHYSICAL_STORE,
            SCHEDULER_CONFIG_SCOPE_RESOURCE,
            SCHEDULER_CONFIG_SCOPE_INCARNATION,
            medulla_kernel::store_authority::process_authority(),
        )?;
        Ok(Self { durable })
    }

    /// The stored policy JSON, if one was stored.
    pub fn get(&self) -> Result<Option<String>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::SCHEDULER_CONFIG)?;
        let row = table.get(POLICY_ROW).map_err(|error| error.to_string())?;
        row.map(|value| {
            String::from_utf8(value.value().to_vec())
                .map_err(|_| "the stored scheduler policy is not text".to_string())
        })
        .transpose()
    }

    /// Replace the stored policy JSON.
    pub fn put(&self, json: &str) -> Result<(), String> {
        self.durable.maintain("scheduler_config_put", |owner| {
            let mut table = owner.open_table(medulla_storage::SCHEDULER_CONFIG)?;
            table
                .insert(POLICY_ROW, json.as_bytes())
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }
}

/// Open (once) the shared policy-store handle `ServerState` retains, under the
/// configured persist directory. redb admits one opener per file.
pub fn shared_scheduler_config_store(
    state: &medulla_server_api::state::ServerState,
) -> Result<Arc<SchedulerConfigStore>, String> {
    let mut guard = state.scheduler_config_store.lock();
    if let Some(store) = guard
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<SchedulerConfigStore>().ok())
    {
        return Ok(store);
    }
    let persist_dir = state.persist_dir.as_deref().ok_or_else(|| {
        "the scheduler policy requires a configured persist directory".to_string()
    })?;
    let store = Arc::new(SchedulerConfigStore::open(persist_dir)?);
    *guard = Some(Arc::clone(&store) as medulla_server_api::state::ErasedHandle);
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R076.2
    #[test]
    fn policy_survives_reopen_and_a_second_put_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().to_str().unwrap();
        {
            let store = SchedulerConfigStore::open(persist).unwrap();
            assert_eq!(store.get().unwrap(), None);
            store.put("{\"a\":1}").unwrap();
            store.put("{\"a\":2}").unwrap();
        }
        let store = SchedulerConfigStore::open(persist).unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("{\"a\":2}"));
    }
}
