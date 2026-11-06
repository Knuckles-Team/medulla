//! Belief revision kernels (MEDULLA-DECISION-ENGINE-R700.2, R700.5).
//!
//! Pure functions: no store access, no writes. The `BeliefRevise` handler is the
//! single writer of the records these kernels return.

use std::collections::{BTreeMap, BTreeSet};

use medulla_types::belief_revision::{
    BeliefRevision, EvidenceRef, FrictionFinding, RetractionProposal,
};
use medulla_types::claims::{Evidence, EvidenceChannel};
use medulla_types::evidence_class::EvidenceClass;

use crate::antonyms::confirms_opposition;
use crate::claims::{combine_evidence, ClaimKnobs};

const PPM: u64 = 1_000_000;

/// The belief knobs the kernels read (loaded by the server knob table; the
/// kernels never read the environment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeliefKnobs {
    /// Channel priors and the claim weight bounds shared with R652.2.
    pub claims: ClaimKnobs,
    /// `belief.weight_cap_micro`, default 6000000, `[1000000, 12000000]`.
    pub weight_cap_micro: u32,
    /// `belief.opposition_similarity_min`, default 800000, `[500000, 1000000]` ppm.
    pub opposition_similarity_min: u32,
    /// `belief.retraction_threshold_micro`, default -1386294, `[-6000000, 0]`.
    pub retraction_threshold_micro: i64,
}

/// One piece of evidence as the pass receives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceInput {
    pub evidence_id: String,
    pub channel: EvidenceChannel,
    pub weight_ppm: u32,
    pub class: EvidenceClass,
}

/// One claim whose evidence changed since the last pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimInput {
    pub text: String,
    pub prior_micro: i64,
    pub support: Vec<EvidenceInput>,
    pub oppose: Vec<EvidenceInput>,
}

/// Lowercase tokens of a text; a contraction `n't` reads as the word `not`.
pub fn tokens(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .replace("n't", " not")
        .replace('\'', "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Token Jaccard similarity in ppm; two empty sets are not similar.
pub fn jaccard_ppm(left: &BTreeSet<String>, right: &BTreeSet<String>) -> u32 {
    let union = left.union(right).count() as u64;
    if union == 0 {
        return 0;
    }
    (left.intersection(right).count() as u64 * PPM / union) as u32
}

/// Inverted index from lowercase token to the claims holding it.
#[derive(Clone, Debug, Default)]
pub struct TokenIndex {
    by_token: BTreeMap<String, BTreeSet<String>>,
    by_claim: BTreeMap<String, BTreeSet<String>>,
}

impl TokenIndex {
    /// Index (or re-index) one claim's text.
    pub fn insert(&mut self, claim_id: &str, text: &str) {
        self.remove(claim_id);
        let words = tokens(text);
        for word in &words {
            self.by_token
                .entry(word.clone())
                .or_default()
                .insert(claim_id.to_string());
        }
        self.by_claim.insert(claim_id.to_string(), words);
    }

    /// Drop one claim from the index.
    pub fn remove(&mut self, claim_id: &str) {
        if let Some(words) = self.by_claim.remove(claim_id) {
            for word in words {
                if let Some(ids) = self.by_token.get_mut(&word) {
                    ids.remove(claim_id);
                }
            }
        }
    }

    fn tokens_of(&self, claim_id: &str) -> Option<&BTreeSet<String>> {
        self.by_claim.get(claim_id)
    }

    /// Claims sharing at least one token with `words`.
    fn candidates(&self, words: &BTreeSet<String>) -> BTreeSet<&String> {
        words
            .iter()
            .filter_map(|word| self.by_token.get(word))
            .flatten()
            .collect()
    }
}

fn effective(item: &EvidenceInput, supports: bool, knobs: &BeliefKnobs) -> i64 {
    let mut claims = knobs.claims;
    claims.weight_cap_ppm = knobs.weight_cap_micro;
    let evidence = Evidence {
        tenant_id: String::new(),
        evidence_id: item.evidence_id.clone(),
        claim_id: String::new(),
        channel: item.channel,
        supports,
        weight_ppm: item.weight_ppm,
        source_digest: String::new(),
    };
    combine_evidence([&evidence], &claims).abs()
}

fn refs(items: &[EvidenceInput], supports: bool, knobs: &BeliefKnobs) -> Vec<EvidenceRef> {
    items
        .iter()
        .map(|item| EvidenceRef {
            evidence_id: item.evidence_id.clone(),
            channel: item.channel,
            weight_ppm: effective(item, supports, knobs).clamp(0, i64::from(u32::MAX)) as u32,
            class: item.class,
        })
        .collect()
}

/// Revise one claim: `posterior = prior + combine(support) - combine(oppose)`,
/// each weight capped at `belief.weight_cap_micro`.
pub fn revise_claim(
    claim_id: &str,
    pass_id: &str,
    input: &ClaimInput,
    knobs: &BeliefKnobs,
) -> BeliefRevision {
    let support = refs(&input.support, true, knobs);
    let oppose = refs(&input.oppose, false, knobs);
    let mut revision = BeliefRevision {
        claim_id: claim_id.to_string(),
        prior_micro: input.prior_micro,
        posterior_micro: 0,
        support,
        oppose,
        reason: String::new(),
        weakest_class: EvidenceClass::Proof,
        pass_id: pass_id.to_string(),
    };
    revision.posterior_micro = replay_posterior(&revision);
    let sum = |items: &[EvidenceRef]| items.iter().map(|r| i64::from(r.weight_ppm)).sum::<i64>();
    revision.reason = match sum(&revision.support).cmp(&sum(&revision.oppose)) {
        std::cmp::Ordering::Greater => "support_exceeds_opposition",
        std::cmp::Ordering::Less => "opposition_exceeds_support",
        std::cmp::Ordering::Equal => "balanced",
    }
    .to_string();
    revision.weakest_class = revision
        .support
        .iter()
        .chain(&revision.oppose)
        .map(|r| r.class)
        .max()
        .unwrap_or(EvidenceClass::Claim);
    revision
}

/// Recompute `posterior_micro` from the recorded inputs alone
/// (MEDULLA-DECISION-ENGINE-R700.5).
pub fn replay_posterior(revision: &BeliefRevision) -> i64 {
    let sum = |items: &[EvidenceRef]| {
        items
            .iter()
            .fold(0i64, |acc, r| acc.saturating_add(i64::from(r.weight_ppm)))
    };
    revision
        .prior_micro
        .saturating_add(sum(&revision.support))
        .saturating_sub(sum(&revision.oppose))
}

/// What one pass produced and what it cost.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassOutput {
    /// Revisions keyed by claim id, so the order is deterministic.
    pub revisions: BTreeMap<String, BeliefRevision>,
    pub friction: Vec<FrictionFinding>,
    pub retractions: Vec<RetractionProposal>,
    /// Claims visited by the pass.
    pub claims_visited: usize,
    /// Evidence edges read by the pass.
    pub evidence_edges: usize,
}

/// Run one pass over the claims whose evidence changed. `index` holds the text of
/// every claim a changed claim may oppose. Time is linear in the changed claims
/// plus their evidence edges (and the index postings of their tokens).
pub fn run_pass(
    pass_id: &str,
    changed: &BTreeMap<String, ClaimInput>,
    index: &TokenIndex,
    knobs: &BeliefKnobs,
) -> PassOutput {
    let mut out = PassOutput::default();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for (claim_id, input) in changed {
        out.claims_visited += 1;
        out.evidence_edges += input.support.len() + input.oppose.len();
        let revision = revise_claim(claim_id, pass_id, input, knobs);
        if revision.posterior_micro < knobs.retraction_threshold_micro {
            out.retractions.push(RetractionProposal {
                claim_id: claim_id.clone(),
                posterior_micro: revision.posterior_micro,
                evidence_class: EvidenceClass::Claim,
            });
        }
        out.revisions.insert(claim_id.clone(), revision);

        let words = tokens(&input.text);
        for other in index.candidates(&words) {
            if other == claim_id {
                continue;
            }
            let Some(other_words) = index.tokens_of(other) else {
                continue;
            };
            let similarity = jaccard_ppm(&words, other_words);
            if similarity < knobs.opposition_similarity_min
                || !confirms_opposition(&words, other_words)
            {
                continue;
            }
            let pair = if claim_id < other {
                (claim_id.clone(), other.clone())
            } else {
                (other.clone(), claim_id.clone())
            };
            if seen.insert(pair.clone()) {
                out.friction.push(FrictionFinding {
                    left: pair.0,
                    right: pair.1,
                    similarity_ppm: similarity,
                    opposes: true,
                });
            }
        }
    }
    out.friction
        .sort_by(|a, b| (&a.left, &a.right).cmp(&(&b.left, &b.right)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knobs() -> BeliefKnobs {
        BeliefKnobs {
            claims: ClaimKnobs {
                weight_cap_ppm: 6_000_000,
                prior_outcome: 1_000_000,
                prior_optimization: 800_000,
                prior_health: 600_000,
                prior_process: 500_000,
                prior_mined: 300_000,
                support_min_micro: 0,
                accept_min_micro: 0,
                contest_max_micro: 0,
                feed_relevance_min: 0,
            },
            weight_cap_micro: 6_000_000,
            opposition_similarity_min: 700_000,
            retraction_threshold_micro: -1_386_294,
        }
    }

    fn ev(id: &str, weight: u32, class: EvidenceClass) -> EvidenceInput {
        EvidenceInput {
            evidence_id: id.into(),
            channel: EvidenceChannel::Outcome,
            weight_ppm: weight,
            class,
        }
    }

    fn claim(
        text: &str,
        prior: i64,
        support: Vec<EvidenceInput>,
        oppose: Vec<EvidenceInput>,
    ) -> ClaimInput {
        ClaimInput {
            text: text.into(),
            prior_micro: prior,
            support,
            oppose,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R700.2
    #[test]
    fn pass_cost_is_linear_in_changed_claims() {
        let mut index = TokenIndex::default();
        for n in 0..500 {
            index.insert(&format!("bulk{n}"), &format!("unrelated topic number{n}"));
        }
        let mut changed = BTreeMap::new();
        for n in 0..10 {
            changed.insert(
                format!("c{n}"),
                claim(
                    "latency is lower",
                    0,
                    vec![ev("s", 2_000_000, EvidenceClass::Proof)],
                    vec![ev("o", 500_000, EvidenceClass::Claim)],
                ),
            );
        }
        let out = run_pass("p", &changed, &index, &knobs());
        assert_eq!(out.claims_visited, 10, "only changed claims are visited");
        assert_eq!(out.evidence_edges, 20);
        assert_eq!(out.revisions["c0"].posterior_micro, 1_500_000);
        assert_eq!(out.revisions["c0"].weakest_class, EvidenceClass::Claim);
        let capped = revise_claim(
            "c",
            "p",
            &claim(
                "x",
                0,
                vec![ev("big", 99_000_000, EvidenceClass::Proof)],
                vec![],
            ),
            &knobs(),
        );
        assert_eq!(capped.posterior_micro, 6_000_000, "the weight cap applies");
    }

    // spec: MEDULLA-DECISION-ENGINE-R700.2
    #[test]
    fn opposed_pair_needs_similarity_and_a_confirming_negation() {
        let mut index = TokenIndex::default();
        index.insert("a", "the cache hit rate is higher");
        index.insert("b", "the cache hit rate is lower");
        index.insert("c", "deploys run on tuesday");
        let mut changed = BTreeMap::new();
        changed.insert(
            "a".to_string(),
            claim("the cache hit rate is higher", 0, vec![], vec![]),
        );
        let out = run_pass("p", &changed, &index, &knobs());
        assert_eq!(out.friction.len(), 1);
        assert_eq!(
            (
                out.friction[0].left.as_str(),
                out.friction[0].right.as_str()
            ),
            ("a", "b")
        );
        assert!(out.friction[0].opposes);
    }

    // spec: MEDULLA-DECISION-ENGINE-R700.5
    #[test]
    fn replay_matches_the_recorded_posterior() {
        let revision = revise_claim(
            "c",
            "p",
            &claim(
                "x",
                100,
                vec![ev("s", 1_000_000, EvidenceClass::Observation)],
                vec![ev("o", 3_000_000, EvidenceClass::Claim)],
            ),
            &knobs(),
        );
        assert_eq!(revision.posterior_micro, 100 + 1_000_000 - 3_000_000);
        assert_eq!(replay_posterior(&revision), revision.posterior_micro);
    }
}
