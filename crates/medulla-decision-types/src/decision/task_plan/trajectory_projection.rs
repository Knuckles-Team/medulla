//! Provenance-preserving trajectory projection and reinsertion
//! (MEDULLA-DECISION-ENGINE-R140.101).
//!
//! A projection keeps some token ranges of source trajectories and removes
//! others. The engine validates every range against the source's length and
//! against the stored outcome record, shuffles the source order with a seeded
//! Fisher-Yates over ChaCha8, and names the new candidate from its parents so
//! the projected trace always cites exactly the trajectories it came from.
//! Source trajectories are never written.

use std::collections::{BTreeMap, BTreeSet};

use rand_chacha::rand_core::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medulla_query_types::solve_scalar::digest_text;

use super::trajectory_refusal::{request_premise, TrajectoryRefusal};
use crate::decision::record::outcome::RefusalCode;
use crate::decision::{PremiseClass, PremiseRef};

/// A removed or retained range is empty, reversed or beyond the trace.
pub const PROJECTION_RANGE_INVALID: RefusalCode = RefusalCode::new("PROJECTION_RANGE_INVALID");
/// Two ranges of one source overlap.
pub const PROJECTION_RANGE_OVERLAP: RefusalCode = RefusalCode::new("PROJECTION_RANGE_OVERLAP");
/// A source has no outcome record visible to the caller.
pub const PROJECTION_SOURCE_UNKNOWN: RefusalCode = RefusalCode::new("PROJECTION_SOURCE_UNKNOWN");
/// A source revision or digest differs from the stored outcome record.
pub const PROJECTION_SOURCE_STALE: RefusalCode = RefusalCode::new("PROJECTION_SOURCE_STALE");
/// The redaction policy or the tokenizer is not pinned.
pub const PROJECTION_POLICY_UNPINNED: RefusalCode = RefusalCode::new("PROJECTION_POLICY_UNPINNED");
/// The parent set is empty or names a source twice.
pub const PROJECTION_PARENT_INCOMPLETE: RefusalCode =
    RefusalCode::new("PROJECTION_PARENT_INCOMPLETE");

/// The refusal codes of the projection decision.
pub const REFUSALS: &[RefusalCode] = &[
    PROJECTION_RANGE_INVALID,
    PROJECTION_RANGE_OVERLAP,
    PROJECTION_SOURCE_UNKNOWN,
    PROJECTION_SOURCE_STALE,
    PROJECTION_POLICY_UNPINNED,
    PROJECTION_PARENT_INCOMPLETE,
];

/// Most source trajectories one projection may merge.
pub const MAX_PROJECTION_SOURCES: usize = 64;

const PROJECTION_DIGEST_DOMAIN: &str = "eg/trajectory-projection/v1";

/// A half-open token range `[start, end)` with `start` below `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TokenRange {
    pub start: u64,
    pub end: u64,
}

/// A pinned policy or tokenizer: id and revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionPin {
    pub id: String,
    pub revision: u32,
}

/// One source trajectory and what the projection removes and keeps of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceTrajectory {
    pub run_id: String,
    pub candidate_id: String,
    pub source_revision: u64,
    pub trace_digest: String,
    pub trace_token_len: u64,
    #[serde(default)]
    pub removed: Vec<TokenRange>,
    #[serde(default)]
    pub retained: Vec<TokenRange>,
}

/// The projection question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionRequest {
    pub sources: Vec<SourceTrajectory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction_policy: Option<ProjectionPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokenizer: Option<ProjectionPin>,
    pub shuffle_seed: u64,
}

/// The stored outcome record of one source, read by the caller through the
/// tenant-visible log reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSource {
    pub source_revision: u64,
    pub trace_digest: String,
}

/// One parent of the new candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionParent {
    pub run_id: String,
    pub candidate_id: String,
    pub source_revision: u64,
    pub trace_digest: String,
}

/// The validated projection: the shuffled source order, the parents and the
/// id of the new candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionPlan {
    /// Source run ids after the seeded shuffle.
    pub order: Vec<String>,
    /// Parents sorted by run id.
    pub parents: Vec<ProjectionParent>,
    pub decision_digest: String,
    /// SHA-256 over the sorted parent digests and the decision digest: 64
    /// lowercase hex characters.
    pub new_candidate_id: String,
}

fn refuse(code: RefusalCode, detail: impl Into<String>) -> TrajectoryRefusal {
    TrajectoryRefusal::new(code, detail)
}

fn pinned(pin: &Option<ProjectionPin>) -> bool {
    pin.as_ref().is_some_and(|p| !p.id.is_empty())
}

/// A uniform draw in `[0, bound)` by rejection, so no residue is favoured.
fn below(rng: &mut ChaCha8Rng, bound: u64) -> u64 {
    let zone = u64::MAX - (u64::MAX % bound);
    loop {
        let draw = rng.next_u64();
        if draw < zone {
            return draw % bound;
        }
    }
}

/// Fisher-Yates over `ids` with ChaCha8 seeded by SHA-256 of the seed bytes.
fn shuffle(ids: &mut [String], seed: u64) {
    let key: [u8; 32] = Sha256::digest(seed.to_le_bytes()).into();
    let mut rng = ChaCha8Rng::from_seed(key);
    for i in (1..ids.len()).rev() {
        let j = below(&mut rng, i as u64 + 1) as usize;
        ids.swap(i, j);
    }
}

fn check_ranges(source: &SourceTrajectory) -> Result<(), TrajectoryRefusal> {
    let mut all: Vec<TokenRange> = Vec::new();
    for range in source.removed.iter().chain(source.retained.iter()) {
        if range.start >= range.end || range.end > source.trace_token_len {
            return Err(refuse(
                PROJECTION_RANGE_INVALID,
                format!(
                    "source {} range [{}, {}) is not inside its {} tokens",
                    source.run_id, range.start, range.end, source.trace_token_len
                ),
            ));
        }
        all.push(*range);
    }
    all.sort_unstable();
    if all.windows(2).any(|pair| pair[1].start < pair[0].end) {
        return Err(refuse(
            PROJECTION_RANGE_OVERLAP,
            format!("source {} has overlapping ranges", source.run_id),
        ));
    }
    Ok(())
}

/// Validate a projection against the stored outcome records and derive its
/// plan and premises. Writes nothing.
pub fn validate_projection(
    request: &ProjectionRequest,
    stored: &BTreeMap<String, StoredSource>,
) -> Result<(ProjectionPlan, Vec<PremiseRef>), TrajectoryRefusal> {
    if !pinned(&request.redaction_policy) || !pinned(&request.tokenizer) {
        return Err(refuse(
            PROJECTION_POLICY_UNPINNED,
            "a projection pins its redaction policy and its tokenizer",
        ));
    }
    let mut seen_runs = BTreeSet::new();
    let mut seen_candidates = BTreeSet::new();
    if request.sources.is_empty()
        || request.sources.len() > MAX_PROJECTION_SOURCES
        || request.sources.iter().any(|s| {
            s.run_id.is_empty()
                || s.candidate_id.is_empty()
                || !seen_runs.insert(s.run_id.as_str())
                || !seen_candidates.insert(s.candidate_id.as_str())
        })
    {
        return Err(refuse(
            PROJECTION_PARENT_INCOMPLETE,
            "a projection names 1 to 64 distinct source runs and candidates",
        ));
    }
    for source in &request.sources {
        check_ranges(source)?;
        let Some(record) = stored.get(&source.run_id) else {
            return Err(refuse(
                PROJECTION_SOURCE_UNKNOWN,
                format!("source {} has no visible outcome record", source.run_id),
            ));
        };
        if record.source_revision != source.source_revision
            || record.trace_digest != source.trace_digest
        {
            return Err(refuse(
                PROJECTION_SOURCE_STALE,
                format!(
                    "source {} is at revision {} with digest {}",
                    source.run_id, record.source_revision, record.trace_digest
                ),
            ));
        }
    }
    let mut ordered: Vec<&SourceTrajectory> = request.sources.iter().collect();
    ordered.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    let mut order: Vec<String> = ordered.iter().map(|s| s.run_id.clone()).collect();
    shuffle(&mut order, request.shuffle_seed);
    let parents: Vec<ProjectionParent> = ordered
        .iter()
        .map(|s| ProjectionParent {
            run_id: s.run_id.clone(),
            candidate_id: s.candidate_id.clone(),
            source_revision: s.source_revision,
            trace_digest: s.trace_digest.clone(),
        })
        .collect();
    let bodies: Vec<(&str, &[TokenRange], &[TokenRange])> = ordered
        .iter()
        .map(|s| {
            (
                s.run_id.as_str(),
                s.removed.as_slice(),
                s.retained.as_slice(),
            )
        })
        .collect();
    let decision_digest = digest_text(
        PROJECTION_DIGEST_DOMAIN,
        &(
            &parents,
            bodies,
            &request.redaction_policy,
            &request.tokenizer,
            request.shuffle_seed,
            &order,
        ),
    );
    let mut parent_digests: Vec<&str> = parents.iter().map(|p| p.trace_digest.as_str()).collect();
    parent_digests.sort_unstable();
    let mut hasher = Sha256::new();
    for digest in &parent_digests {
        hasher.update(digest.as_bytes());
        hasher.update([0u8]);
    }
    hasher.update(decision_digest.as_bytes());
    let new_candidate_id = hex::encode(hasher.finalize());
    let premises = parents
        .iter()
        .map(|p| {
            request_premise(
                &format!("projection:source:{}", p.run_id),
                format!(
                    "projection:source:{};revision={};digest={}",
                    p.run_id, p.source_revision, p.trace_digest
                ),
                PremiseClass::Claim,
                "params.projection.sources",
            )
        })
        .collect();
    Ok((
        ProjectionPlan {
            order,
            parents,
            decision_digest,
            new_candidate_id,
        },
        premises,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u64, end: u64) -> TokenRange {
        TokenRange { start, end }
    }

    fn source(run: &str, removed: Vec<TokenRange>, retained: Vec<TokenRange>) -> SourceTrajectory {
        SourceTrajectory {
            run_id: run.into(),
            candidate_id: format!("cand-{run}"),
            source_revision: 4,
            trace_digest: format!("sha256:trace-{run}"),
            trace_token_len: 100,
            removed,
            retained,
        }
    }

    fn request(seed: u64) -> ProjectionRequest {
        let pin = |id: &str| {
            Some(ProjectionPin {
                id: id.into(),
                revision: 1,
            })
        };
        ProjectionRequest {
            sources: ["a", "b", "c", "d", "e"]
                .iter()
                .map(|run| source(run, vec![range(0, 10)], vec![range(10, 100)]))
                .collect(),
            redaction_policy: pin("redact"),
            tokenizer: pin("tok"),
            shuffle_seed: seed,
        }
    }

    fn store(request: &ProjectionRequest) -> BTreeMap<String, StoredSource> {
        request
            .sources
            .iter()
            .map(|s| {
                (
                    s.run_id.clone(),
                    StoredSource {
                        source_revision: s.source_revision,
                        trace_digest: s.trace_digest.clone(),
                    },
                )
            })
            .collect()
    }

    fn code(request: &ProjectionRequest, stored: &BTreeMap<String, StoredSource>) -> RefusalCode {
        validate_projection(request, stored).unwrap_err().code
    }

    // spec: MEDULLA-DECISION-ENGINE-R140.101
    #[test]
    fn order_and_candidate_id_are_deterministic() {
        let req = request(7);
        let stored = store(&req);
        let (first, premises) = validate_projection(&req, &stored).unwrap();
        let (again, _) = validate_projection(&req, &stored).unwrap();
        assert_eq!(first, again);
        let mut sorted = first.order.clone();
        sorted.sort();
        assert_eq!(sorted, ["a", "b", "c", "d", "e"]);
        assert_eq!(first.new_candidate_id.len(), 64);
        assert!(first
            .new_candidate_id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(premises.len(), 5);
        assert_eq!(premises[0].subject, "projection:source:a");
        // Some seed gives another order and another candidate id.
        let orders: BTreeSet<Vec<String>> = (0..16)
            .map(|seed| {
                validate_projection(&request(seed), &stored)
                    .unwrap()
                    .0
                    .order
            })
            .collect();
        assert!(orders.len() > 1);
        assert_ne!(
            validate_projection(&request(8), &stored)
                .unwrap()
                .0
                .new_candidate_id,
            first.new_candidate_id
        );
        // The input order of the sources changes neither the order nor the id.
        let mut reversed = req.clone();
        reversed.sources.reverse();
        assert_eq!(validate_projection(&reversed, &stored).unwrap().0, first);
        // The shuffle is a permutation for every length, including the one-source case.
        let mut one = req.clone();
        one.sources.truncate(1);
        assert_eq!(validate_projection(&one, &stored).unwrap().0.order, ["a"]);
        // The shuffle alone, on the sorted ids, gives the plan's order for the same seed.
        let mut pinned_order: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        shuffle(&mut pinned_order, 7);
        assert_eq!(pinned_order, first.order);
        // Refusals.
        let mut bad = req.clone();
        bad.sources[0].retained = vec![range(10, 101)];
        assert_eq!(code(&bad, &stored), PROJECTION_RANGE_INVALID);
        bad.sources[0].retained = vec![range(10, 10)];
        assert_eq!(code(&bad, &stored), PROJECTION_RANGE_INVALID);
        bad.sources[0].retained = vec![range(5, 20)];
        assert_eq!(code(&bad, &stored), PROJECTION_RANGE_OVERLAP);
        let mut missing = stored.clone();
        missing.remove("c");
        assert_eq!(code(&req, &missing), PROJECTION_SOURCE_UNKNOWN);
        let mut stale = stored.clone();
        stale.get_mut("b").unwrap().source_revision = 5;
        assert_eq!(code(&req, &stale), PROJECTION_SOURCE_STALE);
        let mut changed = stored.clone();
        changed.get_mut("b").unwrap().trace_digest = "sha256:other".into();
        assert_eq!(code(&req, &changed), PROJECTION_SOURCE_STALE);
        let mut unpinned = req.clone();
        unpinned.tokenizer = None;
        assert_eq!(code(&unpinned, &stored), PROJECTION_POLICY_UNPINNED);
        unpinned.tokenizer = Some(ProjectionPin {
            id: String::new(),
            revision: 1,
        });
        assert_eq!(code(&unpinned, &stored), PROJECTION_POLICY_UNPINNED);
        let mut none = req.clone();
        none.sources.clear();
        assert_eq!(code(&none, &stored), PROJECTION_PARENT_INCOMPLETE);
        let mut twice = req.clone();
        twice.sources.push(twice.sources[0].clone());
        assert_eq!(code(&twice, &stored), PROJECTION_PARENT_INCOMPLETE);
    }
}
