//! Verifier-free pairwise candidate ranking with bounded revision
//! (MEDULLA-DECISION-ENGINE-R140.103).
//!
//! The caller supplies a judged record for every pair of candidates. The
//! engine scores each candidate by wins minus losses (a tie scores zero),
//! orders by descending score then ascending id, keeps `pairwise.top_k` and
//! hands the rest back for at most `pairwise.max_revision_rounds` revision
//! rounds. A preference cycle resolves by score alone. This ranking is
//! separate from the answer-ranking kernel of R134.105.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_query_types::solve_scalar::digest_text;

use super::trajectory_refusal::{request_premise, TrajectoryRefusal};
use crate::decision::codes::PARAMETER_INVALID;
use crate::decision::knob_families::trajectory as knobs;
use crate::decision::policy::DecisionPolicy;
use crate::decision::record::outcome::RefusalCode;
use crate::decision::{PremiseClass, PremiseRef};

/// The request does not pin the `pairwise_copeland` policy at revision 1.
pub const PAIRWISE_POLICY_UNPINNED: RefusalCode = RefusalCode::new("PAIRWISE_POLICY_UNPINNED");
/// A pair of candidates has no judged record.
pub const PAIRWISE_PAIR_MISSING: RefusalCode = RefusalCode::new("PAIRWISE_PAIR_MISSING");
/// One pair carries two different verdicts.
pub const PAIRWISE_PAIR_CONFLICT: RefusalCode = RefusalCode::new("PAIRWISE_PAIR_CONFLICT");
/// A record's candidate digest differs from the candidate's own digest.
pub const PAIRWISE_DIGEST_MISMATCH: RefusalCode = RefusalCode::new("PAIRWISE_DIGEST_MISMATCH");

/// The refusal codes of the pairwise ranking.
pub const REFUSALS: &[RefusalCode] = &[
    PAIRWISE_POLICY_UNPINNED,
    PAIRWISE_PAIR_MISSING,
    PAIRWISE_PAIR_CONFLICT,
    PAIRWISE_DIGEST_MISMATCH,
];

/// The one policy id this ranking runs.
pub const PAIRWISE_POLICY_ID: &str = "pairwise_copeland";
/// The revision of [`PAIRWISE_POLICY_ID`].
pub const PAIRWISE_POLICY_REVISION: u32 = 1;
/// Most candidates one ranking may hold (the pair table grows with the square).
pub const MAX_PAIRWISE_CANDIDATES: usize = 64;

const PAIRWISE_DIGEST_DOMAIN: &str = "eg/pairwise-ranking/v1";

/// One candidate and the digest of the content the judge saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairwiseCandidate {
    pub id: String,
    pub digest: String,
}

/// A judge's verdict between candidate A and candidate B of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Verdict {
    WinA,
    WinB,
    Tie,
}

/// One judged pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairwiseRecord {
    pub candidate_a: String,
    pub digest_a: String,
    pub candidate_b: String,
    pub digest_b: String,
    /// The judge or model revision that decided the pair.
    pub judge_revision: String,
    pub verdict: Verdict,
    pub evidence_digest: String,
}

/// The pinned policy a request names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairwisePolicyPin {
    pub id: String,
    pub revision: u32,
}

/// The ranking question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairwiseRequest {
    pub candidates: Vec<PairwiseCandidate>,
    pub records: Vec<PairwiseRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PairwisePolicyPin>,
}

/// One candidate's place in the ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RankedCandidate {
    pub id: String,
    pub score: i64,
}

/// The ranking: the order, who is kept and who may be revised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairwiseRanking {
    pub policy_id: String,
    pub policy_revision: u32,
    pub ranked: Vec<RankedCandidate>,
    pub kept: Vec<String>,
    /// The candidates not kept, in rank order.
    pub to_revise: Vec<String>,
    pub max_revision_rounds: u32,
    pub decision_digest: String,
}

/// The knobs the ranking reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairwiseKnobs {
    pub top_k: usize,
    pub max_revision_rounds: u32,
}

impl PairwiseKnobs {
    /// The knobs a pinned policy sets, each held inside its declared range.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        Self {
            top_k: knobs::read(policy, &knobs::PAIRWISE_TOP_K) as usize,
            max_revision_rounds: knobs::read(policy, &knobs::PAIRWISE_MAX_REVISION_ROUNDS) as u32,
        }
    }
}

impl Default for PairwiseKnobs {
    fn default() -> Self {
        Self {
            top_k: 3,
            max_revision_rounds: 2,
        }
    }
}

fn refuse(code: RefusalCode, detail: impl Into<String>) -> TrajectoryRefusal {
    TrajectoryRefusal::new(code, detail)
}

/// Rank the candidates by a full round-robin of judged pairs.
pub fn rank_pairwise(
    request: &PairwiseRequest,
    knobs: &PairwiseKnobs,
) -> Result<(PairwiseRanking, Vec<PremiseRef>), TrajectoryRefusal> {
    let pinned = request
        .policy
        .as_ref()
        .is_some_and(|p| p.id == PAIRWISE_POLICY_ID && p.revision == PAIRWISE_POLICY_REVISION);
    if !pinned {
        return Err(refuse(
            PAIRWISE_POLICY_UNPINNED,
            "a pairwise ranking pins pairwise_copeland at revision 1",
        ));
    }
    let mut digests: BTreeMap<&str, &str> = BTreeMap::new();
    for candidate in &request.candidates {
        if candidate.id.is_empty()
            || digests
                .insert(candidate.id.as_str(), candidate.digest.as_str())
                .is_some()
        {
            return Err(refuse(
                PARAMETER_INVALID,
                "candidate ids are non-empty and unique",
            ));
        }
    }
    if digests.len() < 2 || digests.len() > MAX_PAIRWISE_CANDIDATES {
        return Err(refuse(
            PARAMETER_INVALID,
            "a pairwise ranking holds 2 to 64 candidates",
        ));
    }
    // Each pair keyed by its ids in sorted order, holding the record and the
    // verdict seen from the lower id.
    let mut pairs: BTreeMap<(&str, &str), (&PairwiseRecord, Verdict)> = BTreeMap::new();
    for record in &request.records {
        let (Some(digest_a), Some(digest_b)) = (
            digests.get(record.candidate_a.as_str()),
            digests.get(record.candidate_b.as_str()),
        ) else {
            return Err(refuse(
                PARAMETER_INVALID,
                format!(
                    "record {} vs {} names a candidate outside the request",
                    record.candidate_a, record.candidate_b
                ),
            ));
        };
        if record.candidate_a == record.candidate_b {
            return Err(refuse(
                PARAMETER_INVALID,
                "a candidate is not judged against itself",
            ));
        }
        if *digest_a != record.digest_a || *digest_b != record.digest_b {
            return Err(refuse(
                PAIRWISE_DIGEST_MISMATCH,
                format!(
                    "record {} vs {} judged other content than the candidates carry",
                    record.candidate_a, record.candidate_b
                ),
            ));
        }
        let swapped = record.candidate_a > record.candidate_b;
        let key = if swapped {
            (record.candidate_b.as_str(), record.candidate_a.as_str())
        } else {
            (record.candidate_a.as_str(), record.candidate_b.as_str())
        };
        let from_low = match (record.verdict, swapped) {
            (Verdict::Tie, _) => Verdict::Tie,
            (verdict, false) => verdict,
            (Verdict::WinA, true) => Verdict::WinB,
            (Verdict::WinB, true) => Verdict::WinA,
        };
        if let Some((first, first_verdict)) = pairs.get(&key) {
            if *first_verdict != from_low {
                return Err(refuse(
                    PAIRWISE_PAIR_CONFLICT,
                    format!("pair {} vs {} carries two verdicts", key.0, key.1),
                ));
            }
            if *first != record {
                return Err(refuse(
                    PAIRWISE_PAIR_CONFLICT,
                    format!(
                        "pair {} vs {} is judged twice with different records",
                        key.0, key.1
                    ),
                ));
            }
            continue;
        }
        pairs.insert(key, (record, from_low));
    }
    let ids: Vec<&str> = digests.keys().copied().collect();
    let mut scores: BTreeMap<&str, i64> = ids.iter().map(|id| (*id, 0)).collect();
    for (i, low) in ids.iter().enumerate() {
        for high in &ids[i + 1..] {
            let Some((_, verdict)) = pairs.get(&(*low, *high)) else {
                return Err(refuse(
                    PAIRWISE_PAIR_MISSING,
                    format!("pair {low} vs {high} has no judged record"),
                ));
            };
            let (low_delta, high_delta) = match verdict {
                Verdict::WinA => (1, -1),
                Verdict::WinB => (-1, 1),
                Verdict::Tie => (0, 0),
            };
            *scores.get_mut(low).expect("scored id") += low_delta;
            *scores.get_mut(high).expect("scored id") += high_delta;
        }
    }
    let mut ranked: Vec<RankedCandidate> = scores
        .iter()
        .map(|(id, score)| RankedCandidate {
            id: (*id).to_string(),
            score: *score,
        })
        .collect();
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    let keep = knobs.top_k.clamp(1, ranked.len());
    let kept: Vec<String> = ranked[..keep].iter().map(|r| r.id.clone()).collect();
    let to_revise: Vec<String> = ranked[keep..].iter().map(|r| r.id.clone()).collect();
    let mut ranking = PairwiseRanking {
        policy_id: PAIRWISE_POLICY_ID.to_string(),
        policy_revision: PAIRWISE_POLICY_REVISION,
        ranked,
        kept,
        to_revise,
        max_revision_rounds: knobs.max_revision_rounds,
        decision_digest: String::new(),
    };
    let mut evidence: Vec<(&str, &str, &str, &str, &str)> = pairs
        .iter()
        .map(|((low, high), (record, _))| {
            (
                *low,
                *high,
                record.judge_revision.as_str(),
                record.evidence_digest.as_str(),
                match record.verdict {
                    Verdict::WinA => "win_a",
                    Verdict::WinB => "win_b",
                    Verdict::Tie => "tie",
                },
            )
        })
        .collect();
    evidence.sort_unstable();
    ranking.decision_digest = digest_text(
        PAIRWISE_DIGEST_DOMAIN,
        &(
            &ranking.policy_id,
            ranking.policy_revision,
            &ranking.ranked,
            ranking.max_revision_rounds,
            keep,
            &digests,
            &evidence,
        ),
    );
    let mut premises: Vec<PremiseRef> = pairs
        .iter()
        .map(|((low, high), (record, _))| {
            request_premise(
                &format!("pairwise:record:{low}:{high}"),
                format!(
                    "pairwise:record:{low}:{high};judge={};evidence={};verdict={:?}",
                    record.judge_revision, record.evidence_digest, record.verdict
                ),
                PremiseClass::Claim,
                "params.pairwise.records",
            )
        })
        .collect();
    premises.extend(ranking.ranked.iter().map(|r| {
        request_premise(
            &format!("pairwise:score:{}", r.id),
            format!("pairwise:score:{}={}", r.id, r.score),
            PremiseClass::Claim,
            "params.pairwise.records",
        )
    }));
    Ok((ranking, premises))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str) -> PairwiseCandidate {
        PairwiseCandidate {
            id: id.into(),
            digest: format!("sha256:{id}"),
        }
    }

    fn record(a: &str, b: &str, verdict: Verdict) -> PairwiseRecord {
        PairwiseRecord {
            candidate_a: a.into(),
            digest_a: format!("sha256:{a}"),
            candidate_b: b.into(),
            digest_b: format!("sha256:{b}"),
            judge_revision: "judge-7".into(),
            verdict,
            evidence_digest: format!("sha256:ev-{a}-{b}"),
        }
    }

    fn request(ids: &[&str], records: Vec<PairwiseRecord>) -> PairwiseRequest {
        PairwiseRequest {
            candidates: ids.iter().map(|id| candidate(id)).collect(),
            records,
            policy: Some(PairwisePolicyPin {
                id: PAIRWISE_POLICY_ID.into(),
                revision: PAIRWISE_POLICY_REVISION,
            }),
        }
    }

    fn scores(ranking: &PairwiseRanking) -> Vec<(&str, i64)> {
        ranking
            .ranked
            .iter()
            .map(|r| (r.id.as_str(), r.score))
            .collect()
    }

    fn code(request: &PairwiseRequest) -> RefusalCode {
        rank_pairwise(request, &PairwiseKnobs::default())
            .unwrap_err()
            .code
    }

    // spec: MEDULLA-DECISION-ENGINE-R140.103
    #[test]
    fn copeland_cycle_and_tie_fixture() {
        let knobs = PairwiseKnobs::default();
        // A preference cycle a > b > c > a: every candidate scores zero, so the
        // id tie-break alone orders them.
        let cycle = request(
            &["c", "a", "b"],
            vec![
                record("a", "b", Verdict::WinA),
                record("b", "c", Verdict::WinA),
                record("c", "a", Verdict::WinA),
            ],
        );
        let (ranking, premises) = rank_pairwise(&cycle, &knobs).unwrap();
        assert_eq!(scores(&ranking), [("a", 0), ("b", 0), ("c", 0)]);
        assert_eq!(ranking.kept, ["a", "b", "c"]);
        assert!(ranking.to_revise.is_empty());
        assert_eq!(
            premises
                .iter()
                .filter(|p| p.subject.starts_with("pairwise:record:"))
                .count(),
            3
        );
        assert_eq!(
            premises
                .iter()
                .filter(|p| p.subject.starts_with("pairwise:score:"))
                .count(),
            3
        );
        // A tie scores zero for both; wins and losses are signed; ties on score
        // fall to the lower id. d beats everyone, a ties b, a and b each beat c.
        let fixture = request(
            &["a", "b", "c", "d"],
            vec![
                record("a", "b", Verdict::Tie),
                record("c", "a", Verdict::WinB),
                record("b", "c", Verdict::WinA),
                record("d", "a", Verdict::WinA),
                record("b", "d", Verdict::WinB),
                record("c", "d", Verdict::WinB),
            ],
        );
        let (ranking, _) = rank_pairwise(&fixture, &knobs).unwrap();
        assert_eq!(scores(&ranking), [("d", 3), ("a", 0), ("b", 0), ("c", -3)]);
        assert_eq!(ranking.kept, ["d", "a", "b"]);
        assert_eq!(ranking.to_revise, ["c"]);
        assert_eq!(ranking.max_revision_rounds, 2);
        // top_k never exceeds the candidate count; a smaller top_k revises the rest.
        let all = PairwiseKnobs { top_k: 9, ..knobs };
        assert_eq!(rank_pairwise(&fixture, &all).unwrap().0.kept.len(), 4);
        let one = PairwiseKnobs {
            top_k: 1,
            max_revision_rounds: 0,
        };
        let (single, _) = rank_pairwise(&fixture, &one).unwrap();
        assert_eq!(
            (
                single.kept.as_slice(),
                single.to_revise.len(),
                single.max_revision_rounds
            ),
            (["d".to_string()].as_slice(), 3, 0)
        );
        // The order of candidates and records changes nothing; the digest binds the verdicts.
        let mut shuffled = fixture.clone();
        shuffled.candidates.reverse();
        shuffled.records.reverse();
        assert_eq!(rank_pairwise(&shuffled, &knobs).unwrap().0, ranking);
        let mut flipped = fixture.clone();
        flipped.records[0].verdict = Verdict::WinA;
        assert_ne!(
            rank_pairwise(&flipped, &knobs).unwrap().0.decision_digest,
            ranking.decision_digest
        );
        // Refusals.
        let mut unpinned = fixture.clone();
        unpinned.policy = None;
        assert_eq!(code(&unpinned), PAIRWISE_POLICY_UNPINNED);
        unpinned.policy = Some(PairwisePolicyPin {
            id: PAIRWISE_POLICY_ID.into(),
            revision: 2,
        });
        assert_eq!(code(&unpinned), PAIRWISE_POLICY_UNPINNED);
        let mut missing = fixture.clone();
        missing.records.pop();
        assert_eq!(code(&missing), PAIRWISE_PAIR_MISSING);
        let mut conflict = fixture.clone();
        conflict.records.push(record("b", "a", Verdict::WinA));
        assert_eq!(code(&conflict), PAIRWISE_PAIR_CONFLICT);
        let mut mismatch = fixture.clone();
        mismatch.records[0].digest_b = "sha256:other".into();
        assert_eq!(code(&mismatch), PAIRWISE_DIGEST_MISMATCH);
        let mut outsider = fixture.clone();
        outsider.records.push(record("a", "z", Verdict::Tie));
        assert_eq!(code(&outsider), PARAMETER_INVALID);
        let lone = request(&["a"], vec![]);
        assert_eq!(code(&lone), PARAMETER_INVALID);
    }
}
