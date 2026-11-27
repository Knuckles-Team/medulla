//! The durable startup reports of the last boots (MEDULLA-OBSERVE-R008.2).
//!
//! One kernel-owned owner file, `observe_startup.redb`, of layout `OwnerLayout::ObserveStartup`,
//! modelled on the scheduler policy store. One row per boot, keyed by the boot sequence number and
//! valued with the MessagePack of that boot's report. A write keeps the newest `keep` rows. This
//! type is the only caller of the `observe_startup_report` table.

use std::path::Path;
use std::sync::Arc;

use medulla_observe::startup::StoredBoot;
use redb::ReadableTable;

/// The owner file name under the persist dir.
pub const OBSERVE_STARTUP_FILE: &str = "observe_startup.redb";
/// The physical authority of [`OBSERVE_STARTUP_FILE`].
pub const OBSERVE_STARTUP_PHYSICAL_STORE: &str = "epistemic-graph:observe-startup";
const OBSERVE_STARTUP_SCOPE_RESOURCE: &str = "observe-startup";
const OBSERVE_STARTUP_SCOPE_INCARNATION: &str = "observe-startup:v1";

type Durable = medulla_store::sidecar_store::SidecarStore<medulla_storage::ObserveStartupOwner>;

/// The startup report owner file.
pub struct ObserveStartupStore {
    durable: Durable,
}

impl std::fmt::Debug for ObserveStartupStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObserveStartupStore")
            .finish_non_exhaustive()
    }
}

impl ObserveStartupStore {
    /// Open (or create) `observe_startup.redb` under `persist_dir`.
    pub fn open(persist_dir: &str) -> Result<Self, String> {
        let path = Path::new(persist_dir).join(OBSERVE_STARTUP_FILE);
        let durable = Durable::open(
            &path,
            OBSERVE_STARTUP_PHYSICAL_STORE,
            OBSERVE_STARTUP_SCOPE_RESOURCE,
            OBSERVE_STARTUP_SCOPE_INCARNATION,
            medulla_kernel::store_authority::process_authority(),
        )?;
        Ok(Self { durable })
    }

    /// Every stored boot report, oldest first. A row that does not decode is an error: a report
    /// that cannot be read is never silently dropped.
    pub fn list(&self) -> Result<Vec<StoredBoot>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::OBSERVE_STARTUP_REPORT)?;
        let mut boots = Vec::new();
        for row in table.iter().map_err(|error| error.to_string())? {
            let (_, value) = row.map_err(|error| error.to_string())?;
            boots.push(
                rmp_serde::from_slice::<StoredBoot>(value.value())
                    .map_err(|error| format!("a stored startup report does not decode: {error}"))?,
            );
        }
        Ok(boots)
    }

    /// Store `boot` under its sequence number and keep only the newest `keep` rows.
    pub fn put(&self, boot: &StoredBoot, keep: u64) -> Result<(), String> {
        let bytes = rmp_serde::to_vec_named(boot).map_err(|error| error.to_string())?;
        self.durable.maintain("observe_startup_put", |owner| {
            let mut table = owner.open_table(medulla_storage::OBSERVE_STARTUP_REPORT)?;
            table
                .insert(boot.boot_seq, bytes.as_slice())
                .map_err(|error| error.to_string())?;
            let keys: Vec<u64> = table
                .iter()
                .map_err(|error| error.to_string())?
                .map(|row| {
                    row.map(|(key, _)| key.value())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<_, _>>()?;
            let excess = keys.len().saturating_sub(keep.max(1) as usize);
            for key in keys.into_iter().take(excess) {
                table.remove(key).map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }
}

/// Open (once) the shared handle `ServerState` retains, under the configured persist directory.
/// redb admits one opener per file.
pub fn shared_observe_startup_store(
    state: &medulla_server_api::state::ServerState,
) -> Result<Arc<ObserveStartupStore>, String> {
    let mut guard = state.observe_startup_store.lock();
    if let Some(store) = guard
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<ObserveStartupStore>().ok())
    {
        return Ok(store);
    }
    let persist_dir = state.persist_dir.as_deref().ok_or_else(|| {
        "the startup report store requires a configured persist directory".to_string()
    })?;
    let store = Arc::new(ObserveStartupStore::open(persist_dir)?);
    *guard = Some(Arc::clone(&store) as medulla_server_api::state::ErasedHandle);
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_observe::startup::StoredStep;

    fn boot(seq: u64) -> StoredBoot {
        StoredBoot {
            boot_seq: seq,
            started_unix_ms: 1_000 * seq,
            steps: vec![StoredStep {
                name: "store_migration".to_string(),
                graph: String::new(),
                started_unix_ms: 1_000 * seq,
                duration_ms: seq,
                outcome: "Ok".to_string(),
                detail: None,
            }],
        }
    }

    // spec: MEDULLA-OBSERVE-R008.2
    #[test]
    fn reports_survive_reopen_and_only_the_newest_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().to_str().unwrap();
        {
            let store = ObserveStartupStore::open(persist).unwrap();
            assert!(store.list().unwrap().is_empty());
            for seq in 1..=5 {
                store.put(&boot(seq), 3).unwrap();
            }
        }
        let store = ObserveStartupStore::open(persist).unwrap();
        let kept: Vec<u64> = store
            .list()
            .unwrap()
            .iter()
            .map(|boot| boot.boot_seq)
            .collect();
        assert_eq!(kept, vec![3, 4, 5]);
        assert_eq!(store.list().unwrap()[2], boot(5));
    }
}
