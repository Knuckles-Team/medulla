//! Tests of the resident capability index (MEDULLA-DECISION-ENGINE-R172.2.x, R172.6).

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::capability_designation::{CapabilityRecord, EntityKind};
use medulla_types::contract::{OpaqueId, TenantId};

use super::arena::{dot, normalized, Arena};
use super::postings::Postings;
use super::shard::{Change, IndexRefusal, Shard, ShardLimits};
use super::CapabilityIndex;

/// A small seeded generator, so every test is reproducible.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 20001) as f32 / 10000.0 - 1.0
    }

    fn vector(&mut self, dim: usize) -> Vec<f32> {
        (0..dim).map(|_| self.unit()).collect()
    }
}

fn limits() -> ShardLimits {
    ShardLimits {
        max_records: 1_000_000,
        compact_ratio_q32: (1i64 << 32) / 5,
    }
}

fn record(
    id: &str,
    tenant: Option<&str>,
    caps: &[&str],
    version: &str,
    dim: u16,
) -> CapabilityRecord {
    CapabilityRecord {
        tenant_id: tenant.map(|t| TenantId::new(t).unwrap()),
        entity_id: OpaqueId::new(id).unwrap(),
        entity_kind: EntityKind::Tool,
        capabilities: caps.iter().map(|c| (*c).to_string()).collect(),
        ontology_type: None,
        policy_tags: BTreeSet::new(),
        swappable_with: BTreeSet::new(),
        embedding_version: version.into(),
        dim,
        record_revision: 1,
        applies_to: BTreeSet::new(),
        executes: BTreeSet::new(),
        pack: None,
    }
}

fn names(items: &[usize], prefix: &str) -> BTreeSet<String> {
    items.iter().map(|i| format!("{prefix}{i}")).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R172.2.1
#[test]
fn postings_intersect_and_union_subtypes() {
    let mut rng = Lcg(7);
    let mut postings = Postings::default();
    let mut rows: Vec<(BTreeSet<String>, BTreeSet<String>)> = Vec::new();
    for slot in 0..400u32 {
        let caps: Vec<usize> = (0..rng.below(4)).map(|_| rng.below(12) as usize).collect();
        let tags: Vec<usize> = (0..rng.below(3)).map(|_| rng.below(5) as usize).collect();
        let (caps, tags) = (names(&caps, "cap"), names(&tags, "tag"));
        postings.add(slot, &caps, &tags);
        rows.push((caps, tags));
    }
    let universe: Vec<u32> = (0..400).collect();
    // cap0 has two subtypes; cap5 has one; the rest none.
    let subtypes: BTreeMap<String, BTreeSet<String>> = [
        ("cap0".to_string(), names(&[0, 1, 2], "cap")),
        ("cap5".to_string(), names(&[5, 6], "cap")),
    ]
    .into_iter()
    .collect();
    for _ in 0..300 {
        let required: Vec<usize> = (0..rng.below(4)).map(|_| rng.below(14) as usize).collect();
        let tags: Vec<usize> = (0..rng.below(3)).map(|_| rng.below(6) as usize).collect();
        let (required, tags) = (names(&required, "cap"), names(&tags, "tag"));
        let got = postings.filtered_pool(&universe, &required, &tags, &subtypes);
        let brute: Vec<u32> = rows
            .iter()
            .enumerate()
            .filter(|(_, (caps, have))| {
                required.iter().all(|need| {
                    let family = subtypes
                        .get(need)
                        .cloned()
                        .unwrap_or_else(|| [need.clone()].into());
                    family.iter().any(|c| caps.contains(c))
                }) && tags.is_subset(have)
            })
            .map(|(slot, _)| slot as u32)
            .collect();
        assert_eq!(got, brute, "required {required:?} tags {tags:?}");
    }
    // An empty filter admits the whole universe; removal drops a slot from every list.
    let all = postings.filtered_pool(&universe, &BTreeSet::new(), &BTreeSet::new(), &subtypes);
    assert_eq!(all.len(), 400);
    let (caps, tags) = rows[3].clone();
    postings.remove(3, &caps, &tags);
    for cap in &caps {
        let got = postings.filtered_pool(
            &universe,
            &[cap.clone()].into(),
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(!got.contains(&3));
    }
}

// spec: MEDULLA-DECISION-ENGINE-R172.2.2
#[test]
fn sq8_scan_with_rescore_matches_exact_top_k() {
    let (n, dim, k, oversample) = (2000usize, 64usize, 5usize, 4usize);
    let mut rng = Lcg(42);
    let mut arena = Arena::new(dim);
    let mut stored = Vec::new();
    for _ in 0..n {
        let v = rng.vector(dim);
        arena.push(&v);
        stored.push(normalized(&v));
    }
    let pool: Vec<u32> = (0..n as u32).collect();
    let mut total = 0.0;
    let queries = 60;
    for _ in 0..queries {
        let q = normalized(&rng.vector(dim));
        let mut exact: Vec<(u32, f32)> = stored
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, dot(&q, v)))
            .collect();
        exact.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let truth: BTreeSet<u32> = exact.iter().take(k).map(|(i, _)| *i).collect();
        let got = arena.search(&q, &pool, oversample * k, 4096);
        // The shortlist is exact f32 scored: its order is the f32 order.
        assert!(got.windows(2).all(|w| w[0].1 >= w[1].1));
        let found = got
            .iter()
            .take(k)
            .filter(|(i, _)| truth.contains(i))
            .count();
        total += found as f64 / k as f64;
    }
    let recall = total / f64::from(queries);
    assert!(recall >= 0.95, "recall@5 was {recall}");
    // A pool above the flat threshold goes through the graph and still honours the pool.
    let sub: Vec<u32> = (0..n as u32).filter(|i| i % 2 == 0).collect();
    let q = normalized(&rng.vector(dim));
    let graph = arena.search(&q, &sub, 20, 10);
    assert!(!graph.is_empty());
    assert!(graph.iter().all(|(slot, _)| slot % 2 == 0));
}

fn top(shard: &Shard, query: &[f32], k: usize) -> Vec<(OpaqueId, i64)> {
    let snapshot = shard.snapshot();
    let pool = snapshot.filtered_pool(&BTreeSet::new(), &BTreeSet::new(), &BTreeMap::new());
    let q = normalized(query);
    snapshot
        .search(&q, &pool, k, 1_000_000)
        .into_iter()
        .map(|(slot, cos)| {
            (
                snapshot.record_at(slot).unwrap().entity_id.clone(),
                (cos * 1e6) as i64,
            )
        })
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R172.2.3
#[test]
fn incremental_upsert_and_tombstone_match_rebuild() {
    let dim = 8usize;
    let mut rng = Lcg(11);
    let shard = Shard::new(limits());
    let mut live: BTreeMap<String, (CapabilityRecord, Vec<f32>)> = BTreeMap::new();
    for _ in 0..300 {
        let id = format!("e{}", rng.below(60));
        if rng.below(4) == 0 && live.contains_key(&id) {
            shard
                .tombstone(&OpaqueId::new(id.clone()).unwrap())
                .unwrap();
            live.remove(&id);
        } else {
            let rec = record(&id, Some("tenant-a"), &["cap:x"], "emb-v1", dim as u16);
            let v = rng.vector(dim);
            shard.upsert(rec.clone(), v.clone()).unwrap();
            live.insert(id, (rec, v));
        }
    }
    // A rebuild from the survivors answers the same top-k as the incremental shard.
    let rebuilt = Shard::new(limits());
    rebuilt.reindex(live.values().cloned().collect()).unwrap();
    assert_eq!(shard.snapshot().live_count(), live.len());
    let queries: Vec<Vec<f32>> = (0..20).map(|_| rng.vector(dim)).collect();
    for q in &queries {
        assert_eq!(top(&shard, q, 7), top(&rebuilt, q, 7));
    }
    // Compaction keeps the same top-k and drops the tombstones into a new generation.
    let before = shard.snapshot();
    let generation = before.generation();
    shard.compact();
    let after = shard.snapshot();
    assert_eq!(after.tombstone_count(), 0);
    assert!(after.generation() > generation);
    assert!(after.epoch() > before.epoch());
    for q in &queries {
        assert_eq!(top(&shard, q, 7), top(&rebuilt, q, 7));
    }
    // A reader that held the old snapshot is unaffected by the swap.
    assert_eq!(before.generation(), generation);

    // An overflow refuses and leaves the shard unchanged: no partial write.
    let small = Shard::new(ShardLimits {
        max_records: 2,
        compact_ratio_q32: (1i64 << 32) / 5,
    });
    small
        .upsert(record("a", None, &[], "emb-v1", 2), vec![1.0, 0.0])
        .unwrap();
    small
        .upsert(record("b", None, &[], "emb-v1", 2), vec![0.0, 1.0])
        .unwrap();
    let epoch = small.snapshot().epoch();
    let refused = small
        .apply(vec![
            Change::Tombstone(OpaqueId::new("a").unwrap()),
            Change::Upsert(record("c", None, &[], "emb-v1", 2), vec![1.0, 1.0]),
            Change::Upsert(record("d", None, &[], "emb-v1", 2), vec![1.0, 2.0]),
            Change::Upsert(record("e", None, &[], "emb-v1", 2), vec![1.0, 3.0]),
        ])
        .unwrap_err();
    assert_eq!(refused, IndexRefusal::CapacityExceeded { limit: 2 });
    assert_eq!(refused.code(), "DESIGNATION_INDEX_CAPACITY_EXCEEDED");
    assert_eq!(small.snapshot().epoch(), epoch);
    assert_eq!(small.snapshot().live_count(), 2);
    // Replacing an entity at the bound is not an overflow.
    small
        .upsert(record("a", None, &[], "emb-v1", 2), vec![0.5, 0.5])
        .unwrap();
    // Tombstoning more than the ratio compacts inside the publish.
    let churn = Shard::new(limits());
    for i in 0..10 {
        churn
            .upsert(
                record(&format!("x{i}"), None, &[], "emb-v1", 2),
                vec![i as f32, 1.0],
            )
            .unwrap();
    }
    for i in 0..5 {
        churn
            .tombstone(&OpaqueId::new(format!("x{i}")).unwrap())
            .unwrap();
    }
    let held = churn.snapshot();
    assert!(held.tombstone_count() * 5 <= held.live_count() + held.tombstone_count());
    assert_eq!(held.live_count(), 5);
}

// spec: MEDULLA-DECISION-ENGINE-R172.6
#[test]
fn version_pin_refuses_mixed_spaces() {
    let shard = Shard::new(limits());
    shard
        .upsert(record("a", None, &[], "emb-v1", 3), vec![1.0, 0.0, 0.0])
        .unwrap();
    // Another version or dim refuses on upsert, naming expected and actual.
    let version = shard
        .upsert(record("b", None, &[], "emb-v2", 3), vec![1.0, 0.0, 0.0])
        .unwrap_err();
    assert_eq!(
        version,
        IndexRefusal::EmbeddingVersionMismatch {
            expected: "emb-v1".into(),
            actual: "emb-v2".into()
        }
    );
    assert_eq!(version.code(), "DESIGNATION_EMBEDDING_VERSION_MISMATCH");
    let dim = shard
        .upsert(
            record("b", None, &[], "emb-v1", 4),
            vec![1.0, 0.0, 0.0, 0.0],
        )
        .unwrap_err();
    assert_eq!(
        dim,
        IndexRefusal::DimMismatch {
            expected: 3,
            actual: 4
        }
    );
    assert_eq!(dim.code(), "DESIGNATION_DIM_MISMATCH");
    // A question refuses the same way, through the index read.
    let index = CapabilityIndex::new(limits());
    let tenant = TenantId::new("tenant-a").unwrap();
    index
        .upsert(
            record("a", Some("tenant-a"), &[], "emb-v1", 3),
            vec![1.0, 0.0, 0.0],
        )
        .unwrap();
    let none = BTreeSet::new();
    let read = |version: &str, q: &[f32]| {
        index.read(&tenant, version, q, &none, &none, &BTreeMap::new(), 4, 4096)
    };
    assert!(read("emb-v1", &[1.0, 0.0, 0.0]).is_ok());
    assert!(matches!(
        read("emb-v9", &[1.0, 0.0, 0.0]),
        Err(IndexRefusal::EmbeddingVersionMismatch { .. })
    ));
    assert!(matches!(
        read("emb-v1", &[1.0, 0.0]),
        Err(IndexRefusal::DimMismatch {
            expected: 3,
            actual: 2
        })
    ));
    // A failed upsert changed nothing.
    assert_eq!(shard.snapshot().live_count(), 1);

    // A drained shard releases the pin and accepts a new version and dim.
    shard.tombstone(&OpaqueId::new("a").unwrap()).unwrap();
    assert!(shard.snapshot().pin().is_none());
    shard
        .upsert(
            record("c", None, &[], "emb-v2", 4),
            vec![0.0, 1.0, 0.0, 0.0],
        )
        .unwrap();
    assert_eq!(shard.snapshot().pin().unwrap().embedding_version, "emb-v2");
    assert_eq!(shard.snapshot().pin().unwrap().dim, 4);

    // A generation swap is atomic to readers: the held snapshot never changes.
    let held = shard.snapshot();
    shard
        .reindex(vec![(record("c", None, &[], "emb-v3", 2), vec![1.0, 1.0])])
        .unwrap();
    assert_eq!(held.pin().unwrap().embedding_version, "emb-v2");
    assert_eq!(held.live_count(), 1);
    let fresh = shard.snapshot();
    assert_eq!(fresh.pin().unwrap().embedding_version, "emb-v3");
    assert!(fresh.generation() > held.generation());
    // A re-index over mixed spaces refuses and keeps the live generation.
    let mixed = shard.reindex(vec![
        (record("p", None, &[], "emb-v4", 2), vec![1.0, 1.0]),
        (record("q", None, &[], "emb-v5", 2), vec![1.0, 1.0]),
    ]);
    assert!(mixed.is_err());
    assert_eq!(shard.snapshot().pin().unwrap().embedding_version, "emb-v3");
}
