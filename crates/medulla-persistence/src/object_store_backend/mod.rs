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

use std::sync::Arc;

use medulla_storage::object::commit::{CommitRefusal, ObjectPublisher};
use medulla_storage::object::fetch::ObjectHead;
use medulla_storage::{DurabilityClass, StorageBackendSpec};
use object_store::ObjectStore;

/// Typed refusal at open. `wire_code` is the literal the wire boundary keeps
/// stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectStoreOpenRefusal {
    /// The spec names no durability class.
    DurabilityUndeclared,
    /// The spec does not declare the `object_store` backend.
    WrongVariant,
}

impl ObjectStoreOpenRefusal {
    /// The wire-stable refusal literal for this open refusal.
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::DurabilityUndeclared => "OBJECT_DURABILITY_UNDECLARED",
            Self::WrongVariant => "OBJECT_DURABILITY_UNDECLARED",
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
    durability: DurabilityClass,
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
        Ok(Self {
            publisher: ObjectPublisher::new(store, root),
            durability,
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
        self.publisher.publish(tenant, graph, batch_segment).await
    }
}

#[cfg(test)]
mod tests;
