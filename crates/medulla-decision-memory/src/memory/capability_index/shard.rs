//! One shard: incremental upsert, tombstone, compaction and the embedding pin
//! (MEDULLA-DECISION-ENGINE-R172.2.3, R172.6).
//!
//! Readers load an `Arc<ShardSnapshot>` and never lock. One writer at a time
//! applies a batch to a working copy and publishes it as a new epoch; a failed
//! batch publishes nothing. Compaction and a re-index build a new generation
//! beside the live one and swap it atomically, so a reader never sees a mix.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use medulla_decision_types::decision::capability_designation::{CapabilityRecord, Q32_ONE};
use medulla_types::contract::OpaqueId;

use super::arena::Arena;
use super::postings::Postings;

/// Why the index refused a change or a question. Each variant maps to one
/// refusal code of the `capability_designation` family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRefusal {
    /// The record or question names another embedding version than the shard pin.
    EmbeddingVersionMismatch {
        /// The version the shard is pinned to.
        expected: String,
        /// The version that was offered.
        actual: String,
    },
    /// The record or question has another dimension than the shard pin.
    DimMismatch {
        /// The dimension the shard is pinned to.
        expected: u16,
        /// The dimension that was offered.
        actual: u16,
    },
    /// The shard already holds its limit of live records.
    CapacityExceeded {
        /// The `designation.max_records_per_shard` bound.
        limit: usize,
    },
    /// A vector is not finite, or does not match its record.
    MalformedVector {
        /// What is wrong with it.
        detail: String,
    },
    /// The record failed its own validation.
    InvalidRecord {
        /// What is wrong with it.
        detail: String,
    },
    /// A tombstone named an entity the shard does not hold.
    UnknownEntity {
        /// The entity that was named.
        entity_id: OpaqueId,
    },
}

impl IndexRefusal {
    /// The stable refusal code of this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmbeddingVersionMismatch { .. } => "DESIGNATION_EMBEDDING_VERSION_MISMATCH",
            Self::DimMismatch { .. } => "DESIGNATION_DIM_MISMATCH",
            Self::CapacityExceeded { .. } => "DESIGNATION_INDEX_CAPACITY_EXCEEDED",
            Self::MalformedVector { .. } | Self::InvalidRecord { .. } => {
                "DESIGNATION_MALFORMED_QUERY_VECTOR"
            }
            Self::UnknownEntity { .. } => "DESIGNATION_UNKNOWN_ENTITY",
        }
    }
}

/// The embedding space one shard is pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingPin {
    /// The pinned embedding version.
    pub embedding_version: String,
    /// The pinned dimension.
    pub dim: u16,
}

/// One change of a batch.
#[derive(Debug, Clone)]
pub enum Change {
    /// Insert the record, or replace the record of the same entity.
    Upsert(CapabilityRecord, Vec<f32>),
    /// Remove an entity.
    Tombstone(OpaqueId),
}

/// The bounds one shard works under, read from the `designation.*` knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardLimits {
    /// `designation.max_records_per_shard`.
    pub max_records: usize,
    /// `designation.tombstone_compact_ratio` on Q32.
    pub compact_ratio_q32: i64,
}

/// One published, immutable generation of a shard.
#[derive(Debug, Clone)]
pub struct ShardSnapshot {
    epoch: u64,
    generation: u64,
    pin: Option<EmbeddingPin>,
    records: Vec<Option<CapabilityRecord>>,
    by_entity: BTreeMap<OpaqueId, u32>,
    live: Vec<u32>,
    postings: Postings,
    arena: Arena,
}

impl ShardSnapshot {
    fn empty() -> Self {
        Self {
            epoch: 0,
            generation: 0,
            pin: None,
            records: Vec::new(),
            by_entity: BTreeMap::new(),
            live: Vec::new(),
            postings: Postings::default(),
            arena: Arena::new(1),
        }
    }

    /// The epoch: one more after each published batch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The generation: one more after each compaction or re-index.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The pin, or `None` while the shard holds no live record.
    pub fn pin(&self) -> Option<&EmbeddingPin> {
        self.pin.as_ref()
    }

    /// How many live records the shard holds.
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// How many slots are tombstoned and not yet compacted.
    pub fn tombstone_count(&self) -> usize {
        self.records.len() - self.live.len()
    }

    /// The live record of `entity_id`.
    pub fn record(&self, entity_id: &OpaqueId) -> Option<&CapabilityRecord> {
        self.by_entity
            .get(entity_id)
            .and_then(|slot| self.records[*slot as usize].as_ref())
    }

    /// The record at a slot, if live.
    pub fn record_at(&self, slot: u32) -> Option<&CapabilityRecord> {
        self.records.get(slot as usize).and_then(Option::as_ref)
    }

    /// The normalized stored vector at a slot.
    pub fn vector_at(&self, slot: u32) -> &[f32] {
        self.arena.vector(slot)
    }

    /// Whether a question in `version` and `dim` fits the shard's embedding space.
    pub fn check_space(&self, version: &str, dim: u16) -> Result<(), IndexRefusal> {
        match &self.pin {
            Some(pin) if pin.embedding_version != version => {
                Err(IndexRefusal::EmbeddingVersionMismatch {
                    expected: pin.embedding_version.clone(),
                    actual: version.to_string(),
                })
            }
            Some(pin) if pin.dim != dim => Err(IndexRefusal::DimMismatch {
                expected: pin.dim,
                actual: dim,
            }),
            _ => Ok(()),
        }
    }

    /// The live slots that pass the capability and policy-tag filter.
    pub fn filtered_pool(
        &self,
        required: &BTreeSet<String>,
        tags: &BTreeSet<String>,
        subtypes: &BTreeMap<String, BTreeSet<String>>,
    ) -> Vec<u32> {
        self.postings
            .filtered_pool(&self.live, required, tags, subtypes)
    }

    /// The best `shortlist` slots of `pool` for a unit `query`, by f32 cosine.
    pub fn search(
        &self,
        query: &[f32],
        pool: &[u32],
        shortlist: usize,
        flat_threshold: usize,
    ) -> Vec<(u32, f32)> {
        if self.live.is_empty() {
            return Vec::new();
        }
        self.arena.search(query, pool, shortlist, flat_threshold)
    }

    fn pin_for(&self, record: &CapabilityRecord) -> Result<(), IndexRefusal> {
        self.check_space(&record.embedding_version, record.dim)
    }

    fn drop_slot(&mut self, slot: u32) {
        if let Some(record) = self.records[slot as usize].take() {
            self.postings
                .remove(slot, &record.capabilities, &record.policy_tags);
            self.by_entity.remove(&record.entity_id);
            if let Ok(at) = self.live.binary_search(&slot) {
                self.live.remove(at);
            }
        }
        if self.live.is_empty() {
            self.pin = None;
        }
    }

    fn apply_change(&mut self, change: Change, limits: &ShardLimits) -> Result<(), IndexRefusal> {
        match change {
            Change::Tombstone(entity_id) => {
                let slot = *self
                    .by_entity
                    .get(&entity_id)
                    .ok_or(IndexRefusal::UnknownEntity { entity_id })?;
                self.drop_slot(slot);
                Ok(())
            }
            Change::Upsert(record, vector) => {
                record
                    .validate()
                    .map_err(|detail| IndexRefusal::InvalidRecord { detail })?;
                if vector.len() != usize::from(record.dim) || vector.iter().any(|v| !v.is_finite())
                {
                    return Err(IndexRefusal::MalformedVector {
                        detail: format!(
                            "a vector holds {} finite values, the record names dim {}",
                            vector.len(),
                            record.dim
                        ),
                    });
                }
                self.pin_for(&record)?;
                let replacing = self.by_entity.get(&record.entity_id).copied();
                if replacing.is_none() && self.live.len() >= limits.max_records {
                    return Err(IndexRefusal::CapacityExceeded {
                        limit: limits.max_records,
                    });
                }
                if let Some(slot) = replacing {
                    self.drop_slot(slot);
                }
                if self.live.is_empty() && self.arena_dim() != usize::from(record.dim) {
                    self.reset_for_dim(usize::from(record.dim));
                }
                if self.pin.is_none() {
                    self.pin = Some(EmbeddingPin {
                        embedding_version: record.embedding_version.clone(),
                        dim: record.dim,
                    });
                }
                let slot = self.arena.push(&vector);
                self.postings
                    .add(slot, &record.capabilities, &record.policy_tags);
                self.by_entity.insert(record.entity_id.clone(), slot);
                self.live.push(slot);
                self.records.push(Some(record));
                Ok(())
            }
        }
    }

    /// A drained shard takes a new dimension: its tombstoned slots are dropped
    /// with the old arena, which is a new generation.
    fn reset_for_dim(&mut self, dim: usize) {
        self.records.clear();
        self.by_entity.clear();
        self.live.clear();
        self.postings = Postings::default();
        self.arena = Arena::new(dim);
        self.generation += 1;
    }

    fn arena_dim(&self) -> usize {
        self.arena.vector_dim()
    }

    /// A new generation holding only the live records, in slot order.
    fn compacted(&self, limits: &ShardLimits) -> ShardSnapshot {
        let mut next = ShardSnapshot::empty();
        next.epoch = self.epoch;
        next.generation = self.generation + 1;
        for slot in &self.live {
            let record = self.records[*slot as usize]
                .clone()
                .expect("a live slot holds a record");
            let vector = self.arena.vector(*slot).to_vec();
            next.apply_change(Change::Upsert(record, vector), limits)
                .expect("live records re-insert into an empty generation");
        }
        next
    }

    fn over_ratio(&self, limits: &ShardLimits) -> bool {
        let total = self.records.len() as i128;
        total > 0
            && (self.tombstone_count() as i128) * i128::from(Q32_ONE)
                > i128::from(limits.compact_ratio_q32) * total
    }
}

/// A shard: the writer side of a stream of snapshots.
#[derive(Debug)]
pub struct Shard {
    current: ArcSwap<ShardSnapshot>,
    writer: Mutex<()>,
    limits: ShardLimits,
}

impl Shard {
    /// An empty shard under `limits`.
    pub fn new(limits: ShardLimits) -> Self {
        Self {
            current: ArcSwap::from_pointee(ShardSnapshot::empty()),
            writer: Mutex::new(()),
            limits,
        }
    }

    /// The live snapshot. Never blocks a writer and is never blocked by one.
    pub fn snapshot(&self) -> Arc<ShardSnapshot> {
        self.current.load_full()
    }

    /// Apply a batch all or nothing and publish one new epoch. Compaction runs
    /// when tombstones exceed the knob ratio, in the same publish.
    pub fn apply(&self, changes: Vec<Change>) -> Result<u64, IndexRefusal> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut work = (*self.current.load_full()).clone();
        for change in changes {
            work.apply_change(change, &self.limits)?;
        }
        work.epoch += 1;
        if work.over_ratio(&self.limits) {
            work = work.compacted(&self.limits);
        }
        let epoch = work.epoch;
        self.current.store(Arc::new(work));
        Ok(epoch)
    }

    /// Insert or replace one record.
    pub fn upsert(&self, record: CapabilityRecord, vector: Vec<f32>) -> Result<u64, IndexRefusal> {
        self.apply(vec![Change::Upsert(record, vector)])
    }

    /// Remove one entity.
    pub fn tombstone(&self, entity_id: &OpaqueId) -> Result<u64, IndexRefusal> {
        self.apply(vec![Change::Tombstone(entity_id.clone())])
    }

    /// Compact now: a new generation beside the live one, swapped atomically.
    pub fn compact(&self) -> u64 {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = self.current.load().compacted(&self.limits);
        next.epoch += 1;
        let epoch = next.epoch;
        self.current.store(Arc::new(next));
        epoch
    }

    /// Re-index to a new embedding version: build a second generation from
    /// `records` beside the live one and swap it in. All records must share one
    /// version and dimension, else nothing changes.
    pub fn reindex(&self, records: Vec<(CapabilityRecord, Vec<f32>)>) -> Result<u64, IndexRefusal> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let live = self.current.load_full();
        let mut next = ShardSnapshot::empty();
        next.generation = live.generation + 1;
        next.epoch = live.epoch + 1;
        for (record, vector) in records {
            next.apply_change(Change::Upsert(record, vector), &self.limits)?;
        }
        let epoch = next.epoch;
        self.current.store(Arc::new(next));
        Ok(epoch)
    }
}
