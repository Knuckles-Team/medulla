//! The durable COLD rung of the KV-cache tier ladder (MEDULLA-DURABLE-KERNEL-R083.5).
//!
//! [`KvNamespaceColdStore`] implements [`medulla_kvcache::ColdStore`] over the block
//! namespace of the engine's own `kv.redb` ([`KvStore`]), so a demoted block has ONE durable
//! copy: the same row a `KvGet` reads. Every cold write is the ledgered, admitted
//! owner-maintenance mutation `KvStore::put` already performs; this crate opens no table.

use std::io;
use std::sync::Arc;

use medulla_kvcache::ColdStore;
use medulla_server_blob::kv::KvStore;

/// The cold rung of a [`medulla_kvcache::TieredCache`] over one KV namespace.
pub struct KvNamespaceColdStore {
    store: Arc<KvStore>,
    namespace: &'static str,
}

impl KvNamespaceColdStore {
    /// A cold rung that keeps its blocks under `namespace` of `store`.
    pub fn new(store: Arc<KvStore>, namespace: &'static str) -> Self {
        Self { store, namespace }
    }
}

impl ColdStore<String> for KvNamespaceColdStore {
    fn put(&mut self, key: &String, bytes: &[u8]) -> io::Result<()> {
        self.store
            .put(self.namespace, key, bytes.to_vec())
            .map_err(io::Error::other)
    }

    fn get(&self, key: &String) -> io::Result<Option<Vec<u8>>> {
        self.store
            .get(self.namespace, key)
            .map_err(io::Error::other)
    }

    fn remove(&mut self, key: &String) -> io::Result<()> {
        self.store
            .delete(self.namespace, key)
            .map(|_| ())
            .map_err(io::Error::other)
    }

    fn kind(&self) -> &'static str {
        "kv-namespace"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cold rung round-trips a block through the namespace and a removal leaves it absent.
    #[test]
    fn namespace_cold_store_roundtrips_and_removes() {
        let store = Arc::new(KvStore::open(None).unwrap());
        let mut cold = KvNamespaceColdStore::new(Arc::clone(&store), "kvcache-blocks");
        let key = "abc".to_string();
        cold.put(&key, b"page").unwrap();
        assert_eq!(cold.get(&key).unwrap().as_deref(), Some(&b"page"[..]));
        assert_eq!(
            store.get("kvcache-blocks", "abc").unwrap().as_deref(),
            Some(&b"page"[..])
        );
        cold.remove(&key).unwrap();
        assert_eq!(cold.get(&key).unwrap(), None);
    }
}
