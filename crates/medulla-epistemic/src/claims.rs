//! Claim lifecycle and evidence kernels (MEDULLA-DECISION-ENGINE-R652.2, .7, .8).
//!
//! Pure functions only: no store, no clock, no float. The `Claims` handler in
//! `medulla-h-decide` is the single writer of claim state and calls these to
//! decide a transition, combine evidence into signed log-odds, arbitrate
//! perspectives, grade a feed item and seal a research artifact.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::claims::{
    ArtifactSeal, ClaimRecord, ClaimState, Evidence, EvidenceChannel, ObserverRef, ProofReceipt,
    ProofStanding, ProofVerdict, ProvenClaim, ResearchArtifact, Stance,
};
use medulla_query_types::solve_scalar::{digest_text, DIGEST_TEXT_PREFIX};
use medulla_types::contract::ServerErrorCode;

/// Digest domain of a sealed research artifact.
pub const ARTIFACT_DIGEST_DOMAIN: &str = "research_artifact";

/// The axioms a machine-checked proof may rest on and still grant standing
/// (MEDULLA-DECISION-ENGINE-R652.11). A constant, not a knob: Lean names are
/// case-sensitive and compare exactly, and the set changes with a release.
pub const PROOF_STANDARD_AXIOMS: [&str; 3] = ["propext", "Classical.choice", "Quot.sound"];

/// Rule `PROVEN_STANDING`: a claim is `proven` when at least one of its
/// evidence records holds a receipt with verdict `pass`, no `sorry`, and every
/// axiom equal to one of [`PROOF_STANDARD_AXIOMS`]; otherwise `unproven`. A
/// failing receipt, a `sorry` and a non-standard axiom never grant standing.
pub fn proof_standing(evidence: &[Evidence]) -> ProofStanding {
    let proven = evidence
        .iter()
        .filter_map(|item| item.proof.as_ref())
        .any(|receipt| receipt_standing(receipt) == ProofStanding::Proven);
    if proven {
        ProofStanding::Proven
    } else {
        ProofStanding::Unproven
    }
}

/// The standing one receipt grants (MEDULLA-DECISION-ENGINE-R652.11): `proven` for a `pass`
/// verdict with no `sorry` over the standard axioms only, else `unproven`.
pub fn receipt_standing(receipt: &ProofReceipt) -> ProofStanding {
    let proven = receipt.verdict == ProofVerdict::Pass
        && !receipt.has_sorry
        && receipt
            .axioms
            .iter()
            .all(|axiom| PROOF_STANDARD_AXIOMS.contains(&axiom.as_str()));
    if proven {
        ProofStanding::Proven
    } else {
        ProofStanding::Unproven
    }
}

/// The `ProvenClaims` rows of one claim (MEDULLA-DECISION-ENGINE-R652.13): one per receipt with
/// verdict `pass` and no `sorry`, its standing derived from that receipt alone, ordered by
/// `claim_id`, then `declaration` (then the rest of the row, so the order is total). A receipt
/// recorded twice answers one row.
pub fn proven_rows(record: &ClaimRecord) -> Vec<ProvenClaim> {
    let mut rows: Vec<ProvenClaim> = record
        .evidence
        .iter()
        .filter_map(|item| item.proof.as_ref())
        .filter(|receipt| receipt.verdict == ProofVerdict::Pass && !receipt.has_sorry)
        .map(|receipt| ProvenClaim {
            claim_id: record.claim_id.clone(),
            declaration: receipt.declaration.clone(),
            checker_id: receipt.checker_id.clone(),
            toolchain_version: receipt.toolchain_version.clone(),
            artifact_digest: receipt.artifact_digest.clone(),
            axioms: receipt.axioms.clone(),
            standing: receipt_standing(receipt),
        })
        .collect();
    rows.sort_by(|a, b| {
        (&a.claim_id, &a.declaration, &a.artifact_digest, &a.checker_id)
            .cmp(&(&b.claim_id, &b.declaration, &b.artifact_digest, &b.checker_id))
    });
    rows.dedup();
    rows
}

/// Ppm denominator.
const PPM: i64 = 1_000_000;

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

/// Whether `from -> to` is an edge of `ClaimMachine`
/// (MEDULLA-FEDERATED-QUERY-R738.3), the one lifecycle table.
pub fn legal_transition(from: ClaimState, to: ClaimState) -> bool {
    from.move_to(to).is_ok()
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

/// Sum the signed contributions of `evidence` in micro-nats. Evidence that
/// carries a proof receipt is skipped.
pub fn combine_evidence<'a>(
    evidence: impl IntoIterator<Item = &'a Evidence>,
    knobs: &ClaimKnobs,
) -> i64 {
    // A proof receipt is typed evidence of a checker's verdict, never weight:
    // it moves no log-odds (MEDULLA-DECISION-ENGINE-R652.10).
    evidence
        .into_iter()
        .filter(|item| item.proof.is_none())
        .fold(0i64, |sum, item| {
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

/// The view one reader holds over claims: the observer it reads as and the sessions that
/// observer belongs to (MEDULLA-DECISION-ENGINE-R652.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderView {
    observer: ObserverRef,
    sessions: BTreeSet<String>,
}

impl ReaderView {
    /// A view that reads as `observer` over `sessions`.
    pub fn of(observer: ObserverRef, sessions: BTreeSet<String>) -> Self {
        Self { observer, sessions }
    }

    /// The observer this view reads as.
    pub fn observer(&self) -> &ObserverRef {
        &self.observer
    }
}

/// Whether `reader` may see `claim`: the one pure visibility rule (MEDULLA-DECISION-ENGINE-R652.9).
/// A claim proposed without a perspective keeps its present visibility. A claim of the shared
/// system observer is visible to every observer; a claim of one participant only to that
/// participant. In both cases every session its evidence names must be a session of the reader's
/// view; evidence that names no session never narrows a view.
pub fn claim_visible_to(claim: &ClaimRecord, reader: &ReaderView) -> bool {
    let Some(perspective) = &claim.perspective else {
        return true;
    };
    let observer_sees = match &perspective.observer {
        ObserverRef::System => true,
        participant => participant == &reader.observer,
    };
    observer_sees
        && claim
            .evidence
            .iter()
            .filter_map(|evidence| evidence.session_id.as_ref())
            .all(|session| reader.sessions.contains(session))
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
            session_id: None,
            proof: None,
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

    fn receipt_evidence(axioms: &[&str], has_sorry: bool, verdict: ProofVerdict) -> Evidence {
        Evidence {
            tenant_id: "t1".to_string(),
            evidence_id: "e1".to_string(),
            claim_id: "c1".to_string(),
            channel: EvidenceChannel::Process,
            supports: true,
            weight_ppm: 0,
            source_digest: "sha256:aa".to_string(),
            session_id: None,
            proof: Some(medulla_decision_types::claims::ProofReceipt {
                checker_id: "checker".to_string(),
                toolchain_version: "1".to_string(),
                declaration: "Decl.a".to_string(),
                artifact_digest: "ab".repeat(32),
                axioms: axioms.iter().map(|axiom| axiom.to_string()).collect(),
                has_sorry,
                verdict,
            }),
        }
    }

    /// Verifies MEDULLA-DECISION-ENGINE-R652.11: only a passing, sorry-free
    /// receipt over the standard axioms grants standing.
    // spec: MEDULLA-DECISION-ENGINE-R652.11
    #[test]
    fn proof_standing_needs_pass_no_sorry_and_standard_axioms() {
        let standard = ["propext", "Classical.choice", "Quot.sound"];
        assert_eq!(
            proof_standing(&[receipt_evidence(&standard, false, ProofVerdict::Pass)]),
            ProofStanding::Proven
        );
        assert_eq!(
            proof_standing(&[receipt_evidence(&[], false, ProofVerdict::Pass)]),
            ProofStanding::Proven
        );
        assert_eq!(
            proof_standing(&[receipt_evidence(&standard, false, ProofVerdict::Fail)]),
            ProofStanding::Unproven
        );
        assert_eq!(
            proof_standing(&[receipt_evidence(&standard, true, ProofVerdict::Fail)]),
            ProofStanding::Unproven
        );
        assert_eq!(
            proof_standing(&[receipt_evidence(&["propext", "Foo.axiom"], false, ProofVerdict::Pass)]),
            ProofStanding::Unproven
        );
        // Lean names are case-sensitive: lower-case `quot.sound` is not standard.
        assert_eq!(
            proof_standing(&[receipt_evidence(&["quot.sound"], false, ProofVerdict::Pass)]),
            ProofStanding::Unproven
        );
        assert_eq!(proof_standing(&[]), ProofStanding::Unproven);
        // One standard receipt among others is enough.
        assert_eq!(
            proof_standing(&[
                receipt_evidence(&["Foo.axiom"], false, ProofVerdict::Pass),
                receipt_evidence(&standard, false, ProofVerdict::Pass),
            ]),
            ProofStanding::Proven
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.9
    #[test]
    fn observer_sees_own_and_visible_system_claims_only() {
        use medulla_decision_types::claims::{ClaimPerspective, ParticipantId};

        let ada = ObserverRef::Participant(ParticipantId::new("ada").unwrap());
        let bob = ObserverRef::Participant(ParticipantId::new("bob").unwrap());
        let claim = |observer: ObserverRef, sessions: &[&str]| {
            let mut record = ClaimRecord {
                tenant_id: "t1".to_string(),
                claim_id: "c1".to_string(),
                perspective: Some(ClaimPerspective {
                    subject: ParticipantId::new("ada").unwrap(),
                    observer,
                }),
                state: ClaimState::Proposed,
                log_odds_micro: 0,
                transitions: Vec::new(),
                evidence: Vec::new(),
                stances: BTreeMap::new(),
                contradiction_pairs: Vec::new(),
                feeds: Vec::new(),
                proof_standing: ProofStanding::Unproven,
            };
            for (n, session) in sessions.iter().enumerate() {
                let mut e = evidence(&format!("e{n}"), EvidenceChannel::Outcome, true, 1);
                e.session_id = Some((*session).to_string());
                record.evidence.push(e);
            }
            record
        };
        let sessions = |names: &[&str]| -> BTreeSet<String> { names.iter().map(|n| n.to_string()).collect() };
        let view = |observer: &ObserverRef, names: &[&str]| ReaderView::of(observer.clone(), sessions(names));

        // Own claim: seen with its sessions visible, hidden when one is not.
        assert!(claim_visible_to(&claim(ada.clone(), &["s1"]), &view(&ada, &["s1"])));
        assert!(!claim_visible_to(&claim(ada.clone(), &["s1", "s2"]), &view(&ada, &["s1"])));
        // Another participant's claim is hidden even when the sessions match.
        assert!(!claim_visible_to(&claim(bob.clone(), &["s1"]), &view(&ada, &["s1"])));
        // A system claim is visible to every observer, with the session rule kept.
        assert!(claim_visible_to(&claim(ObserverRef::System, &["s1"]), &view(&ada, &["s1"])));
        assert!(claim_visible_to(&claim(ObserverRef::System, &["s1"]), &view(&bob, &["s1"])));
        assert!(!claim_visible_to(&claim(ObserverRef::System, &["s2"]), &view(&bob, &["s1"])));
        // Evidence without a session never narrows a view.
        assert!(claim_visible_to(&claim(ada.clone(), &[]), &view(&ada, &[])));
        // The system observer sees system claims, not a participant's.
        let system = view(&ObserverRef::System, &["s9"]);
        assert!(claim_visible_to(&claim(ObserverRef::System, &["s9"]), &system));
        assert!(!claim_visible_to(&claim(ada, &["s9"]), &system));
        // A claim with no perspective keeps its present visibility.
        let mut plain = claim(bob, &["s9"]);
        plain.perspective = None;
        assert!(claim_visible_to(&plain, &view(&ObserverRef::System, &[])));
    }
}
