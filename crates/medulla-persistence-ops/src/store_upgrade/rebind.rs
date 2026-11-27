//! Explicit re-binding of a copied data directory
//! (MEDULLA-DURABLE-KERNEL-R862.2).
//!
//! A store binds to the device and inode of its file, so a copied data
//! directory refuses the ordinary open. `rebind` is the one command that says
//! the copy is intended. Each copied store is measured against the same-named
//! store of the directory it was copied from, rebound through the storage
//! kernel's `rebind_copied_store`, and measured again. Nothing is written
//! unless every store passes the pre-checks.

use std::path::{Path, PathBuf};

use medulla_storage::{
    open_read_only, rebind_copied_store, recovery_store_data_fingerprint, store_root_is_current,
    validate_recovery_store_read_only,
};

use super::discover::{discover, StoreFile};
use super::existing_directory;
use super::report::{Report, StoreReport, StoreStatus};

/// What `rebind` was asked to do.
#[derive(Debug, Clone)]
pub struct RebindOptions {
    /// The data directory the copy was made from.
    pub copied_from: PathBuf,
    /// The explicit confirmation. Without it nothing is opened.
    pub confirm: bool,
}

/// One copied store with the store it was copied from.
struct Pair<'a> {
    store: &'a StoreFile,
    source: PathBuf,
}

/// Re-bind every copied store under `data_dir` to its own path.
pub fn rebind(data_dir: &Path, options: &RebindOptions) -> Report {
    match rebind_stores(data_dir, options) {
        Ok(stores) => Report::finished("rebind", data_dir, stores, 0),
        Err(error) => Report::refused("rebind", data_dir, error),
    }
}

fn rebind_stores(data_dir: &Path, options: &RebindOptions) -> Result<Vec<StoreReport>, String> {
    if !options.confirm {
        return Err(
            "REBIND_NOT_CONFIRMED: rebind rewrites the store roots of a copied data directory; \
             pass --confirm to run it"
                .to_string(),
        );
    }
    let data_dir = existing_directory(data_dir)?;
    let copied_from = existing_directory(&options.copied_from)?;
    // The same single-writer lock the engine holds for its whole lifetime.
    let _lease = medulla_core::persist_lock::acquire(&data_dir.to_string_lossy())?;
    let stores = discover(&data_dir)?;

    let mut reports = Vec::with_capacity(stores.len());
    let mut pairs = Vec::new();
    for store in &stores {
        if store_root_is_current(&store.path)? {
            reports.push(StoreReport::new(
                &store.name,
                store.served_layout,
                StoreStatus::Current,
                "already bound to this path; skipped".to_string(),
            ));
        } else {
            let source = copied_from.join(&store.name);
            if !std::fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_file()) {
                return Err(format!(
                    "REBIND_SOURCE_MISSING: {} has no same-named store under {}",
                    store.name,
                    copied_from.display()
                ));
            }
            pairs.push(Pair { store, source });
        }
    }

    // Measure every pair before the first write: the source is validated
    // read-only, and the copy must carry the same data rows as its source.
    let mut fingerprints = Vec::with_capacity(pairs.len());
    for pair in &pairs {
        validate_source(pair)?;
        let source = recovery_store_data_fingerprint(&pair.source)?;
        if recovery_store_data_fingerprint(&pair.store.path)? != source {
            return Err(fingerprint_changed(pair.store));
        }
        fingerprints.push(source);
    }

    for (pair, source_fingerprint) in pairs.iter().zip(fingerprints) {
        rebind_copied_store(&pair.store.path, &pair.source)?;
        if recovery_store_data_fingerprint(&pair.store.path)? != source_fingerprint {
            return Err(fingerprint_changed(pair.store));
        }
        reports.push(StoreReport::new(
            &pair.store.name,
            pair.store.served_layout,
            StoreStatus::Rebound,
            "rebound to this path; data rows unchanged".to_string(),
        ));
    }
    Ok(reports)
}

/// Run the read-only recovery validation on the source store. The copy is never
/// validated this way before its rebind: it fails that check by design.
fn validate_source(pair: &Pair<'_>) -> Result<(), String> {
    let integrity = if pair.store.name == medulla_persistence::backup::ADMIN_MUTATIONS_FILE {
        medulla_persistence::redb_backend::admin_mutations_private_integrity()
    } else {
        None
    };
    let source = open_read_only(&pair.source, integrity)
        .map_err(|error| format!("source store {}: {error}", pair.store.name))?;
    validate_recovery_store_read_only(&source)
        .map(|_| ())
        .map_err(|error| format!("source store {}: {error}", pair.store.name))
}

fn fingerprint_changed(store: &StoreFile) -> String {
    format!(
        "REBIND_FINGERPRINT_CHANGED: {} does not hold the data rows of its source",
        store.name
    )
}
