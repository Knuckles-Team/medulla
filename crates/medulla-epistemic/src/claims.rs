//! Claim lifecycle and evidence kernels (MEDULLA-DECISION-ENGINE-R652.2, .7, .8).
//!
//! Pure functions only: no store, no clock, no float. The `Claims` handler in
//! `medulla-h-decide` is the single writer of claim state and calls these to
//! decide a transition, combine evidence into signed log-odds, arbitrate
//! perspectives, grade a feed item and seal a research artifact.

use std::collections::{BTreeMap, BTreeSet};

use medulla_query_types::solve_scalar::{digest_text, DIGEST_TEXT_PREFIX};
use medulla_types::claims::{
    ArtifactSeal, ClaimState, Evidence, EvidenceChannel, ResearchArtifact, Stance,
};
use medulla_types::contract::ServerErrorCode;

/// Digest domain of a sealed research artifact.
pub const ARTIFACT_DIGEST_DOMAIN: &str = "research_artifact";

/// Ppm denominator.
const PPM: i64 = 1_000_000;

/// The legal lifecycle edges, `LEGAL_TRANSITIONS[from][to]`, indexed by
/// [`ClaimState::index`] in the order proposed, supported, contested,
/// accepted, retired. `retired` is final: its row is all false.
pub const LEGAL_TRANSITIONS: [[bool; 5]; 5] = [
    // proposed -> supported, contested, retired
    [false, true, true, false, true],
    // supported -> accepted, contested, retired
    [false, false, true, true, true],
    // contested -> supported, retired
    [false, true, false, false, true],
    // accepted -> contested, retired
    [false, false, true, false, true],
    // retired -> (final)
    [false, false, false, false, false],
];

/// The claim knobs the kernels read (`claims.*`, loaded by the server knob
/// table; the kernels never read the environment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimKnobs {
    /// `claims.weight_cap_ppm`.
    pub weight_cap_ppm: u32,
    /// `claims.prior_outcome`.
    pub prior_outcome: u32,
    /// `claims.prior_optimization`.
    pub prior_optimization: u32,
    /// `claims.prior_health`.
    pub prior_health: u32,
    /// `claims.prior_process`.
    pub prior_process: u32,
    /// `claims.prior_mined`.
    pub prior_mined: u32,
    /// `claims.support_min_micro`.
    pub support_min_micro: i64,
    /// `claims.accept_min_micro`.
    pub accept_min_micro: i64,
    /// `claims.contest_max_micro`.
    pub contest_max_micro: i64,
    /// `claims.feed_relevance_min`.
    pub feed_relevance_min: u32,
}

impl ClaimKnobs {
    /// The prior of one evidence channel, in ppm.
    pub fn prior(&self, channel: EvidenceChannel) -> u32 {
        match channel {
            EvidenceChannel::Outcome => self.prior_outcome,
            EvidenceChannel::Optimization => self.prior_optimization,
            EvidenceChannel::Health => self.prior_health,
            EvidenceChannel::Process => self.prior_process,
            EvidenceChannel::Mined => self.prior_mined,
        }
    }
}

/// Whether `from -> to` is an edge of the lifecycle table. O(1).
pub fn legal_transition(from: ClaimState, to: ClaimState) -> bool {
    LEGAL_TRANSITIONS[from.index()][to.index()]
}

/// The signed contribution of one piece of evidence, in micro-nats:
/// `sign * min(weight_ppm, cap) * prior / 1000000`.
fn contribution(evidence: &Evidence, knobs: &ClaimKnobs) -> i64 {
    let weight = i64::from(evidence.weight_ppm.min(knobs.weight_cap_ppm));
    let magnitude = weight * i64::from(knobs.prior(evidence.channel)) / PPM;
    if evidence.supports {
        magnitude
    } else {
        -magnitude
    }
}

/// Sum the signed contributions of `evidence` in micro-nats.
pub fn combine_evidence<'a>(
    evidence: impl IntoIterator<Item = &'a Evidence>,
    knobs: &ClaimKnobs,
) -> i64 {
    evidence.into_iter().fold(0i64, |sum, item| {
        sum.saturating_add(contribution(item, knobs))
    })
}

/// Whether the log-odds guard of the edge into `to` holds. Entering
/// `supported` needs `support_min`, `accepted` needs `accept_min`,
/// `contested` needs the log-odds at or below `contest_max`; `retired` and
/// `proposed` carry no guard.
pub fn guard_met(to: ClaimState, log_odds_micro: i64, knobs: &ClaimKnobs) -> bool {
    match to {
        ClaimState::Supported => log_odds_micro >= knobs.support_min_micro,
        ClaimState::Accepted => log_odds_micro >= knobs.accept_min_micro,
        ClaimState::Contested => log_odds_micro <= knobs.contest_max_micro,
        ClaimState::Proposed | ClaimState::Retired => true,
    }
}

/// Decide one edge: the lifecycle table first, then the log-odds guard.
pub fn check_transition(
    from: ClaimState,
    to: ClaimState,
    log_odds_micro: i64,
    knobs: &ClaimKnobs,
) -> Result<(), ServerErrorCode> {
    if !legal_transition(from, to) {
        return Err(ServerErrorCode::ClaimIllegalTransition);
    }
    if !guard_met(to, log_odds_micro, knobs) {
        return Err(ServerErrorCode::ClaimGuardNotMet);
    }
    Ok(())
}

/// Pairs of perspectives with opposite stances on one claim, each pair as
/// `(supporter, opposer)` ordered by name, the list sorted.
pub fn contradiction_pairs(stances: &BTreeMap<String, Stance>) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (supporter, _) in stances.iter().filter(|(_, s)| **s == Stance::Supports) {
        for (opposer, _) in stances.iter().filter(|(_, s)| **s == Stance::Opposes) {
            pairs.push((supporter.clone(), opposer.clone()));
        }
    }
    pairs.sort();
    pairs
}

fn token_set(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .map(|token| token.to_lowercase())
        .collect()
}

/// Token-set Jaccard similarity, in ppm, between a feed item and the fleet
/// topic set. Tokens are lower-cased whitespace splits; an empty union grades
/// 0.
pub fn grade_feed(feed_item: &str, fleet_topics: &[String]) -> u32 {
    let item = token_set(feed_item);
    let topics: BTreeSet<String> = fleet_topics
        .iter()
        .flat_map(|topic| token_set(topic))
        .collect();
    let union = item.union(&topics).count() as u64;
    if union == 0 {
        return 0;
    }
    let shared = item.intersection(&topics).count() as u64;
    (shared * PPM as u64 / union) as u32
}

/// Whether a feed grade clears `claims.feed_relevance_min`.
pub fn feed_is_relevant(grade_ppm: u32, knobs: &ClaimKnobs) -> bool {
    grade_ppm >= knobs.feed_relevance_min
}

/// The seal of an artifact: SHA-256 over its canonical JSON with the seal
/// removed, as 64 lower-case hex characters.
fn seal_digest(artifact: &ResearchArtifact) -> String {
    let mut subject = artifact.clone();
    subject.seal = None;
    let text = digest_text(ARTIFACT_DIGEST_DOMAIN, &subject);
    text.strip_prefix(DIGEST_TEXT_PREFIX)
        .unwrap_or(&text)
        .to_string()
}

/// Build the sealed artifact of a cohort. The claim ids are a set, so they are
/// sorted and de-duplicated before sealing.
pub fn compile_artifact(
    tenant_id: &str,
    artifact_id: &str,
    cohort_id: &str,
    claim_ids: &[String],
    code_specs: &[String],
    exploration_edges: &[(String, String)],
) -> ResearchArtifact {
    let claim_ids: BTreeSet<String> = claim_ids.iter().cloned().collect();
    let mut artifact = ResearchArtifact {
        tenant_id: tenant_id.to_string(),
        artifact_id: artifact_id.to_string(),
        cohort_id: cohort_id.to_string(),
        claim_ids: claim_ids.into_iter().collect(),
        code_specs: code_specs.to_vec(),
        exploration_edges: exploration_edges.to_vec(),
        seal: None,
    };
    artifact.seal = Some(ArtifactSeal {
        digest: seal_digest(&artifact),
    });
    artifact
}

/// Check an artifact's seal: present, 64 lower-case hex characters and equal
/// to the digest of the artifact without its seal.
pub fn verify_seal(artifact: &ResearchArtifact) -> Result<(), ServerErrorCode> {
    let Some(seal) = &artifact.seal else {
        return Err(ServerErrorCode::ArtifactSealViolation);
    };
    let well_formed = seal.digest.len() == 64
        && seal
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !well_formed || seal.digest != seal_digest(artifact) {
        return Err(ServerErrorCode::ArtifactSealViolation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knobs() -> ClaimKnobs {
        ClaimKnobs {
            weight_cap_ppm: 6_000_000,
            prior_outcome: 1_000_000,
            prior_optimization: 800_000,
            prior_health: 600_000,
            prior_process: 500_000,
            prior_mined: 300_000,
            support_min_micro: 1_000_000,
            accept_min_micro: 2_944_439,
            contest_max_micro: -1_000_000,
            feed_relevance_min: 300_000,
        }
    }

    fn evidence(id: &str, channel: EvidenceChannel, supports: bool, weight: u32) -> Evidence {
        Evidence {
            tenant_id: "t1".to_string(),
            evidence_id: id.to_string(),
            claim_id: "c1".to_string(),
            channel,
            supports,
            weight_ppm: weight,
            source_digest: format!("sha256:{id}"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.2
    #[test]
    fn lifecycle_table_and_log_odds_are_deterministic() {
        use ClaimState::*;
        let legal = [
            (Proposed, Supported),
            (Proposed, Contested),
            (Proposed, Retired),
            (Supported, Accepted),
            (Supported, Contested),
            (Supported, Retired),
            (Contested, Supported),
            (Contested, Retired),
            (Accepted, Contested),
            (Accepted, Retired),
        ];
        for from in ClaimState::ALL {
            for to in ClaimState::ALL {
                assert_eq!(
                    legal_transition(from, to),
                    legal.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }

        let k = knobs();
        // 1.0 * 1_000_000 + 0.8 * 500_000 - 0.6 * 250_000 micro-nats.
        let items = vec![
            evidence("e1", EvidenceChannel::Outcome, true, 1_000_000),
            evidence("e2", EvidenceChannel::Optimization, true, 500_000),
            evidence("e3", EvidenceChannel::Health, false, 250_000),
            evidence("e4", EvidenceChannel::Process, true, 0),
            evidence("e5", EvidenceChannel::Mined, true, 1_000_000),
        ];
        // e4 carries zero weight, so it adds nothing.
        let expected = 1_000_000 + 400_000 - 150_000 + 300_000;
        assert_eq!(combine_evidence(&items, &k), expected);

        // The weight cap binds a weight above it.
        let mut capped = k;
        capped.weight_cap_ppm = 100_000;
        assert_eq!(
            combine_evidence(&items[..1], &capped),
            100_000,
            "min(weight, cap) * prior / 1e6"
        );

        // Guards at their boundaries.
        assert!(guard_met(Supported, 1_000_000, &k));
        assert!(!guard_met(Supported, 999_999, &k));
        assert!(guard_met(Accepted, 2_944_439, &k));
        assert!(!guard_met(Accepted, 2_944_438, &k));
        assert!(guard_met(Contested, -1_000_000, &k));
        assert!(!guard_met(Contested, -999_999, &k));
        assert!(guard_met(Retired, i64::MIN, &k));
        assert_eq!(
            check_transition(Proposed, Accepted, i64::MAX, &k),
            Err(ServerErrorCode::ClaimIllegalTransition)
        );
        assert_eq!(
            check_transition(Proposed, Supported, 0, &k),
            Err(ServerErrorCode::ClaimGuardNotMet)
        );
        assert_eq!(check_transition(Proposed, Supported, 1_000_000, &k), Ok(()));
        assert_eq!(
            check_transition(Retired, Retired, 0, &k),
            Err(ServerErrorCode::ClaimIllegalTransition)
        );

        // Equal results across 1 and 8 threads.
        let many: Vec<Evidence> = (0..400)
            .map(|i| {
                evidence(
                    &format!("m{i}"),
                    EvidenceChannel::Outcome,
                    i % 3 != 0,
                    (i as u32 * 2_503) % 1_000_001,
                )
            })
            .collect();
        let single = combine_evidence(&many, &k);
        let results: Vec<i64> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| combine_evidence(&many, &k)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(results.iter().all(|r| *r == single));
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.7
    #[test]
    fn contradiction_pairs_and_feed_grades_are_deterministic() {
        let mut stances = BTreeMap::new();
        stances.insert("zeta".to_string(), Stance::Opposes);
        stances.insert("alpha".to_string(), Stance::Supports);
        stances.insert("mid".to_string(), Stance::Supports);
        stances.insert("beta".to_string(), Stance::Opposes);
        let pairs = contradiction_pairs(&stances);
        assert_eq!(
            pairs,
            vec![
                ("alpha".to_string(), "beta".to_string()),
                ("alpha".to_string(), "zeta".to_string()),
                ("mid".to_string(), "beta".to_string()),
                ("mid".to_string(), "zeta".to_string()),
            ]
        );
        assert!(contradiction_pairs(&BTreeMap::new()).is_empty());
        let mut agreeing = BTreeMap::new();
        agreeing.insert("a".to_string(), Stance::Supports);
        agreeing.insert("b".to_string(), Stance::Supports);
        assert!(contradiction_pairs(&agreeing).is_empty());

        let topics = vec!["Graph Storage".to_string(), "query planner".to_string()];
        // item {graph, storage, engine}, topics {graph, storage, query, planner}:
        // intersection 2, union 5.
        assert_eq!(grade_feed("graph STORAGE engine", &topics), 400_000);
        assert_eq!(
            grade_feed("graph storage query planner", &topics),
            1_000_000
        );
        assert_eq!(grade_feed("unrelated words", &topics), 0);
        assert_eq!(grade_feed("", &[]), 0);

        let k = knobs();
        assert!(feed_is_relevant(300_000, &k));
        assert!(!feed_is_relevant(299_999, &k));
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.8
    #[test]
    fn seal_detects_any_field_change() {
        let artifact = compile_artifact(
            "t1",
            "a1",
            "k1",
            &["c2".to_string(), "c1".to_string(), "c1".to_string()],
            &["spec-1".to_string()],
            &[("c1".to_string(), "c2".to_string())],
        );
        assert_eq!(artifact.claim_ids, vec!["c1".to_string(), "c2".to_string()]);
        let seal = artifact.seal.clone().unwrap();
        assert_eq!(seal.digest.len(), 64);
        assert!(seal
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(verify_seal(&artifact), Ok(()));

        let again = compile_artifact(
            "t1",
            "a1",
            "k1",
            &["c1".to_string(), "c2".to_string()],
            &["spec-1".to_string()],
            &[("c1".to_string(), "c2".to_string())],
        );
        assert_eq!(again, artifact, "compile is deterministic");

        type Mutation = Box<dyn Fn(&mut ResearchArtifact)>;
        let mutations: Vec<Mutation> = vec![
            Box::new(|a| a.tenant_id.push('x')),
            Box::new(|a| a.artifact_id.push('x')),
            Box::new(|a| a.cohort_id.push('x')),
            Box::new(|a| a.claim_ids.push("c3".to_string())),
            Box::new(|a| a.code_specs.push("spec-2".to_string())),
            Box::new(|a| {
                a.exploration_edges
                    .push(("c2".to_string(), "c1".to_string()))
            }),
        ];
        for mutate in &mutations {
            let mut tampered = artifact.clone();
            mutate(&mut tampered);
            assert_eq!(
                verify_seal(&tampered),
                Err(ServerErrorCode::ArtifactSealViolation)
            );
        }

        let mut unsealed = artifact.clone();
        unsealed.seal = None;
        assert_eq!(
            verify_seal(&unsealed),
            Err(ServerErrorCode::ArtifactSealViolation)
        );
        let mut upper = artifact.clone();
        upper.seal = Some(ArtifactSeal {
            digest: seal.digest.to_uppercase(),
        });
        if upper.seal.as_ref().unwrap().digest != seal.digest {
            assert_eq!(
                verify_seal(&upper),
                Err(ServerErrorCode::ArtifactSealViolation)
            );
        }
        let mut short = artifact;
        short.seal = Some(ArtifactSeal {
            digest: "abc".to_string(),
        });
        assert_eq!(
            verify_seal(&short),
            Err(ServerErrorCode::ArtifactSealViolation)
        );
    }
}
