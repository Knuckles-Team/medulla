//! The object-store persistence backend, commit before ack
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R003).
//!
//! [`ObjectStoreBackend`] is the object-store arm of the persistence layer:
//! the served commit chain (`PersistenceBackend::commit_mutation_batch`)
//! commits the batch to the local redb store first (step 1), then hands the
//! encoded, already-durable `BatchSegment` to [`ObjectPublisher::publish`]
//! for steps 2–4, and acks the caller only after the `HEAD` conditional put
//! (step 4, the fence) is confirmed by the store. A losing fence is
//! `OBJECT_PUBLISH_STALE`: the node stops publishing, discards its local
//! commit and re-fetches `HEAD`.
//!
//! Open: a spec without a durability class refuses with
//! `OBJECT_DURABILITY_UNDECLARED`, the typed form of
//! [`medulla_storage::backend_spec::BackendSpecRefusal::MissingDurabilityClass`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use medulla_server_api::api::persistence::PersistenceBackend as _;
use medulla_storage::object::commit::{CommitRefusal, GraphBootstrap, ObjectPublisher};
use medulla_storage::object::fetch::{ObjectFetcher, ObjectHead, ObjectReadRefusal};
use medulla_storage::{BackendSpecRefusal, DurabilityClass, StorageBackendSpec};
use object_store::ObjectStore;

mod client;
pub use client::resolve_object_store_client;
#[cfg(feature = "redb")]
mod shard_publisher;
#[cfg(feature = "redb")]
pub(crate) use shard_publisher::{attach_shard_publishers, ShardCipher};

/// The tenant the served engine files its graph versions under in the root:
/// `<root>/engine/<graph>/`.
pub const OBJECT_ROOT_TENANT: &str = "engine";

/// The environment variable naming the archive destination: a
/// `StorageBackendSpec` JSON of the `object_store` variant carrying secret
/// references only (`MEDULLA-ENGINE-PARITY-R016.1`).
pub const ARCHIVE_BACKEND_ENV: &str = "MEDULLA_ARCHIVE_BACKEND";

/// Build the archive named by [`ARCHIVE_BACKEND_ENV`]: `Ok(None)` when the
/// variable is unset or empty (no archiver runs), `Ok(Some(_))` for a spec that
/// resolves through [`resolve_object_store_client`], and `Err("<wire_code>:
/// <detail>")` for a spec that does not parse or resolve.
pub fn archive_from_env() -> Result<Option<medulla_storage::object::archive::ObjectArchive>, String>
{
    let raw = match std::env::var(ARCHIVE_BACKEND_ENV) {
        Ok(raw) if !raw.trim().is_empty() => raw,
        _ => return Ok(None),
    };
    let spec: StorageBackendSpec = serde_json::from_str(&raw).map_err(|error| {
        format!("OBJECT_STORE_CLIENT_INVALID: {ARCHIVE_BACKEND_ENV} is not a backend spec: {error}")
    })?;
    let (store, root) = resolve_object_store_client(&spec)
        .map_err(|refusal| format!("{}: {refusal:?}", refusal.wire_code()))?;
    Ok(Some(medulla_storage::object::archive::ObjectArchive::new(
        store, root,
    )))
}

/// The environment variable naming the served durable backend: unset or
/// `local` is redb, otherwise a `StorageBackendSpec` JSON.
pub const STORAGE_BACKEND_ENV: &str = "MEDULLA_STORAGE_BACKEND";

/// Open the backend named by [`STORAGE_BACKEND_ENV`] at server startup
/// (MEDULLA-OBJECT-STORAGE-BACKEND-R002.1): `Ok(None)` for the redb default,
/// `Ok(Some(_))` for an `object_store` spec that is admitted, resolves to a
/// client through [`resolve_object_store_client`] and opens through
/// [`ObjectStoreBackend::open`], and `Err("<wire_code>: <detail>")` for a spec
/// that does not parse, admit or open. Every refusal precedes any effect.
pub fn open_from_env() -> Result<Option<ObjectStoreBackend>, String> {
    let setting = std::env::var(STORAGE_BACKEND_ENV).ok();
    let spec = StorageBackendSpec::from_setting(setting.as_deref())
        .map_err(|refusal| format!("STORAGE_BACKEND_UNKNOWN: {refusal:?}"))?;
    spec.admit()
        .map_err(|refusal| match refusal.wire_code() {
            Some(code) => format!("{code}: {refusal:?}"),
            None => format!("storage_backend_refused: {refusal:?}"),
        })?;
    if matches!(spec, StorageBackendSpec::Local) {
        return Ok(None);
    }
    let (store, root) = resolve_object_store_client(&spec)
        .map_err(|refusal| format!("{}: {refusal:?}", refusal.wire_code()))?;
    ObjectStoreBackend::open(&spec, store, root)
        .map(Some)
        .map_err(|refusal| format!("{}: {refusal:?}", refusal.wire_code()))
}

/// Open the backend named by [`STORAGE_BACKEND_ENV`] and attach it to the local
/// store (MEDULLA-OBJECT-STORAGE-BACKEND-R002, R003): restore every graph the
/// root holds into `redb`, then make `redb` publish each commit to the root
/// before its ack. Returns the number of batches restored; `Ok(None)` for the
/// redb default. The restore client and the serving client are two clients,
/// because the restore runs on its own runtime.
#[cfg(feature = "redb")]
pub fn attach_from_env(
    redb: &mut crate::redb_backend::RedbBackend,
) -> Result<Option<usize>, String> {
    let Some(restoring) = open_from_env()? else {
        return Ok(None);
    };
    let restored = restoring.restore_blocking(redb)?;
    let publishing = open_from_env()?.ok_or_else(|| {
        format!("OBJECT_DURABILITY_UNDECLARED: {STORAGE_BACKEND_ENV} changed during startup")
    })?;
    redb.attach_object_store(Arc::new(publishing))?;
    Ok(Some(restored))
}

/// Typed refusal at open. `wire_code` is the literal the wire boundary keeps
/// stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectStoreOpenRefusal {
    /// The spec names no durability class.
    DurabilityUndeclared,
    /// The spec does not declare the `object_store` backend.
    WrongVariant,
    /// A secret reference names a variable that is unset or empty; the
    /// message names the field and never the value.
    SecretUnresolved {
        /// The spec field whose secret reference did not resolve.
        field: &'static str,
    },
    /// An `http` endpoint whose host is not loopback.
    PlainHttpNotLoopback,
    /// The object-store client could not be built from the resolved values.
    ClientBuild {
        /// Why the client builder refused.
        reason: String,
    },
    /// The spec itself refuses (an outside-database endpoint scheme).
    Spec(BackendSpecRefusal),
}

impl ObjectStoreOpenRefusal {
    /// The wire-stable refusal literal for this open refusal.
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::DurabilityUndeclared => "OBJECT_DURABILITY_UNDECLARED",
            Self::WrongVariant => "OBJECT_DURABILITY_UNDECLARED",
            Self::SecretUnresolved { .. } => "OBJECT_STORE_SECRET_UNRESOLVED",
            Self::PlainHttpNotLoopback => "OBJECT_STORE_PLAIN_HTTP_REFUSED",
            Self::ClientBuild { .. } => "OBJECT_STORE_CLIENT_INVALID",
            Self::Spec(refusal) => refusal
                .wire_code()
                .unwrap_or("OBJECT_STORE_CLIENT_INVALID"),
        }
    }
}

/// The object-store arm of the persistence layer. The local redb store is a
/// cache the root overrides: at open, a local store behind `HEAD` replays the
/// missing batches, and a local store ahead of `HEAD` refuses the open with
/// `OBJECT_LOCAL_AHEAD` (nothing is purged silently).
#[derive(Debug, Clone)]
pub struct ObjectStoreBackend {
    publisher: ObjectPublisher,
    fetcher: ObjectFetcher,
    durability: DurabilityClass,
    /// Set when step 4 lost the conditional put: this node stops publishing.
    stopped: Arc<AtomicBool>,
}

impl ObjectStoreBackend {
    /// Refuse an undeclared durability class before any effect
    /// (`OBJECT_DURABILITY_UNDECLARED`).
    pub fn open(
        spec: &StorageBackendSpec,
        store: Arc<dyn ObjectStore>,
        root: impl Into<String>,
    ) -> Result<Self, ObjectStoreOpenRefusal> {
        let durability = match spec {
            StorageBackendSpec::ObjectStore {
                durability: Some(durability),
                ..
            } => *durability,
            StorageBackendSpec::ObjectStore { .. } => {
                return Err(ObjectStoreOpenRefusal::DurabilityUndeclared);
            }
            StorageBackendSpec::Local => return Err(ObjectStoreOpenRefusal::WrongVariant),
        };
        let root = root.into();
        Ok(Self {
            publisher: ObjectPublisher::new(store.clone(), root.clone()),
            fetcher: ObjectFetcher::new(store, root),
            durability,
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The spec's durability class decides when a publish counts as durable.
    pub fn durability(&self) -> DurabilityClass {
        self.durability
    }

    /// Steps 2–4 of the commit-before-ack path over a commit that is final in
    /// the open local transaction. The commit chokepoint
    /// (`Shard::commit_batch`) calls this with the encoded `BatchSegment` of
    /// that commit; the returned `HEAD` carries the version the caller may ack
    /// only now (`ObjectStoreConditionalPut` means the store confirmed the
    /// conditional `HEAD` put). A [`CommitRefusal::PublishStale`] refusal fails
    /// the commit and the local transaction rolls back: the unacked write
    /// stays absent from both stores. `graph_version` is the graph version the
    /// commit leaves (`None` when it moves none). A graph with no `HEAD` is
    /// bootstrapped first from `bootstrap`, which is only called then.
    pub async fn commit_mutation_batch(
        &self,
        tenant: &str,
        graph: &str,
        batch_segment: &[u8],
        graph_version: Option<u64>,
        bootstrap: &dyn Fn() -> Result<GraphBootstrap, CommitRefusal>,
    ) -> Result<ObjectHead, CommitRefusal> {
        match self
            .publisher
            .publish(tenant, graph, batch_segment, graph_version)
            .await
        {
            Err(CommitRefusal::HeadMissing) => {
                self.publisher
                    .bootstrap(tenant, graph, &bootstrap()?)
                    .await?;
                self.publisher
                    .publish(tenant, graph, batch_segment, graph_version)
                    .await
            }
            other => other,
        }
    }

    /// Publish one commit (steps 2-4) as the one-entry `BatchSegment` of this
    /// commit, and return only after the `HEAD` fence. The entry carries the
    /// caller's batch and its material, which the restore re-commits through
    /// the local store by the path the original took. A lost fence is
    /// `OBJECT_PUBLISH_STALE`: this node stops publishing, and every later
    /// publish refuses with the same code until the node reopens from the root.
    /// `graph_snapshot` is the graph's row pages, read only when the graph has no
    /// root yet and already holds data. `seal_key_id` is the id of the key that
    /// seals this store's values (`None` with at-rest encryption off); the root's
    /// first manifest records it.
    pub async fn publish_commit(
        &self,
        commit: &medulla_store::redb_store::commit_publish::PublishedCommit<'_>,
        graph_snapshot: &dyn Fn() -> Result<Vec<u8>, String>,
        seal_key_id: Option<String>,
    ) -> Result<ObjectHead, String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(format!(
                "{}: this node stopped publishing after a lost HEAD fence",
                CommitRefusal::PublishStale { version: 0 }.wire_code()
            ));
        }
        let segment = rmp_serde::to_vec_named(&vec![commit.entry])
            .map_err(|error| format!("OBJECT_HEAD_MISSING: batch segment encode: {error}"))?;
        let bootstrap = || {
            // A graph that already holds data is bootstrapped with its row pages
            // as version zero; a graph with none starts empty.
            let snapshot = if commit.prior_graph_version > 0 {
                graph_snapshot().map_err(|detail| CommitRefusal::Remote { detail })?
            } else {
                Vec::new()
            };
            Ok(GraphBootstrap {
                graph_name: commit.graph_name.to_string(),
                graph_type: commit.graph_type,
                graph_version: commit.prior_graph_version,
                seal_key_id: seal_key_id.clone(),
                snapshot,
            })
        };
        self.commit_mutation_batch(
            OBJECT_ROOT_TENANT,
            commit.graph_fname,
            &segment,
            commit.graph_version,
            &bootstrap,
        )
        .await
        .map_err(|refusal| {
            if matches!(refusal, CommitRefusal::PublishStale { .. }) {
                self.stopped.store(true, Ordering::SeqCst);
            }
            format!("{}: {refusal:?}", refusal.wire_code())
        })
    }

    /// Restore every graph the root holds into `redb` on a runtime of its own
    /// (the open path is synchronous), through [`Self::restore_into`].
    #[cfg(feature = "redb")]
    pub fn restore_blocking(
        &self,
        redb: &crate::redb_backend::RedbBackend,
    ) -> Result<usize, String> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| format!("OBJECT_SEGMENT_MISSING: restore runtime: {error}"))?;
                    runtime.block_on(self.restore_into(redb))
                })
                .join()
                .map_err(|_| "OBJECT_SEGMENT_MISSING: the restore thread panicked".to_string())?
        })
    }

    /// Restore the root into the local store: for each graph with a `HEAD`,
    /// read the head version (`ObjectFetcher::materialize` verifies every
    /// segment digest) and register the graph under the name and type its
    /// manifest records.
    ///
    /// A segment is either row pages (the version-zero snapshot of a bootstrapped
    /// graph, or a segment of a graph copy: [`medulla_storage::object::rows`]) or a
    /// list of commit entries. The row pages restore the graph's rows and install
    /// the graph version they stand for, which is the head's `graph_version` less
    /// the number of entries after them: every published entry is one commit and
    /// moves the version by exactly one. The entries then re-commit, in order,
    /// through the local store by the path each was first committed through, and
    /// an entry whose version the local graph already holds is skipped. A read
    /// never writes to the root. A graph whose local version ends past the head's
    /// refuses with `OBJECT_LOCAL_AHEAD`; one that ends before it refuses with
    /// `OBJECT_SEGMENT_MISSING`. Returns the number of entries applied.
    #[cfg(feature = "redb")]
    pub async fn restore_into(
        &self,
        redb: &crate::redb_backend::RedbBackend,
    ) -> Result<usize, String> {
        use medulla_storage::object::rows::{decode_row_pages, is_row_page_segment};
        use medulla_store::redb_store::commit_publish::BatchSegmentEntry;

        let read = |refusal: ObjectReadRefusal| format!("{}: {refusal:?}", refusal.wire_code());
        let local_version = |graph_fname: &str| -> Result<u64, String> {
            Ok(redb
                .graph_identity(graph_fname)?
                .map(|(_, _, local_version)| local_version)
                .unwrap_or(0))
        };
        let mut applied = 0;
        for graph_fname in self
            .fetcher
            .graphs(OBJECT_ROOT_TENANT)
            .await
            .map_err(read)?
        {
            let head = match self.fetcher.head(OBJECT_ROOT_TENANT, &graph_fname).await {
                Ok(head) => head,
                // A directory with no `HEAD` is a bootstrap that never finished: it holds no version.
                Err(ObjectReadRefusal::SegmentMissing { .. }) => continue,
                Err(refusal) => return Err(read(refusal)),
            };
            let version = self
                .fetcher
                .materialize(OBJECT_ROOT_TENANT, &graph_fname, head.version)
                .await
                .map_err(read)?;
            // Sealed values restore as stored, so the store must hold the key that
            // sealed them: refuse before anything is registered or written.
            if let Some(sealed_under) = version.seal_key_id.as_deref() {
                if redb.seal_key_id()?.as_deref() != Some(sealed_under) {
                    return Err(format!(
                        "OBJECT_SEAL_KEY_MISSING: refusing to restore graph {graph_fname} from the object root: its \
                         values are sealed under encryption key {sealed_under}, which this \
                         store does not hold; configure that key before restoring"
                    ));
                }
            }
            redb.register_graph(&graph_fname, &version.graph_name, version.graph_type)
                .await?;

            let mut pages = decode_row_pages(&version.snapshot.bytes)
                .map_err(|error| format!("OBJECT_SEGMENT_MISSING: snapshot {}: {error}", version.snapshot.key))?;
            let mut entries: Vec<BatchSegmentEntry> = Vec::new();
            for segment in &version.batches {
                if segment.bytes.is_empty() {
                    continue;
                }
                if is_row_page_segment(&segment.bytes) {
                    pages.extend(decode_row_pages(&segment.bytes).map_err(|error| {
                        format!("OBJECT_SEGMENT_MISSING: row segment {}: {error}", segment.key)
                    })?);
                } else {
                    entries.extend(
                        rmp_serde::from_slice::<Vec<BatchSegmentEntry>>(&segment.bytes).map_err(
                            |error| {
                                format!(
                                    "OBJECT_SEGMENT_MISSING: batch segment {} does not decode: {error}",
                                    segment.key
                                )
                            },
                        )?,
                    );
                }
            }
            let base = version
                .graph_version
                .checked_sub(entries.len() as u64)
                .ok_or_else(|| {
                    format!(
                        "OBJECT_SEGMENT_MISSING: graph {graph_fname} is at version {} in the root, below its {} entries",
                        version.graph_version,
                        entries.len()
                    )
                })?;
            if base == 0 && !pages.is_empty() {
                return Err(format!(
                    "OBJECT_SEGMENT_MISSING: graph {graph_fname} holds row pages at graph version 0"
                ));
            }
            let mut local = local_version(&graph_fname)?;
            if base > 0 && local < base {
                let (mut nodes, mut edges, mut tables) = (Vec::new(), Vec::new(), Vec::new());
                for page in pages {
                    nodes.extend(page.nodes);
                    edges.extend(page.edges);
                    tables.extend(page.tables);
                }
                redb.restore_graph_rows(&graph_fname, nodes, edges, tables, base)
                    .await?;
                local = local_version(&graph_fname)?;
                if local != base {
                    return Err(format!(
                        "OBJECT_SEGMENT_MISSING: graph {graph_fname} restored to version {local}, the snapshot stands for {base}"
                    ));
                }
            }
            for (index, entry) in entries.iter().enumerate() {
                let target = base + index as u64 + 1;
                if local >= target {
                    continue;
                }
                self.replay_entry(redb, &graph_fname, entry).await?;
                local = local_version(&graph_fname)?;
                if local != target {
                    return Err(format!(
                        "OBJECT_SEGMENT_MISSING: replaying entry {} of graph {graph_fname} left it at version {local}, expected {target}",
                        index + 1
                    ));
                }
                applied += 1;
            }
            if local > version.graph_version {
                return Err(format!(
                    "{}: graph {graph_fname} is at version {local} locally and at {} in the root; the local copy is ahead and is not purged",
                    medulla_types::contract::ServerErrorCode::ObjectLocalAhead.as_str(),
                    version.graph_version
                ));
            }
            if local < version.graph_version {
                return Err(format!(
                    "OBJECT_SEGMENT_MISSING: graph {graph_fname} restored to version {local}, the root head is at {}",
                    version.graph_version
                ));
            }
        }
        Ok(applied)
    }

    /// Re-commit one published entry through the local store by the path its
    /// material names.
    #[cfg(feature = "redb")]
    async fn replay_entry(
        &self,
        redb: &crate::redb_backend::RedbBackend,
        graph_fname: &str,
        entry: &medulla_store::redb_store::commit_publish::BatchSegmentEntry,
    ) -> Result<(), String> {
        use medulla_server_api::api::persistence::{CrossModalCommitArgs, PersistenceBackend};
        use medulla_store::redb_store::commit_publish::CommitMaterial;

        let record = &entry.record;
        match &entry.material {
            CommitMaterial::Compact => {
                redb.commit_mutation_batch(
                    graph_fname,
                    &record.batch,
                    record.result_msgpack.as_deref(),
                    record.committed_at_ms,
                )
                .await?;
            }
            CommitMaterial::Staged {
                authoritative_state_msgpack,
                audited,
            } => {
                redb.commit_mutation_batch_state(
                    graph_fname,
                    &record.batch,
                    authoritative_state_msgpack.clone(),
                    record.result_msgpack.as_deref(),
                    record.committed_at_ms,
                    *audited,
                )
                .await?;
            }
            CommitMaterial::CrossModal {
                methods,
                vectors,
                blob_refs,
                measurements,
            } => {
                redb.commit_mutation_batch_crossmodal(CrossModalCommitArgs {
                    graph_fname,
                    batch: &record.batch,
                    methods,
                    vectors,
                    blob_refs,
                    measurements,
                    result_msgpack: record.result_msgpack.as_deref(),
                    committed_at_ms: record.committed_at_ms,
                })
                .await?;
            }
            CommitMaterial::ChangeEnvelope {
                envelope,
                source_budget,
            } => {
                match source_budget {
                    Some(authority) => {
                        redb.commit_repository_change_envelope(
                            graph_fname,
                            envelope,
                            authority.clone(),
                            record.committed_at_ms,
                        )
                        .await
                    }
                    None => {
                        redb.commit_change_envelope(graph_fname, envelope, record.committed_at_ms)
                            .await
                    }
                }?;
            }
            CommitMaterial::TopUp { transition_msgpack } => {
                #[cfg(feature = "raft")]
                {
                    let transition = rmp_serde::from_slice(transition_msgpack).map_err(|error| {
                        format!("OBJECT_SEGMENT_MISSING: top-up transition does not decode: {error}")
                    })?;
                    redb.commit_repository_enrichment_top_up(
                        graph_fname,
                        transition,
                        record.committed_at_ms,
                    )
                    .await?;
                }
                #[cfg(not(feature = "raft"))]
                {
                    let _ = transition_msgpack;
                    return Err(
                        "OBJECT_SEGMENT_MISSING: the root holds a top-up entry and this build has no raft feature"
                            .to_string(),
                    );
                }
            }
            CommitMaterial::Drain { methods } => {
                redb.replay_drain(graph_fname, methods.clone()).await?;
            }
            CommitMaterial::VersionBump => {
                redb.replay_version_bump(graph_fname).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
