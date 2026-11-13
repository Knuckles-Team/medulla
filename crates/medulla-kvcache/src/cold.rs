//! The COLD tier backing store (CONCEPT:MEDULLA-KG.storage.durable-redb-cold-tier).
//!
//! The COLD tier is where the cache offloads bytes it can no longer keep in RAM — the
//! "survive OOM by spilling to disk" rung. [`ColdStore`] is the pluggable seam:
//!
//! * [`MemoryColdStore`] — always available, dependency-free. Holds the spilled blobs
//!   in a separate `HashMap` so the tier machinery (and all its tests) work in a pure
//!   RAM build. It models the tier boundary without a durable device.
//! * A durable store is supplied by the caller: the server's cold rung is
//!   `KvNamespaceColdStore` over the `kvcache-blocks` namespace of the kv store, so demoted
//!   bytes SURVIVE an OOM / process restart through the one kv owner file. This crate
//!   owns no database.
//!
//! `ColdStore` is intentionally object-safe (`Box<dyn ColdStore<K>>`) and moves only
//! bytes — the cache owns all per-key metadata (score, compression flag, sizes), so a
//! backend is a dumb blob device.

use std::io;

/// A pluggable COLD-tier blob device the tiered cache spills bytes into (CONCEPT:MEDULLA-KG.storage.durable-redb-cold-tier).
///
/// Object-safe by design: keys and values cross as borrows / owned `Vec<u8>`, so it can
/// be held as `Box<dyn ColdStore<K>>`. The cache keeps the per-key metadata; a store
/// only persists / returns opaque blobs.
pub trait ColdStore<K> {
    /// Write (or overwrite) the blob for `key`.
    fn put(&mut self, key: &K, bytes: &[u8]) -> io::Result<()>;
    /// Read the blob for `key`, if present.
    fn get(&self, key: &K) -> io::Result<Option<Vec<u8>>>;
    /// Remove the blob for `key` (no-op if absent).
    fn remove(&mut self, key: &K) -> io::Result<()>;
    /// A short label for diagnostics / stats.
    fn kind(&self) -> &'static str;
}

/// An in-RAM COLD store (CONCEPT:MEDULLA-KG.storage.durable-redb-cold-tier) — always available, no dependency.
///
/// It keeps spilled blobs in a *separate* map from the HOT/WARM tiers, so demotion to
/// COLD is a real tier transition (bytes leave the HOT/WARM budgets) even in a pure-RAM
/// build. It is NOT durable — plug a durable [`ColdStore`] in for
/// offload-that-survives-a-restart.
#[derive(Debug, Default)]
pub struct MemoryColdStore<K> {
    blobs: std::collections::HashMap<K, Vec<u8>>,
}

impl<K: std::hash::Hash + Eq + Clone> MemoryColdStore<K> {
    /// A fresh, empty in-RAM cold store.
    pub fn new() -> Self {
        MemoryColdStore {
            blobs: std::collections::HashMap::new(),
        }
    }
}

impl<K: std::hash::Hash + Eq + Clone> ColdStore<K> for MemoryColdStore<K> {
    fn put(&mut self, key: &K, bytes: &[u8]) -> io::Result<()> {
        self.blobs.insert(key.clone(), bytes.to_vec());
        Ok(())
    }
    fn get(&self, key: &K) -> io::Result<Option<Vec<u8>>> {
        Ok(self.blobs.get(key).cloned())
    }
    fn remove(&mut self, key: &K) -> io::Result<()> {
        self.blobs.remove(key);
        Ok(())
    }
    fn kind(&self) -> &'static str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CONCEPT:MEDULLA-KG.storage.durable-redb-cold-tier — the RAM cold store round-trips blobs (models the tier boundary).
    #[test]
    fn eg185_memory_cold_store_roundtrips() {
        let mut cs: MemoryColdStore<String> = MemoryColdStore::new();
        cs.put(&"k".to_string(), b"hello").unwrap();
        assert_eq!(
            cs.get(&"k".to_string()).unwrap().as_deref(),
            Some(&b"hello"[..])
        );
        cs.remove(&"k".to_string()).unwrap();
        assert_eq!(cs.get(&"k".to_string()).unwrap(), None);
    }
}
