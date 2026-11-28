//! Near-duplicate skill detection (MEDULLA-DECISION-ENGINE-R703.2).
//!
//! Skills are blocked by the side-effect class of their capability
//! descriptor, so a pair is compared only inside a block; a block larger than
//! `skill.max_block` splits by sorted skill id. A pair is a merge candidate
//! when the Jaccard overlap of the declared capability ids and the cosine of
//! the stored embeddings both reach their floors. The work is bounded by the
//! block size squared, never the library size squared. Arithmetic is integer
//! only, and the result is a proposal: nothing is written.

use std::collections::{BTreeMap, BTreeSet};

use medulla_agent_types::agent_component::capability_descriptor::SideEffectClass;
use medulla_decision_policy_types::decision::skill_evolution::SkillMergeCandidate;
use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::knob_families::skill::integer,
    medulla_decision_statistical_types::decision::knob_families::skill::MAX_BLOCK,
    medulla_decision_statistical_types::decision::knob_families::skill::MERGE_COSINE_MIN,
    medulla_decision_statistical_types::decision::knob_families::skill::MERGE_JACCARD_MIN,
};

const PPM: u128 = 1_000_000;

/// One library skill as the detector sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    /// The skill's component id.
    pub skill_id: String,
    /// The side-effect class of its capability descriptor; the blocking key.
    pub side_effect_class: SideEffectClass,
    /// The capability ids the skill declares.
    pub capability_ids: BTreeSet<String>,
    /// The stored embedding, one integer per dimension.
    pub embedding: Vec<i32>,
}

/// The floors and block size, read from the pinned decision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeParams {
    pub max_block: usize,
    pub jaccard_min_ppm: u32,
    pub cosine_min_ppm: u32,
}

impl MergeParams {
    /// The knob values the policy pins (defaults where it pins none).
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let clamp = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
        Self {
            max_block: usize::try_from(integer(policy, &MAX_BLOCK)).unwrap_or(usize::MAX),
            jaccard_min_ppm: clamp(integer(policy, &MERGE_JACCARD_MIN)),
            cosine_min_ppm: clamp(integer(policy, &MERGE_COSINE_MIN)),
        }
    }
}

/// The candidates found and the number of pairs compared to find them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReport {
    pub candidates: Vec<SkillMergeCandidate>,
    pub comparisons: u64,
}

fn class_rank(class: SideEffectClass) -> u8 {
    match class {
        SideEffectClass::ReadOnly => 0,
        SideEffectClass::GraphWrite => 1,
        SideEffectClass::External => 2,
        SideEffectClass::Irreversible => 3,
    }
}

/// Jaccard overlap of two id sets in ppm; two empty sets overlap in nothing.
fn jaccard_ppm(a: &BTreeSet<String>, b: &BTreeSet<String>) -> u32 {
    let shared = a.intersection(b).count() as u128;
    let union = a.len() as u128 + b.len() as u128 - shared;
    if union == 0 {
        return 0;
    }
    u32::try_from(shared * PPM / union).unwrap_or(u32::MAX)
}

/// Cosine similarity of two embeddings in ppm, floored at zero. Vectors of
/// different length or with no magnitude are not similar.
fn cosine_ppm(a: &[i32], b: &[i32]) -> u32 {
    if a.len() != b.len() || a.is_empty() {
        return 0;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0i128, 0u128, 0u128);
    for (x, y) in a.iter().zip(b) {
        dot += i128::from(*x) * i128::from(*y);
        norm_a += (i128::from(*x) * i128::from(*x)) as u128;
        norm_b += (i128::from(*y) * i128::from(*y)) as u128;
    }
    let denominator = (norm_a * norm_b).isqrt();
    if dot <= 0 || denominator == 0 {
        return 0;
    }
    let ppm = (dot as u128).saturating_mul(PPM) / denominator;
    u32::try_from(ppm.min(PPM)).unwrap_or(u32::MAX)
}

/// Propose merges among `skills`. Candidates sort by overlap, then cosine,
/// both descending, then by the pair's ids.
pub fn merge_candidates(skills: &[SkillEntry], params: &MergeParams) -> MergeReport {
    let mut blocks: BTreeMap<u8, Vec<&SkillEntry>> = BTreeMap::new();
    for skill in skills {
        blocks
            .entry(class_rank(skill.side_effect_class))
            .or_default()
            .push(skill);
    }
    let mut candidates = Vec::new();
    let mut comparisons = 0u64;
    for (_, mut block) in blocks {
        block.sort_by(|a, b| a.skill_id.cmp(&b.skill_id));
        for chunk in block.chunks(params.max_block.max(2)) {
            for (index, a) in chunk.iter().enumerate() {
                for b in &chunk[index + 1..] {
                    comparisons += 1;
                    let overlap = jaccard_ppm(&a.capability_ids, &b.capability_ids);
                    if overlap < params.jaccard_min_ppm {
                        continue;
                    }
                    let cosine = cosine_ppm(&a.embedding, &b.embedding);
                    if cosine < params.cosine_min_ppm {
                        continue;
                    }
                    let Ok(evidence) =
                        BoundedVec::new(vec![a.skill_id.clone(), b.skill_id.clone()])
                    else {
                        continue;
                    };
                    candidates.push(SkillMergeCandidate {
                        a: a.skill_id.clone(),
                        b: b.skill_id.clone(),
                        overlap_ppm: overlap,
                        cosine_ppm: cosine,
                        evidence,
                    });
                }
            }
        }
    }
    candidates.sort_by(|x, y| {
        y.overlap_ppm
            .cmp(&x.overlap_ppm)
            .then(y.cosine_ppm.cmp(&x.cosine_ppm))
            .then(x.a.cmp(&y.a))
            .then(x.b.cmp(&y.b))
    });
    MergeReport {
        candidates,
        comparisons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: usize, class: SideEffectClass, caps: &[&str], embedding: &[i32]) -> SkillEntry {
        SkillEntry {
            skill_id: format!("skill-{id:06}"),
            side_effect_class: class,
            capability_ids: caps.iter().map(|c| (*c).to_owned()).collect(),
            embedding: embedding.to_vec(),
        }
    }

    fn params(max_block: usize) -> MergeParams {
        MergeParams {
            max_block,
            jaccard_min_ppm: 600_000,
            cosine_min_ppm: 900_000,
        }
    }

    /// `count` skills of one class whose capability sets never overlap.
    fn library(count: usize, class: SideEffectClass) -> Vec<SkillEntry> {
        (0..count)
            .map(|i| {
                let cap = format!("cap-{i}");
                skill(i, class, &[cap.as_str()], &[1, 0])
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R703.2
    #[test]
    fn blocking_bounds_pair_comparisons() {
        // A pair qualifies only when both floors are met, and only inside a block.
        let near = [
            skill(1, SideEffectClass::ReadOnly, &["a", "b", "c"], &[10, 0]),
            skill(
                2,
                SideEffectClass::ReadOnly,
                &["a", "b", "c", "d"],
                &[10, 1],
            ),
            // Same capabilities and embedding, but another side-effect class.
            skill(3, SideEffectClass::External, &["a", "b", "c"], &[10, 0]),
            // Overlap passes, cosine does not.
            skill(4, SideEffectClass::ReadOnly, &["a", "b", "c"], &[0, 10]),
            // Cosine passes, overlap does not.
            skill(5, SideEffectClass::ReadOnly, &["x", "y"], &[10, 0]),
        ];
        let report = merge_candidates(&near, &params(256));
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].a, "skill-000001");
        assert_eq!(report.candidates[0].b, "skill-000002");
        assert_eq!(report.candidates[0].overlap_ppm, 750_000);
        assert!(report.candidates[0].cosine_ppm >= 900_000);
        // Four read-only skills compare in 6 pairs; the lone external one in none.
        assert_eq!(report.comparisons, 6);

        // Ten skills: one block, every pair compared once.
        let ten = library(10, SideEffectClass::ReadOnly);
        assert_eq!(merge_candidates(&ten, &params(256)).comparisons, 45);

        // A thousand skills under the default block of 256: four chunks.
        let thousand = library(1000, SideEffectClass::ReadOnly);
        let report = merge_candidates(&thousand, &params(256));
        assert_eq!(report.comparisons, 3 * (256 * 255 / 2) + (232 * 231 / 2));
        assert!(report.comparisons < 1000 * 999 / 2);

        // A hundred thousand skills under the smallest block: the work grows
        // with the block size, far below the library size squared.
        let large = library(100_000, SideEffectClass::ReadOnly);
        let report = merge_candidates(&large, &params(8));
        assert_eq!(report.comparisons, 12_500 * (8 * 7 / 2));
        assert!(report.comparisons < 100_000u64 * 99_999 / 2 / 1000);
        assert!(report.candidates.is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R703.2
    #[test]
    fn params_read_pinned_policy_knobs() {
        use medulla_decision_statistical_types::decision::knobs::KnobValue;
        let mut policy = DecisionPolicy::engine_default();
        assert_eq!(
            MergeParams::from_policy(&policy),
            MergeParams {
                max_block: 256,
                jaccard_min_ppm: 600_000,
                cosine_min_ppm: 900_000
            }
        );
        policy
            .knobs
            .insert(MERGE_JACCARD_MIN.knob_name(), KnobValue::Integer(800_000));
        assert_eq!(MergeParams::from_policy(&policy).jaccard_min_ppm, 800_000);
    }
}
