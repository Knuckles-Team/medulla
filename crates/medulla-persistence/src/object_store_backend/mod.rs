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

use medulla_protocol_types::mutation_batch::{
    MutationBatch, MutationBatchCommit, MutationBatchRecord, MutationBatchStatus,
};
use medulla_storage::object::commit::{CommitRefusal, ObjectPublisher};
use medulla_storage::object::fetch::{ObjectFetcher, ObjectHead, ObjectReadRefusal};
use medulla_storage::{BackendSpecRefusal, DurabilityClass, StorageBackendSpec};
use object_store::ObjectStore;

mod client;
pub use client::resolve_object_store_client;

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
    redb.attach_object_store(Arc::new(publishing));
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
/// cache the root overrides: at open, a local store ahead of `HEAD` is
/// discarded and rebuilt from the root; a local store behind `HEAD` replays
/// the missing batches (the fixing of the cache is the open path of R002 and
/// R003's rebuild-from-root arm).
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

    /// Steps 2–4 of the commit-before-ack path over an already-durable local
    /// commit. The served caller (`PersistenceBackend::commit_mutation_batch`)
    /// runs step 1 (the local redb commit) first and calls this with the
    /// encoded `BatchSegment` of that commit; the returned `HEAD` carries the
    /// version the caller may ack only now (`ObjectStoreConditionalPut`
    /// means the store confirmed the conditional `HEAD` put). A
    /// [`CommitRefusal::PublishStale`] refusal must discard the local commit
    /// and re-fetch `HEAD` — the unacked write stays absent from the root.
    pub async fn commit_mutation_batch(
        &self,
        tenant: &str,
        graph: &str,
        batch_segment: &[u8],
    ) -> Result<ObjectHead, CommitRefusal> {
        match self.publisher.publish(tenant, graph, batch_segment).await {
            Err(CommitRefusal::HeadMissing) => {
                self.publisher.bootstrap(tenant, graph).await?;
                self.publisher.publish(tenant, graph, batch_segment).await
            }
            other => other,
        }
    }

    /// Publish one batch that is already durable locally (steps 2-4) as the
    /// one-record `BatchSegment` of this commit, and return only after the
    /// `HEAD` fence. The record carries the caller's batch, which the restore
    /// re-commits through the local store. A lost fence is
    /// `OBJECT_PUBLISH_STALE`: this node stops publishing, and every later
    /// publish refuses with the same code until the node reopens from the root.
    pub async fn publish_batch(
        &self,
        graph_fname: &str,
        batch: &MutationBatch,
        result_msgpack: Option<&[u8]>,
        committed_at_ms: u64,
        commit: &MutationBatchCommit,
    ) -> Result<ObjectHead, String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(format!(
                "{}: this node stopped publishing after a lost HEAD fence",
                CommitRefusal::PublishStale { version: 0 }.wire_code()
            ));
        }
        let record = MutationBatchRecord {
            batch: batch.clone(),
            identity: batch.identity.clone(),
            status: MutationBatchStatus::Committed,
            committed_version: commit.record.committed_version.clone(),
            result_msgpack: result_msgpack.map(ToOwned::to_owned),
            committed_at_ms,
        };
        let segment = rmp_serde::to_vec_named(&vec![record])
            .map_err(|error| format!("OBJECT_HEAD_MISSING: batch segment encode: {error}"))?;
        self.commit_mutation_batch(OBJECT_ROOT_TENANT, graph_fname, &segment)
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
    /// segment digest) and re-commit its batch records, in order, through the
    /// local store's mutation kernel. A batch the local store already holds
    /// replays as a no-op. A read never writes to the root. Returns the number
    /// of records applied.
    #[cfg(feature = "redb")]
    pub async fn restore_into(
        &self,
        redb: &crate::redb_backend::RedbBackend,
    ) -> Result<usize, String> {
        use medulla_mutation_types::mutation_scope::MutationScope;
        use medulla_protocol_types::protocol::GraphType;
        use medulla_server_api::api::persistence::PersistenceBackend;

        let read = |refusal: ObjectReadRefusal| format!("{}: {refusal:?}", refusal.wire_code());
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
            let mut registered = false;
            for segment in &version.batches {
                let records: Vec<MutationBatchRecord> = rmp_serde::from_slice(&segment.bytes)
                    .map_err(|error| {
                        format!(
                            "OBJECT_SEGMENT_MISSING: batch segment {} does not decode: {error}",
                            segment.key
                        )
                    })?;
                for record in records {
                    if !registered {
                        if let MutationScope::Graph { graph } = record.batch.identity.scope() {
                            let graph_type = if graph.as_str()
                                == medulla_identity_types::tenancy::COMMONS_GRAPH
                            {
                                GraphType::Commons
                            } else {
                                GraphType::Global
                            };
                            redb.register_graph(&graph_fname, graph.as_str(), graph_type)
                                .await?;
                            registered = true;
                        }
                    }
                    redb.commit_mutation_batch(
                        &graph_fname,
                        &record.batch,
                        record.result_msgpack.as_deref(),
                        record.committed_at_ms,
                    )
                    .await?;
                    applied += 1;
                }
            }
        }
        Ok(applied)
    }
}

#[cfg(test)]
mod tests;
