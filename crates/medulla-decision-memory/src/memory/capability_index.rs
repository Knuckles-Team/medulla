//! The resident capability index (MEDULLA-DECISION-ENGINE-R172.2).
//!
//! One shard per tenant plus one commons shard; the index stores nothing
//! durable. A reader loads the visible snapshots (its tenant shard and the
//! commons shard) and never takes a lock a writer holds. The query path takes
//! no global lock: the shard map lock is held only to clone shard handles.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use medulla_decision_types::decision::capability_designation::CapabilityRecord;
use medulla_types::contract::{OpaqueId, TenantId};

pub mod arena;
pub mod postings;
pub mod shard;
#[cfg(test)]
mod tests;

pub use shard::{Change, EmbeddingPin, IndexRefusal, Shard, ShardLimits, ShardSnapshot};

/// One candidate read from the index: the record, its stored unit vector and
/// its exact cosine to the query.
#[derive(Debug, Clone)]
pub struct Hit {
    /// The candidate's record.
    pub record: CapabilityRecord,
    /// The stored vector, L2-normalized.
    pub vector: Vec<f32>,
    /// The exact f32 cosine to the query.
    pub cosine: f32,
}

/// What one question read from the visible shards.
#[derive(Debug, Clone)]
pub struct PoolRead {
    /// Live records of the visible shards, before the capability filter.
    pub candidate_pool_size: u64,
    /// Records left after the capability and policy-tag filter.
    pub capability_filtered: u64,
    /// The sum of the visible shards' epochs: it grows with every change batch.
    pub index_epoch: u64,
    /// The embedding version of the visible shards, when any holds a record.
    pub embedding_version: Option<String>,
    /// The best candidates, cosine descending, entity id ascending.
    pub hits: Vec<Hit>,
}

/// The resident index: tenant shards plus the commons shard.
#[derive(Debug)]
pub struct CapabilityIndex {
    shards: RwLock<BTreeMap<Option<TenantId>, Arc<Shard>>>,
    limits: ShardLimits,
}

impl CapabilityIndex {
    /// An empty index whose shards work under `limits`.
    pub fn new(limits: ShardLimits) -> Self {
        Self {
            shards: RwLock::new(BTreeMap::new()),
            limits,
        }
    }

    fn shard_for(&self, tenant: Option<&TenantId>) -> Arc<Shard> {
        let key = tenant.cloned();
        if let Some(shard) = self
            .shards
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Arc::clone(shard);
        }
        let mut shards = self.shards.write().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            shards
                .entry(key)
                .or_insert_with(|| Arc::new(Shard::new(self.limits))),
        )
    }

    fn existing(&self, tenant: Option<&TenantId>) -> Option<Arc<Shard>> {
        self.shards
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&tenant.cloned())
            .cloned()
    }

    /// The shard of a tenant, or of the commons for `None`.
    pub fn shard(&self, tenant: Option<&TenantId>) -> Option<Arc<Shard>> {
        self.existing(tenant)
    }

    /// Insert or replace a record in the shard its tenant names.
    pub fn upsert(&self, record: CapabilityRecord, vector: Vec<f32>) -> Result<u64, IndexRefusal> {
        self.shard_for(record.tenant_id.as_ref())
            .upsert(record, vector)
    }

    /// Remove an entity from the shard of `tenant` (`None`: the commons).
    pub fn tombstone(
        &self,
        tenant: Option<&TenantId>,
        entity_id: &OpaqueId,
    ) -> Result<u64, IndexRefusal> {
        match self.existing(tenant) {
            Some(shard) => shard.tombstone(entity_id),
            None => Err(IndexRefusal::UnknownEntity {
                entity_id: entity_id.clone(),
            }),
        }
    }

    /// The snapshots a tenant may read: its own shard, then the commons shard.
    pub fn visible(&self, tenant: &TenantId) -> Vec<Arc<ShardSnapshot>> {
        [Some(tenant), None]
            .into_iter()
            .filter_map(|key| self.existing(key))
            .map(|shard| shard.snapshot())
            .collect()
    }

    /// Read the best `shortlist` candidates for a unit `query` of `version`.
    ///
    /// A visible shard pinned to another version or dimension refuses the whole
    /// read, naming the expected and offered value. `subtypes` maps each
    /// required capability to the classes below it, from the schema closure the
    /// caller holds; the index reads no schema.
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        &self,
        tenant: &TenantId,
        version: &str,
        query: &[f32],
        required: &BTreeSet<String>,
        tags: &BTreeSet<String>,
        subtypes: &BTreeMap<String, BTreeSet<String>>,
        shortlist: usize,
        flat_threshold: usize,
    ) -> Result<PoolRead, IndexRefusal> {
        let dim = u16::try_from(query.len()).unwrap_or(u16::MAX);
        let snapshots = self.visible(tenant);
        for snapshot in &snapshots {
            snapshot.check_space(version, dim)?;
        }
        let mut read = PoolRead {
            candidate_pool_size: 0,
            capability_filtered: 0,
            index_epoch: 0,
            embedding_version: None,
            hits: Vec::new(),
        };
        for snapshot in &snapshots {
            read.index_epoch += snapshot.epoch();
            read.candidate_pool_size += snapshot.live_count() as u64;
            if let Some(pin) = snapshot.pin() {
                read.embedding_version
                    .get_or_insert(pin.embedding_version.clone());
            }
            let pool = snapshot.filtered_pool(required, tags, subtypes);
            read.capability_filtered += pool.len() as u64;
            for (slot, cosine) in snapshot.search(query, &pool, shortlist, flat_threshold) {
                if let Some(record) = snapshot.record_at(slot) {
                    read.hits.push(Hit {
                        record: record.clone(),
                        vector: snapshot.vector_at(slot).to_vec(),
                        cosine,
                    });
                }
            }
        }
        read.hits.sort_by(|a, b| {
            b.cosine
                .total_cmp(&a.cosine)
                .then_with(|| a.record.entity_id.cmp(&b.record.entity_id))
        });
        read.hits.truncate(shortlist);
        Ok(read)
    }
}
