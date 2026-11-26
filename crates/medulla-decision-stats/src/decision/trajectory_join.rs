//! Trajectory join and distillation (MEDULLA-DECISION-ENGINE-R753.751).
//!
//! [`join`] keeps the entry with the strongest evidence (`Proof`, then
//! `Observation`, then `Claim`); ties break on the higher verifier score, then
//! on the lexicographically smallest trajectory id. The distillation names the
//! winner, every source id sorted, and the derivation digest: the SHA-256 of
//! the sorted source ids joined by newlines. A pure function of the entries:
//! the order they arrive in never matters.

use medulla_decision_types::decision::digest::trajectory_derivation_digest;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::trajectory_verdict::{DistilledTrajectory, TrajectoryEntry};

/// The most trajectories one join takes; more refuse with
/// `CANDIDATE_SET_TOO_LARGE`.
pub const MAX_JOIN_TRAJECTORIES: usize = 64;

/// Join competing trajectories into the strongest one.
pub fn join(entries: &[TrajectoryEntry]) -> Result<DistilledTrajectory, String> {
    let mut source_ids: Vec<String> = entries.iter().map(|e| e.trajectory_id.clone()).collect();
    source_ids.sort();
    if source_ids.is_empty() {
        return Err(
            StatisticalErrorCode::JoinEmptySet.refusal("a join needs at least one trajectory")
        );
    }
    if source_ids.len() > MAX_JOIN_TRAJECTORIES {
        return Err(StatisticalErrorCode::CandidateSetTooLarge.refusal(format!(
            "a join takes at most {MAX_JOIN_TRAJECTORIES} trajectories, got {}",
            source_ids.len()
        )));
    }
    if source_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(StatisticalErrorCode::ParameterInvalid
            .refusal("a trajectory id appears twice in the join"));
    }
    // `EvidenceClass` orders `Proof < Observation < Claim`, so the strongest
    // evidence is the smallest; the score is compared descending.
    let winner = entries
        .iter()
        .min_by(|a, b| {
            a.evidence
                .cmp(&b.evidence)
                .then_with(|| b.verifier_score.value.cmp(&a.verifier_score.value))
                .then_with(|| a.trajectory_id.cmp(&b.trajectory_id))
        })
        .ok_or_else(|| {
            StatisticalErrorCode::JoinEmptySet.refusal("a join needs at least one trajectory")
        })?;
    let derivation_digest = trajectory_derivation_digest(&source_ids);
    DistilledTrajectory::new(
        format!("distilled:{derivation_digest}"),
        winner.trajectory_id.clone(),
        source_ids,
        derivation_digest,
    )
    .map_err(|detail| StatisticalErrorCode::ParameterInvalid.refusal(detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
    use medulla_types::evidence_class::EvidenceClass;

    fn entry(id: &str, evidence: EvidenceClass, score: i64) -> TrajectoryEntry {
        TrajectoryEntry::new(
            id.to_string(),
            evidence,
            QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: score,
            },
            "ab".repeat(32),
        )
        .unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R753.751
    #[test]
    fn join_is_deterministic_under_ties() {
        // Proof beats a higher-scored observation and claim.
        let winner = |entries: &[TrajectoryEntry]| join(entries).unwrap().winner_id;
        assert_eq!(
            winner(&[
                entry("a", EvidenceClass::Claim, 900),
                entry("b", EvidenceClass::Observation, 800),
                entry("c", EvidenceClass::Proof, 1),
            ]),
            "c"
        );
        // Equal evidence: the higher verifier score wins.
        assert_eq!(
            winner(&[
                entry("a", EvidenceClass::Observation, 5),
                entry("b", EvidenceClass::Observation, 9),
            ]),
            "b"
        );
        // Equal evidence and score: the smallest id wins.
        assert_eq!(
            winner(&[
                entry("zeta", EvidenceClass::Claim, 7),
                entry("alpha", EvidenceClass::Claim, 7),
                entry("mid", EvidenceClass::Claim, 7),
            ]),
            "alpha"
        );
        // The distillation lists every source sorted and a hex derivation
        // digest of the sorted ids joined by newlines.
        let distilled = join(&[
            entry("b", EvidenceClass::Claim, 1),
            entry("a", EvidenceClass::Claim, 1),
        ])
        .unwrap();
        assert_eq!(distilled.source_ids.as_slice(), ["a", "b"]);
        assert_eq!(
            distilled.derivation_digest,
            trajectory_derivation_digest(&["a".to_string(), "b".to_string()])
        );
        assert_eq!(distilled.derivation_digest.len(), 64);
        assert_eq!(
            distilled.trajectory_id,
            format!("distilled:{}", distilled.derivation_digest)
        );
        // An empty join and a repeated id are refused.
        assert!(join(&[]).unwrap_err().starts_with("JOIN_EMPTY_SET"));
        assert!(join(&[
            entry("a", EvidenceClass::Claim, 1),
            entry("a", EvidenceClass::Proof, 2)
        ])
        .is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R753.751
    #[test]
    fn join_ignores_input_order() {
        let entries = vec![
            entry("t3", EvidenceClass::Observation, 40),
            entry("t1", EvidenceClass::Observation, 40),
            entry("t4", EvidenceClass::Claim, 99),
            entry("t2", EvidenceClass::Proof, 10),
            entry("t5", EvidenceClass::Proof, 10),
        ];
        let expected = join(&entries).unwrap();
        assert_eq!(expected.winner_id, "t2");
        // Every rotation and the reversal give byte-identical distillations.
        let mut shuffled = entries.clone();
        for _ in 0..entries.len() {
            shuffled.rotate_left(1);
            assert_eq!(join(&shuffled).unwrap(), expected);
            let mut reversed = shuffled.clone();
            reversed.reverse();
            assert_eq!(
                serde_json::to_vec(&join(&reversed).unwrap()).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
    }
}
