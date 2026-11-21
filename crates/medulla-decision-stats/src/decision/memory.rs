//! Pure memory consolidation kernels (MEDULLA-DECISION-ENGINE-R651.2,
//! R651.2.1).
//!
//! Both kernels are deterministic functions of their inputs: no clock, no
//! store, no thread count. The served `MemoryRecall` handler gathers the
//! records and the neighbour lists and calls them; a plan is a proposal that
//! changes nothing.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::contract::ResourceId;
use medulla_work_types::memory::{
    ConsolidationPlan, MemoryRecord, MemoryTier, MergeGroup, Memento, PromotionGroup,
    TrajectoryEntry,
};
use sha2::{Digest, Sha256};

/// Cosine at or above which two records of one tier are near duplicates.
pub const MERGE_COSINE: KnobSpec = KnobSpec {
    name: "memory.merge_cosine",
    unit: "cosine",
    default: KnobValue::DecimalMicros(920_000),
    min: KnobValue::DecimalMicros(500_000),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// The most tokens one memento joins.
pub const MEMENTO_TOKEN_CAP: KnobSpec = KnobSpec {
    name: "memory.memento_token_cap",
    unit: "tokens",
    default: KnobValue::Integer(512),
    min: KnobValue::Integer(64),
    max: KnobValue::Integer(8192),
    safety: KnobSafety::Tunable,
};

/// The most records one `MemoryExport` returns.
pub const EXPORT_MAX_RECORDS: KnobSpec = KnobSpec {
    name: "memory.export_max_records",
    unit: "records",
    default: KnobValue::Integer(10_000),
    min: KnobValue::Integer(100),
    max: KnobValue::Integer(100_000),
    safety: KnobSafety::Tunable,
};

/// An entry below this importance is a prune candidate (R651.8).
pub const IMPORTANCE_FLOOR: KnobSpec = KnobSpec {
    name: "memory.importance_floor",
    unit: "importance",
    default: KnobValue::DecimalMicros(50_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// CKA below this similarity flags an embedding drift (R651.2.2).
pub const DRIFT_CKA_MIN: KnobSpec = KnobSpec {
    name: "memory.drift_cka_min",
    unit: "similarity",
    default: KnobValue::DecimalMicros(900_000),
    min: KnobValue::DecimalMicros(500_000),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// Every knob of the memory surface this module owns.
pub const KNOBS: &[KnobSpec] = &[
    MERGE_COSINE,
    MEMENTO_TOKEN_CAP,
    EXPORT_MAX_RECORDS,
    IMPORTANCE_FLOOR,
    DRIFT_CKA_MIN,
];

/// The token count of one record: its estimate, or one token per four bytes
/// of text when it carries none.
pub fn record_tokens(record: &MemoryRecord) -> u32 {
    if record.token_estimate > 0 {
        return record.token_estimate;
    }
    u32::try_from(record.text.len().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Union-find over record indexes; the root is always the lowest index.
fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

/// Propose merges, promotions and prune candidates for `records`.
///
/// `neighbours` maps a record id to its nearest records and their cosine; the
/// caller builds it from the embedding index, so this kernel never searches.
/// Records of one tier whose cosine is at or above `merge_cosine` form a merge
/// group; the group keeps the highest-importance record, ties to the lowest
/// `memory_id`. Episodic records with an equal `pattern_key` form a promotion
/// group with a support count. A record under legal hold is `held` and in no
/// other list. A record below `importance_floor` that no merge removes is a
/// prune candidate.
pub fn plan_consolidation(
    records: &[MemoryRecord],
    neighbours: &BTreeMap<ResourceId, Vec<(ResourceId, f64)>>,
    merge_cosine: f64,
    importance_floor: f64,
) -> ConsolidationPlan {
    let mut ordered: Vec<&MemoryRecord> = records.iter().filter(|r| !r.forgotten).collect();
    ordered.sort_by(|a, b| a.memory_id.as_str().cmp(b.memory_id.as_str()));
    ordered.dedup_by(|a, b| a.memory_id == b.memory_id);
    let index_of: BTreeMap<&str, usize> = ordered
        .iter()
        .enumerate()
        .map(|(index, record)| (record.memory_id.as_str(), index))
        .collect();

    let mut plan = ConsolidationPlan::default();
    let held: Vec<bool> = ordered.iter().map(|r| r.legal_hold).collect();
    plan.held = ordered
        .iter()
        .filter(|r| r.legal_hold)
        .map(|r| r.memory_id.clone())
        .collect();

    let mut parents: Vec<usize> = (0..ordered.len()).collect();
    for (from, near) in neighbours {
        let Some(&left) = index_of.get(from.as_str()) else {
            continue;
        };
        for (to, cosine) in near {
            let Some(&right) = index_of.get(to.as_str()) else {
                continue;
            };
            if left == right
                || held[left]
                || held[right]
                || ordered[left].tier != ordered[right].tier
                || *cosine < merge_cosine
            {
                continue;
            }
            let (a, b) = (root(&mut parents, left), root(&mut parents, right));
            let (low, high) = (a.min(b), a.max(b));
            parents[high] = low;
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for index in 0..ordered.len() {
        let group = root(&mut parents, index);
        groups.entry(group).or_default().push(index);
    }
    let mut removed: BTreeSet<usize> = BTreeSet::new();
    for members in groups.values().filter(|members| members.len() > 1) {
        let keep = members
            .iter()
            .copied()
            .max_by(|&a, &b| {
                ordered[a]
                    .importance
                    .total_cmp(&ordered[b].importance)
                    .then_with(|| ordered[b].memory_id.as_str().cmp(ordered[a].memory_id.as_str()))
            })
            .expect("a group has members");
        let merged: Vec<ResourceId> = members
            .iter()
            .copied()
            .filter(|&member| member != keep)
            .map(|member| {
                removed.insert(member);
                ordered[member].memory_id.clone()
            })
            .collect();
        plan.merges.push(MergeGroup {
            keep: ordered[keep].memory_id.clone(),
            merged,
        });
    }
    plan.merges
        .sort_by(|a, b| a.keep.as_str().cmp(b.keep.as_str()));

    let mut patterns: BTreeMap<&str, Vec<ResourceId>> = BTreeMap::new();
    for (index, record) in ordered.iter().enumerate() {
        if held[index] || record.tier != MemoryTier::Episodic {
            continue;
        }
        if let Some(key) = record.pattern_key.as_deref() {
            patterns
                .entry(key)
                .or_default()
                .push(record.memory_id.clone());
        }
    }
    plan.promotions = patterns
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(key, record_ids)| PromotionGroup {
            pattern_key: key.to_string(),
            support_count: u32::try_from(record_ids.len()).unwrap_or(u32::MAX),
            record_ids,
        })
        .collect();

    plan.prune_candidates = ordered
        .iter()
        .enumerate()
        .filter(|(index, record)| {
            !held[*index] && !removed.contains(index) && record.importance < importance_floor
        })
        .map(|(_, record)| record.memory_id.clone())
        .collect();
    plan
}

/// Join one session's records, in time order, into mementos of at most
/// `token_cap` tokens. A record larger than the cap forms a memento alone; no
/// record is split or dropped, and `source_ids` lists every record it joined.
pub fn compress_to_mementos(records: &[MemoryRecord], token_cap: u32) -> Vec<Memento> {
    let mut ordered: Vec<&MemoryRecord> = records.iter().collect();
    ordered.sort_by(|a, b| {
        a.timestamp_ms
            .cmp(&b.timestamp_ms)
            .then_with(|| a.memory_id.as_str().cmp(b.memory_id.as_str()))
    });
    let mut mementos = Vec::new();
    let mut ids: Vec<ResourceId> = Vec::new();
    let mut texts: Vec<&str> = Vec::new();
    let mut tokens: u32 = 0;
    for record in ordered {
        let cost = record_tokens(record);
        if !ids.is_empty() && tokens.saturating_add(cost) > token_cap {
            seal_memento(&mut mementos, &mut ids, &texts, tokens);
            texts.clear();
            tokens = 0;
        }
        ids.push(record.memory_id.clone());
        texts.push(&record.text);
        tokens = tokens.saturating_add(cost);
    }
    seal_memento(&mut mementos, &mut ids, &texts, tokens);
    mementos
}

/// Close the open memento, when it holds any record.
fn seal_memento(mementos: &mut Vec<Memento>, ids: &mut Vec<ResourceId>, texts: &[&str], tokens: u32) {
    if ids.is_empty() {
        return;
    }
    let digest: String = Sha256::digest(texts.join("\n").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    mementos.push(Memento {
        source_ids: std::mem::take(ids),
        token_count: tokens,
        digest,
    });
}

/// Linear CKA of two embedding snapshots of the same ids: row `i` of `first`
/// and of `second` embed the same id. The columns are centred and the result is
/// `||Yᵀ X||² / (||Xᵀ X|| ||Yᵀ Y||)` in `[0, 1]`: 1 for snapshots that differ
/// by a rotation, scale or shift, and lower as the geometry changes. `None`
/// when the snapshots differ in row count, are empty or ragged, or a snapshot
/// has no variance, so there is nothing to compare.
pub fn linear_cka(first: &[Vec<f64>], second: &[Vec<f64>]) -> Option<f64> {
    fn centred(rows: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
        let width = rows.first()?.len();
        if width == 0 || rows.iter().any(|row| row.len() != width) {
            return None;
        }
        let count = rows.len() as f64;
        let means: Vec<f64> = (0..width)
            .map(|column| rows.iter().map(|row| row[column]).sum::<f64>() / count)
            .collect();
        Some(
            rows.iter()
                .map(|row| row.iter().zip(&means).map(|(v, m)| v - m).collect())
                .collect(),
        )
    }
    fn cross_norm_squared(left: &[Vec<f64>], right: &[Vec<f64>]) -> f64 {
        let (lw, rw) = (left[0].len(), right[0].len());
        let mut total = 0.0;
        for i in 0..lw {
            for j in 0..rw {
                let entry: f64 = left
                    .iter()
                    .zip(right)
                    .map(|(l, r)| l[i] * r[j])
                    .sum();
                total += entry * entry;
            }
        }
        total
    }
    if first.len() != second.len() {
        return None;
    }
    let (x, y) = (centred(first)?, centred(second)?);
    let denominator = (cross_norm_squared(&x, &x) * cross_norm_squared(&y, &y)).sqrt();
    if denominator == 0.0 {
        return None;
    }
    Some((cross_norm_squared(&x, &y) / denominator).clamp(0.0, 1.0))
}

/// CKA on the Q16 scale (`1.0` is 65536).
pub fn cka_q16(cka: f64) -> i64 {
    (cka * 65536.0).round() as i64
}

/// Whether `cka` fell below `knob memory.drift_cka_min`.
pub fn drifted(cka: f64, cka_min: f64) -> bool {
    cka < cka_min
}

/// The prune candidates of a trajectory, in trajectory order: an entry whose
/// `importance` is below `importance_floor` and that no later entry
/// references. The plan removes nothing.
pub fn prune_trajectory(entries: &[TrajectoryEntry], importance_floor: f64) -> Vec<ResourceId> {
    entries
        .iter()
        .enumerate()
        .filter(|(position, entry)| {
            entry.importance < importance_floor
                && !entries[position + 1..]
                    .iter()
                    .any(|later| later.references.contains(&entry.entry_id))
        })
        .map(|(_, entry)| entry.entry_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::contract::TenantId;
    use medulla_work_types::memory::MemoryKind;

    fn record(id: &str, tier: MemoryTier, importance: f64, text: &str) -> MemoryRecord {
        MemoryRecord {
            memory_id: ResourceId::new(id).unwrap(),
            tenant: TenantId::new("tenant-a").unwrap(),
            kind: MemoryKind::Fact,
            tier,
            text: text.into(),
            provenance_key: None,
            session_id: None,
            timestamp_ms: 0,
            topics: Vec::new(),
            pattern_key: None,
            importance,
            access_count: 0,
            last_access_ms: 0,
            last_decay_ms: 0,
            forgotten: false,
            legal_hold: false,
            expires_at_ms: None,
            support_count: 0,
            token_estimate: 0,
            attachments: Vec::new(),
            schema_version: 1,
        }
    }

    fn near(pairs: &[(&str, &str, f64)]) -> BTreeMap<ResourceId, Vec<(ResourceId, f64)>> {
        let mut map: BTreeMap<ResourceId, Vec<(ResourceId, f64)>> = BTreeMap::new();
        for (a, b, cosine) in pairs {
            map.entry(ResourceId::new(*a).unwrap())
                .or_default()
                .push((ResourceId::new(*b).unwrap(), *cosine));
        }
        map
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.2
    #[test]
    fn near_duplicates_merge_to_the_highest_importance() {
        let records = vec![
            record("m:b", MemoryTier::Semantic, 2.0, "sky"),
            record("m:a", MemoryTier::Semantic, 2.0, "sky"),
            record("m:c", MemoryTier::Semantic, 3.0, "sky"),
            record("m:d", MemoryTier::Episodic, 9.0, "sky"),
            record("m:e", MemoryTier::Semantic, 0.01, "stale"),
        ];
        let neighbours = near(&[
            ("m:a", "m:b", 0.95),
            ("m:b", "m:c", 0.93),
            ("m:a", "m:d", 0.99),
            ("m:a", "m:e", 0.5),
        ]);
        let plan = plan_consolidation(&records, &neighbours, 0.92, 0.05);
        assert_eq!(plan.merges.len(), 1);
        assert_eq!(plan.merges[0].keep.as_str(), "m:c");
        let merged: Vec<&str> = plan.merges[0].merged.iter().map(|id| id.as_str()).collect();
        assert_eq!(merged, ["m:a", "m:b"]);
        // The cross-tier pair never merges, and the low-importance record is
        // a prune candidate, not a merge.
        let prune: Vec<&str> = plan.prune_candidates.iter().map(|id| id.as_str()).collect();
        assert_eq!(prune, ["m:e"]);
        // Ties on importance keep the lowest id.
        let tie = plan_consolidation(&records[..2], &near(&[("m:a", "m:b", 0.95)]), 0.92, 0.05);
        assert_eq!(tie.merges[0].keep.as_str(), "m:a");
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.2
    #[test]
    fn plan_is_deterministic_across_thread_counts() {
        let records: Vec<MemoryRecord> = (0..40)
            .map(|index| {
                let mut r = record(
                    &format!("m:{index:03}"),
                    MemoryTier::Episodic,
                    f64::from(index % 7),
                    "x",
                );
                r.pattern_key = Some(format!("p{}", index % 5));
                r.legal_hold = index % 11 == 0;
                r
            })
            .collect();
        let pairs: Vec<(String, String, f64)> = (0..39)
            .map(|index| (format!("m:{index:03}"), format!("m:{:03}", index + 1), 0.95))
            .collect();
        let borrowed: Vec<(&str, &str, f64)> = pairs
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), *c))
            .collect();
        let neighbours = near(&borrowed);
        let expected = plan_consolidation(&records, &neighbours, 0.92, 0.05);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (records, neighbours) = (records.clone(), neighbours.clone());
                std::thread::spawn(move || plan_consolidation(&records, &neighbours, 0.92, 0.05))
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), expected);
        }
        assert!(!expected.held.is_empty());
        assert!(expected.promotions.iter().all(|group| group.support_count > 1));
        let mut reversed = records.clone();
        reversed.reverse();
        assert_eq!(plan_consolidation(&reversed, &neighbours, 0.92, 0.05), expected);
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.2.1
    #[test]
    fn mementos_respect_the_token_cap_and_keep_sources() {
        let records: Vec<MemoryRecord> = (0..6)
            .map(|index| {
                let mut r = record(&format!("m:{index}"), MemoryTier::Working, 1.0, "abcdefgh");
                r.timestamp_ms = 100 - i64::from(index);
                r.token_estimate = 30;
                r
            })
            .collect();
        let mementos = compress_to_mementos(&records, 64);
        assert_eq!(mementos.len(), 3);
        assert!(mementos.iter().all(|m| m.token_count <= 64));
        let sources: Vec<&str> = mementos
            .iter()
            .flat_map(|m| m.source_ids.iter().map(|id| id.as_str()))
            .collect();
        assert_eq!(sources, ["m:5", "m:4", "m:3", "m:2", "m:1", "m:0"]);
        assert!(mementos.iter().all(|m| m.digest.len() == 64));
        assert_eq!(compress_to_mementos(&records, 64), mementos);
        let mut huge = record("m:huge", MemoryTier::Working, 1.0, "x");
        huge.token_estimate = 500;
        assert_eq!(compress_to_mementos(&[huge], 64).len(), 1);
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.2.2
    #[test]
    fn cka_is_one_for_identical_snapshots_and_drops_for_shuffled() {
        let rows: Vec<Vec<f64>> = (0..6u32)
            .map(|i| vec![f64::from(i), f64::from((i * i) % 7), f64::from((3 * i) % 5)])
            .collect();
        let same = linear_cka(&rows, &rows).unwrap();
        assert!((same - 1.0).abs() < 1e-12, "{same}");
        let scaled: Vec<Vec<f64>> = rows
            .iter()
            .map(|row| row.iter().map(|v| 2.0 * v + 1.0).collect())
            .collect();
        assert!((linear_cka(&rows, &scaled).unwrap() - 1.0).abs() < 1e-12);
        let mut shuffled = rows.clone();
        shuffled.rotate_left(1);
        let low = linear_cka(&rows, &shuffled).unwrap();
        assert!(low < 0.5, "{low}");
        assert!(drifted(low, 0.9));
        assert!(!drifted(same, 0.9));
        assert_eq!(cka_q16(1.0), 65536);
        assert_eq!(linear_cka(&rows, &rows[1..]), None);
        assert_eq!(linear_cka(&[], &[]), None);
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.2.3
    #[test]
    fn referenced_entries_are_never_trajectory_candidates() {
        let id = |value: &str| ResourceId::new(value).unwrap();
        let entry = |name: &str, importance: f64, references: &[&str]| TrajectoryEntry {
            entry_id: id(name),
            importance,
            references: references.iter().map(|r| id(r)).collect(),
        };
        let entries = vec![
            entry("e1", 0.01, &[]),
            entry("e2", 0.9, &["e1"]),
            entry("e3", 0.02, &[]),
            entry("e4", 0.5, &[]),
            entry("e5", 0.03, &["e4"]),
        ];
        // e1 is referenced by the later e2; e4 is not low; e3 and e5 are
        // unreferenced and below the floor, listed in trajectory order.
        assert_eq!(
            prune_trajectory(&entries, 0.05),
            vec![id("e3"), id("e5")]
        );
        // An earlier entry's reference does not protect a later one.
        let early = vec![entry("a", 0.9, &["b"]), entry("b", 0.01, &[])];
        assert_eq!(prune_trajectory(&early, 0.05), vec![id("b")]);
    }
}
