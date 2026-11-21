//! `CountingObjectStore`: a wrapper over any `object_store` implementation that counts
//! the actions and bytes one lifecycle costs (MEDULLA-OBJECT-STORAGE-BACKEND-R005).
//!
//! Actions are counted per shard, the `<root>/<tenant>/<graph>` prefix of the object key,
//! which a single-store harness cannot express. The wrapper changes no result: every call
//! is forwarded unchanged and only counted. Time and memory are measured on a separate
//! run without the wrapper.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use object_store::path::Path as ObjPath;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult, Result as OsResult,
};

/// What one shard cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShardCounts {
    pub gets: u64,
    pub puts: u64,
    pub lists: u64,
    pub deletes: u64,
    pub copies: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

impl ShardCounts {
    /// Every object action, whatever its kind.
    pub fn actions(&self) -> u64 {
        self.gets + self.puts + self.lists + self.deletes + self.copies
    }

    fn add(&mut self, other: &ShardCounts) {
        self.gets += other.gets;
        self.puts += other.puts;
        self.lists += other.lists;
        self.deletes += other.deletes;
        self.copies += other.copies;
        self.bytes_read += other.bytes_read;
        self.bytes_written += other.bytes_written;
    }
}

/// The shard an object key belongs to: its first three path segments.
pub fn shard_of(location: &str) -> String {
    location.split('/').take(3).collect::<Vec<_>>().join("/")
}

/// Counts the actions and bytes of an inner store.
#[derive(Debug)]
pub struct CountingObjectStore {
    inner: Arc<dyn ObjectStore>,
    shards: Mutex<BTreeMap<String, ShardCounts>>,
}

impl CountingObjectStore {
    pub fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            shards: Mutex::new(BTreeMap::new()),
        }
    }

    fn record(&self, location: &str, change: impl FnOnce(&mut ShardCounts)) {
        let mut shards = self.shards.lock().expect("counting lock");
        change(shards.entry(shard_of(location)).or_default());
    }

    /// The counts of each shard, ordered by shard key.
    pub fn per_shard(&self) -> BTreeMap<String, ShardCounts> {
        self.shards.lock().expect("counting lock").clone()
    }

    /// The counts of every shard added together.
    pub fn total(&self) -> ShardCounts {
        let mut total = ShardCounts::default();
        for counts in self.shards.lock().expect("counting lock").values() {
            total.add(counts);
        }
        total
    }

    /// Forget every count, as at the start of the next lifecycle phase.
    pub fn reset(&self) {
        self.shards.lock().expect("counting lock").clear();
    }
}

impl std::fmt::Display for CountingObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "counting({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for CountingObjectStore {
    async fn put_opts(
        &self,
        location: &ObjPath,
        payload: PutPayload,
        options: PutOptions,
    ) -> OsResult<PutResult> {
        let bytes = payload.content_length() as u64;
        self.record(location.as_ref(), |c| {
            c.puts += 1;
            c.bytes_written += bytes;
        });
        self.inner.put_opts(location, payload, options).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjPath,
        options: PutMultipartOptions,
    ) -> OsResult<Box<dyn MultipartUpload>> {
        self.record(location.as_ref(), |c| c.puts += 1);
        self.inner.put_multipart_opts(location, options).await
    }

    async fn get_opts(&self, location: &ObjPath, options: GetOptions) -> OsResult<GetResult> {
        let result = self.inner.get_opts(location, options).await;
        let bytes = result
            .as_ref()
            .map_or(0, |found| found.range.end.saturating_sub(found.range.start));
        self.record(location.as_ref(), |c| {
            c.gets += 1;
            c.bytes_read += bytes;
        });
        result
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, OsResult<ObjPath>>,
    ) -> BoxStream<'static, OsResult<ObjPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&ObjPath>) -> BoxStream<'static, OsResult<ObjectMeta>> {
        self.record(prefix.map_or("", |p| p.as_ref()), |c| c.lists += 1);
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&ObjPath>) -> OsResult<ListResult> {
        self.record(prefix.map_or("", |p| p.as_ref()), |c| c.lists += 1);
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(&self, from: &ObjPath, to: &ObjPath, options: CopyOptions) -> OsResult<()> {
        self.record(to.as_ref(), |c| c.copies += 1);
        self.inner.copy_opts(from, to, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use object_store::ObjectStoreExt;

    // spec: MEDULLA-OBJECT-STORAGE-BACKEND-R005
    #[tokio::test]
    async fn counts_actions_and_bytes_per_shard() {
        let store = CountingObjectStore::new(Arc::new(InMemory::new()));
        let a = ObjPath::from("root/tenant/graph-a/HEAD");
        let b = ObjPath::from("root/tenant/graph-b/HEAD");
        store.put(&a, b"1234".to_vec().into()).await.unwrap();
        store.put(&b, b"12".to_vec().into()).await.unwrap();
        store.get(&a).await.unwrap().bytes().await.unwrap();
        let shards = store.per_shard();
        assert_eq!(shards["root/tenant/graph-a"].puts, 1);
        assert_eq!(shards["root/tenant/graph-a"].gets, 1);
        assert_eq!(shards["root/tenant/graph-a"].bytes_written, 4);
        assert_eq!(shards["root/tenant/graph-a"].bytes_read, 4);
        assert_eq!(shards["root/tenant/graph-b"].actions(), 1);
        assert_eq!(store.total().actions(), 3);
        store.reset();
        assert_eq!(store.total().actions(), 0);
    }
}
