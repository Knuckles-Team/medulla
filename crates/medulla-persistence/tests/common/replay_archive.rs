//! The in-memory archive and the bundle writer of the restore tests and the restore drill
//! bench (`MEDULLA-ENGINE-PARITY-R016.2` and `MEDULLA-ENGINE-PARITY-R016.3`).

use std::path::Path;
use std::sync::Arc;

use medulla_persistence::redb_backend::RedbBackend;
use medulla_storage::object::archive::ObjectArchive;

pub const TENANT: &str = "engine";

pub fn archive() -> ObjectArchive {
    ObjectArchive::new(Arc::new(object_store::memory::InMemory::new()), "root")
}

pub fn bundle_of(backend: &RedbBackend, dir: &Path) {
    backend
        .backup(dir, "test", 1_700_000_000, "replay", &[])
        .expect("bundle is written");
}
