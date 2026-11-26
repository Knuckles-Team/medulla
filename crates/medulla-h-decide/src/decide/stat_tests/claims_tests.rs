//! Served claims tests (MEDULLA-DECISION-ENGINE-R652.3 .. .8): the lifecycle,
//! evidence on five channels, refusals that change no state, a read through a
//! second principal and the sealed artifact, all through the handler.

use std::collections::BTreeMap;

use super::*;
use crate::decide::claims::handle_claims;
use medulla_decision_types::claims::{
    ClaimGetResult, ClaimOp, ClaimPerspective, ClaimRecord, ClaimState, EvidenceChannel,
    ObserverRef, ParticipantId, ProofReceipt, ProofVerdict, ResearchArtifact, Stance,
};

const OTHER_TENANT: &str = "tenant-claims-other";

async fn call<T: serde::de::DeserializeOwned>(
    h: &Harness,
    who: &str,
    tenant: &str,
    op: ClaimOp,
) -> Result<T, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, tenant);
    // The refusal text is "CODE: detail"; these tests pin the typed code.
    decode(handle_claims(&h.state, 3, "default", &verified, op).await).map_err(|text| {
        text.split_once(':')
            .map_or(text.as_str(), |(code, _)| code)
            .to_string()
    })
}

async fn writer<T: serde::de::DeserializeOwned>(h: &Harness, op: ClaimOp) -> Result<T, String> {
    call(h, "claim-writer", TENANT, op).await
}

fn propose(claim_id: &str) -> ClaimOp {
    ClaimOp::Propose {
        claim_id: claim_id.to_string(),
        perspective: None,
    }
}

fn advance(claim_id: &str, to: ClaimState) -> ClaimOp {
    ClaimOp::Advance {
        claim_id: claim_id.to_string(),
        to,
    }
}

fn evidence(
    claim_id: &str,
    evidence_id: &str,
    channel: Option<EvidenceChannel>,
    supports: bool,
    weight_ppm: u32,
) -> ClaimOp {
    ClaimOp::RecordEvidence {
        evidence_id: evidence_id.to_string(),
        claim_id: claim_id.to_string(),
        channel,
        supports,
        weight_ppm,
        source_digest: format!("sha256:{evidence_id}"),
        session_id: None,
        stances: BTreeMap::new(),
        feed: None,
        proof: None,
    }
}

fn proof_receipt() -> ProofReceipt {
    ProofReceipt {
        checker_id: "kernel-checker".to_string(),
        toolchain_version: "1.2.3".to_string(),
        declaration: "Mathlib.Foo.bar".to_string(),
        artifact_digest: "ab".repeat(32),
        axioms: vec!["Quot.sound".to_string(), "propext".to_string(), "propext".to_string()],
        has_sorry: false,
        verdict: ProofVerdict::Pass,
    }
}

fn receipt_evidence(
    claim_id: &str,
    evidence_id: &str,
    channel: EvidenceChannel,
    supports: bool,
    proof: ProofReceipt,
) -> ClaimOp {
    let mut op = evidence(claim_id, evidence_id, Some(channel), supports, 900_000);
    if let ClaimOp::RecordEvidence { proof: slot, .. } = &mut op {
        *slot = Some(proof);
    }
    op
}

fn get_claim(claim_id: &str) -> ClaimOp {
    ClaimOp::Get {
        claim_id: Some(claim_id.to_string()),
        artifact_id: None,
        as_observer: None,
    }
}

fn get_artifact(artifact_id: &str) -> ClaimOp {
    ClaimOp::Get {
        claim_id: None,
        artifact_id: Some(artifact_id.to_string()),
        as_observer: None,
    }
}

async fn read_claim(h: &Harness, claim_id: &str) -> ClaimRecord {
    let got: ClaimGetResult = writer(h, get_claim(claim_id)).await.unwrap();
    got.claim.unwrap()
}

const CHANNELS: [EvidenceChannel; 5] = [
    EvidenceChannel::Outcome,
    EvidenceChannel::Optimization,
    EvidenceChannel::Health,
    EvidenceChannel::Process,
    EvidenceChannel::Mined,
];

// spec: MEDULLA-DECISION-ENGINE-R652.1
// spec: MEDULLA-DECISION-ENGINE-R652.2
// spec: MEDULLA-DECISION-ENGINE-R652.6
// spec: MEDULLA-DECISION-ENGINE-R652.7
// spec: MEDULLA-DECISION-ENGINE-R652.8
#[tokio::test]
async fn claim_journey() {
    let h = Harness::new().await;
    let id = "journey-claim";
    let proposed: ClaimRecord = writer(&h, propose(id)).await.unwrap();
    assert_eq!(proposed.state, ClaimState::Proposed);
    assert_eq!(proposed.tenant_id, TENANT);
    assert_eq!(proposed.log_odds_micro, 0);

    // Evidence on all five channels at full weight: the cumulative log-odds
    // follow the channel priors.
    let expected = [1_000_000, 1_800_000, 2_400_000, 2_900_000, 3_200_000];
    for (index, channel) in CHANNELS.into_iter().enumerate() {
        let mut op = evidence(
            id,
            &format!("journey-e{index}"),
            Some(channel),
            true,
            1_000_000,
        );
        if let ClaimOp::RecordEvidence { stances, .. } = &mut op {
            stances.insert("alpha".to_string(), Stance::Supports);
            stances.insert("beta".to_string(), Stance::Opposes);
        }
        let record: ClaimRecord = writer(&h, op).await.unwrap();
        assert_eq!(record.log_odds_micro, expected[index], "{channel:?}");
        assert_eq!(record.evidence.len(), index + 1);
    }

    // A feed item below the relevance floor is stored but does not count.
    let mut feed = evidence(
        id,
        "journey-feed",
        Some(EvidenceChannel::Mined),
        false,
        1_000_000,
    );
    if let ClaimOp::RecordEvidence { feed: slot, .. } = &mut feed {
        *slot = Some(medulla_decision_types::claims::FeedItem {
            text: "weather forecast".to_string(),
            fleet_topics: vec!["graph storage".to_string()],
        });
    }
    let with_feed: ClaimRecord = writer(&h, feed).await.unwrap();
    assert_eq!(with_feed.log_odds_micro, 3_200_000);
    assert_eq!(with_feed.feeds.len(), 1);
    assert!(!with_feed.feeds[0].relevant);
    assert_eq!(with_feed.feeds[0].grade_ppm, 0);
    assert_eq!(
        with_feed.contradiction_pairs,
        vec![("alpha".to_string(), "beta".to_string())]
    );

    // The lifecycle: proposed -> supported -> accepted.
    let supported: ClaimRecord = writer(&h, advance(id, ClaimState::Supported))
        .await
        .unwrap();
    assert_eq!(supported.state, ClaimState::Supported);
    let accepted: ClaimRecord = writer(&h, advance(id, ClaimState::Accepted)).await.unwrap();
    assert_eq!(accepted.state, ClaimState::Accepted);
    let edges: Vec<(ClaimState, ClaimState)> = accepted
        .transitions
        .iter()
        .map(|t| (t.from, t.to))
        .collect();
    assert_eq!(
        edges,
        vec![
            (ClaimState::Proposed, ClaimState::Supported),
            (ClaimState::Supported, ClaimState::Accepted)
        ]
    );
    assert!(accepted.transitions.iter().all(|t| t.tenant_id == TENANT));

    // The sealed artifact compiles and reads back with a valid seal.
    let compiled: ResearchArtifact = writer(
        &h,
        ClaimOp::CompileArtifact {
            artifact_id: "journey-artifact".to_string(),
            cohort_id: "journey-cohort".to_string(),
            claim_ids: vec![id.to_string()],
            code_specs: vec!["spec-1".to_string()],
            exploration_edges: vec![(id.to_string(), "other".to_string())],
        },
    )
    .await
    .unwrap();
    assert_eq!(compiled.seal.as_ref().unwrap().digest.len(), 64);
    let read: ClaimGetResult = writer(&h, get_artifact("journey-artifact")).await.unwrap();
    assert_eq!(read.artifact, Some(compiled.clone()));

    // An illegal edge is refused and leaves the claim as it was.
    let before = read_claim(&h, id).await;
    assert_eq!(
        writer::<ClaimRecord>(&h, advance(id, ClaimState::Proposed)).await,
        Err("CLAIM_ILLEGAL_TRANSITION".to_string())
    );
    assert_eq!(read_claim(&h, id).await, before);

    // Another tenant neither reads the claim nor the artifact.
    for op in [get_claim(id), get_artifact("journey-artifact")] {
        assert_eq!(
            call::<ClaimGetResult>(&h, "intruder", OTHER_TENANT, op).await,
            Err("CLAIM_NOT_FOUND".to_string())
        );
    }
}

// spec: MEDULLA-DECISION-ENGINE-R652.3
#[tokio::test]
async fn claim_ops_reach_the_handler() {
    let h = Harness::new().await;
    let id = "ops-claim";
    let proposed: ClaimRecord = writer(&h, propose(id)).await.unwrap();
    assert_eq!(proposed.state, ClaimState::Proposed);
    // Propose is idempotent: a second call answers the same claim.
    let again: ClaimRecord = writer(&h, propose(id)).await.unwrap();
    assert_eq!(again, proposed);

    let recorded: ClaimRecord = writer(
        &h,
        evidence(
            id,
            "ops-e1",
            Some(EvidenceChannel::Outcome),
            true,
            1_000_000,
        ),
    )
    .await
    .unwrap();
    assert_eq!(recorded.evidence.len(), 1);
    assert_eq!(recorded.log_odds_micro, 1_000_000);
    // A repeated evidence id is not counted twice.
    let replay: ClaimRecord = writer(
        &h,
        evidence(
            id,
            "ops-e1",
            Some(EvidenceChannel::Outcome),
            true,
            1_000_000,
        ),
    )
    .await
    .unwrap();
    assert_eq!(replay.evidence.len(), 1);
    assert_eq!(replay.log_odds_micro, 1_000_000);

    let advanced: ClaimRecord = writer(&h, advance(id, ClaimState::Supported))
        .await
        .unwrap();
    assert_eq!(advanced.state, ClaimState::Supported);
    assert_eq!(advanced.transitions.len(), 1);
    assert_eq!(read_claim(&h, id).await, advanced);
}

// spec: MEDULLA-DECISION-ENGINE-R652.4
#[tokio::test]
async fn claim_refusals_change_no_state() {
    let h = Harness::new().await;
    let id = "refusal-claim";
    writer::<ClaimRecord>(&h, propose(id)).await.unwrap();
    writer::<ClaimRecord>(
        &h,
        evidence(
            id,
            "refusal-e1",
            Some(EvidenceChannel::Process),
            true,
            100_000,
        ),
    )
    .await
    .unwrap();
    let before = read_claim(&h, id).await;

    let refused: Vec<(&str, Result<ClaimRecord, String>)> = vec![
        (
            "CLAIM_ILLEGAL_TRANSITION",
            writer(&h, advance(id, ClaimState::Accepted)).await,
        ),
        (
            "CLAIM_GUARD_NOT_MET",
            writer(&h, advance(id, ClaimState::Supported)).await,
        ),
        (
            "CLAIM_NOT_FOUND",
            writer(&h, advance("refusal-missing", ClaimState::Retired)).await,
        ),
        (
            "EVIDENCE_NO_CHANNEL",
            writer(&h, evidence(id, "refusal-e2", None, true, 10)).await,
        ),
        (
            "EVIDENCE_WEIGHT_OUT_OF_RANGE",
            writer(
                &h,
                evidence(
                    id,
                    "refusal-e3",
                    Some(EvidenceChannel::Health),
                    true,
                    1_000_001,
                ),
            )
            .await,
        ),
    ];
    for (code, result) in refused {
        assert_eq!(result, Err(code.to_string()));
    }

    // ARTIFACT_SEAL_VIOLATION: an artifact stored with a wrong seal, and one
    // stored unsealed, are refused on read.
    let store = crate::decide::claims::served::ClaimStore::open(
        h.state.read().await.persist_dir.as_deref(),
        TENANT,
        "claim-writer",
    )
    .unwrap();
    let mut forged = medulla_epistemic::claims::compile_artifact(
        TENANT,
        "refusal-forged",
        "refusal-cohort",
        &[id.to_string()],
        &[],
        &[],
    );
    forged.code_specs.push("tampered after sealing".to_string());
    store.store_artifact(&forged).unwrap();
    let mut unsealed = forged.clone();
    unsealed.artifact_id = "refusal-unsealed".to_string();
    unsealed.seal = None;
    store.store_artifact(&unsealed).unwrap();
    for artifact in ["refusal-forged", "refusal-unsealed"] {
        assert_eq!(
            writer::<ClaimGetResult>(&h, get_artifact(artifact)).await,
            Err("ARTIFACT_SEAL_VIOLATION".to_string())
        );
    }

    assert_eq!(
        read_claim(&h, id).await,
        before,
        "no refusal changed the claim"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R652.5
#[tokio::test]
async fn claim_evidence_reads_back_through_a_second_principal() {
    let h = Harness::new().await;
    let id = "reader-claim";
    writer::<ClaimRecord>(&h, propose(id)).await.unwrap();
    let mut op = evidence(
        id,
        "reader-e1",
        Some(EvidenceChannel::Outcome),
        false,
        1_000_000,
    );
    if let ClaimOp::RecordEvidence { stances, .. } = &mut op {
        stances.insert("north".to_string(), Stance::Opposes);
        stances.insert("south".to_string(), Stance::Supports);
    }
    writer::<ClaimRecord>(&h, op).await.unwrap();
    writer::<ClaimRecord>(&h, advance(id, ClaimState::Contested))
        .await
        .unwrap();
    let written = read_claim(&h, id).await;

    let read: ClaimGetResult = call(&h, "claim-reader", TENANT, get_claim(id))
        .await
        .unwrap();
    let read = read.claim.unwrap();
    assert_eq!(read, written, "every field round-trips");
    assert_eq!(read.evidence.len(), 1);
    assert_eq!(read.evidence[0].channel, EvidenceChannel::Outcome);
    assert!(!read.evidence[0].supports);
    assert_eq!(read.evidence[0].weight_ppm, 1_000_000);
    assert_eq!(read.log_odds_micro, -1_000_000);
    assert_eq!(read.transitions.len(), 1);
    // A transition records the writer by its persistence id, the form every
    // durable store keys a principal under.
    let writer_id = VerifiedRequestContext::verified_for_test_in_tenant("claim-writer", TENANT)
        .principal_persistence_id();
    assert_eq!(read.transitions[0].actor, writer_id);
    assert_eq!(
        read.contradiction_pairs,
        vec![("south".to_string(), "north".to_string())]
    );
    assert_eq!(
        call::<ClaimGetResult>(&h, "claim-reader", OTHER_TENANT, get_claim(id)).await,
        Err("CLAIM_NOT_FOUND".to_string())
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R652.10: a proof receipt is recorded through
/// the one evidence path, moves no log-odds, keeps its axioms sorted and
/// deduplicated, and each refusal changes no state.
// spec: MEDULLA-DECISION-ENGINE-R652.10
#[tokio::test]
async fn proof_receipt_records_through_evidence_path_and_refusals_change_no_state() {
    let h = Harness::new().await;
    let id = "proof-claim";
    let _: ClaimRecord = writer(&h, propose(id)).await.unwrap();

    let recorded: ClaimRecord = writer(
        &h,
        receipt_evidence(id, "proof-e1", EvidenceChannel::Process, true, proof_receipt()),
    )
    .await
    .unwrap();
    // The receipt never moves the log-odds, however heavy its weight.
    assert_eq!(recorded.log_odds_micro, 0);
    let stored = recorded.evidence.last().unwrap();
    let receipt = stored.proof.as_ref().expect("the receipt is stored on the evidence");
    assert_eq!(receipt.axioms, ["Quot.sound", "propext"]);
    assert_eq!(receipt.verdict, ProofVerdict::Pass);

    let before = read_claim(&h, id).await;
    let invalid = |mut receipt: ProofReceipt| {
        receipt.artifact_digest = "not-hex".to_string();
        receipt
    };
    let sorry = ProofReceipt {
        has_sorry: true,
        ..proof_receipt()
    };
    let refused: Vec<(&str, Result<ClaimRecord, String>)> = vec![
        (
            "PROOF_RECEIPT_INVALID",
            writer(
                &h,
                receipt_evidence(id, "proof-e2", EvidenceChannel::Process, true, invalid(proof_receipt())),
            )
            .await,
        ),
        (
            "PROOF_RECEIPT_INVALID",
            writer(
                &h,
                receipt_evidence(id, "proof-e3", EvidenceChannel::Outcome, true, proof_receipt()),
            )
            .await,
        ),
        (
            "PROOF_RECEIPT_INVALID",
            writer(
                &h,
                receipt_evidence(id, "proof-e4", EvidenceChannel::Process, false, proof_receipt()),
            )
            .await,
        ),
        (
            "PROOF_SORRY_PRESENT",
            writer(
                &h,
                receipt_evidence(id, "proof-e5", EvidenceChannel::Process, true, sorry),
            )
            .await,
        ),
    ];
    for (code, result) in refused {
        assert_eq!(result, Err(code.to_string()));
    }
    assert_eq!(read_claim(&h, id).await, before, "a refusal changes no state");
}

/// Verifies MEDULLA-DECISION-ENGINE-R652.11: standing is recomputed from the
/// evidence on every read. A passing sorry-free receipt over the standard axioms
/// makes the claim `proven`; a receipt with a non-standard axiom is recorded and
/// readable but leaves the claim `unproven`, and standing never moves the state.
// spec: MEDULLA-DECISION-ENGINE-R652.11
#[tokio::test]
async fn get_reports_proven_standing_from_receipt() {
    use medulla_decision_types::claims::ProofStanding;
    let h = Harness::new().await;

    let plain = "standing-plain";
    let proposed: ClaimRecord = writer(&h, propose(plain)).await.unwrap();
    assert_eq!(proposed.proof_standing, ProofStanding::Unproven);

    let extra = "standing-extra-axiom";
    let _: ClaimRecord = writer(&h, propose(extra)).await.unwrap();
    let non_standard = ProofReceipt {
        axioms: vec!["Foo.axiom".to_string()],
        ..proof_receipt()
    };
    let recorded: ClaimRecord = writer(
        &h,
        receipt_evidence(extra, "standing-e1", EvidenceChannel::Process, true, non_standard),
    )
    .await
    .unwrap();
    assert_eq!(recorded.proof_standing, ProofStanding::Unproven);
    let kept = read_claim(&h, extra).await;
    assert_eq!(kept.evidence.len(), 1, "the receipt stays recorded and readable");
    assert_eq!(kept.proof_standing, ProofStanding::Unproven);

    let proven = "standing-proven";
    let _: ClaimRecord = writer(&h, propose(proven)).await.unwrap();
    let state_before = read_claim(&h, proven).await.state;
    let recorded: ClaimRecord = writer(
        &h,
        receipt_evidence(proven, "standing-e2", EvidenceChannel::Process, true, proof_receipt()),
    )
    .await
    .unwrap();
    assert_eq!(recorded.proof_standing, ProofStanding::Proven);
    let read = read_claim(&h, proven).await;
    assert_eq!(read.proof_standing, ProofStanding::Proven);
    assert_eq!(read.state, state_before, "proven standing does not change the state");

    // A failing receipt records without granting standing.
    let failing = "standing-fail";
    let _: ClaimRecord = writer(&h, propose(failing)).await.unwrap();
    let failed = ProofReceipt {
        verdict: ProofVerdict::Fail,
        ..proof_receipt()
    };
    let _: ClaimRecord = writer(
        &h,
        receipt_evidence(failing, "standing-e3", EvidenceChannel::Process, true, failed),
    )
    .await
    .unwrap();
    assert_eq!(read_claim(&h, failing).await.proof_standing, ProofStanding::Unproven);
}

/// Seed the request graph with the participants and threads a perspective test reads: the
/// participant shape the participant surface stores (a `json` property under the tenant scope).
async fn seed_participants(h: &Harness, tenant: &str, members: &[(&str, &str, &str)]) {
    use medulla_work_types::participant::{Participant, ParticipantKind};
    let guard = h.state.read().await;
    let core = guard.registry.get("default").expect("request graph").core.clone();
    for (slug, principal, session) in members {
        let participant = Participant::new(
            slug,
            tenant,
            "ws-main",
            ParticipantKind::Person,
            slug,
            Some(&format!("person:{slug}")),
            vec![(*principal).to_string()],
        )
        .unwrap();
        core.add_node(
            participant.iri(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Participant",
                "tenant_scope": tenant,
                "json": serde_json::to_string(&participant).unwrap(),
            }))
            .unwrap(),
        );
        let thread = format!("transcript-thread:{session}");
        core.add_node(
            thread.clone(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Transcript",
                "connector": "ws-main",
                "session_id": session,
            }))
            .unwrap(),
        );
        core.add_edge(
            thread,
            participant.iri(),
            rmp_serde::to_vec_named(&serde_json::json!({"relationship": "hasParticipant"})).unwrap(),
        )
        .unwrap();
    }
}

fn perspective_propose(claim_id: &str, subject: &str, observer: ObserverRef) -> ClaimOp {
    ClaimOp::Propose {
        claim_id: claim_id.to_string(),
        perspective: Some(ClaimPerspective {
            subject: ParticipantId::new(subject).unwrap(),
            observer,
            statement: None,
        }),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R652.9
#[tokio::test]
async fn perspective_claims_hide_other_observers() {
    let h = Harness::new().await;
    seed_participants(&h, TENANT, &[("ada", "ada-principal", "s1"), ("bo", "bo-principal", "s2")]).await;
    let ada = || ObserverRef::Participant(ParticipantId::new("ada").unwrap());
    let bo = || ObserverRef::Participant(ParticipantId::new("bo").unwrap());

    // Ada's own claim and a shared system claim, each grounded in session s1.
    for (claim, observer) in [("c-ada", ada()), ("c-system", ObserverRef::System)] {
        let stored: ClaimRecord =
            call(&h, "ada-principal", TENANT, perspective_propose(claim, "ada", observer)).await.unwrap();
        assert_eq!(stored.perspective.as_ref().unwrap().subject.as_str(), "ada");
        let mut op = evidence(claim, &format!("{claim}-e1"), Some(EvidenceChannel::Outcome), true, 100_000);
        if let ClaimOp::RecordEvidence { session_id, .. } = &mut op {
            *session_id = Some("s1".to_string());
        }
        let _: ClaimRecord = call(&h, "ada-principal", TENANT, op).await.unwrap();
    }
    let read = |who: &'static str, tenant: &'static str, claim: &'static str, as_observer: Option<ObserverRef>| {
        let h = &h;
        async move {
            call::<ClaimGetResult>(
                h,
                who,
                tenant,
                ClaimOp::Get { claim_id: Some(claim.to_string()), artifact_id: None, as_observer },
            )
            .await
        }
    };
    // The observer reads its own claim and the system claim of its own session.
    assert!(read("ada-principal", TENANT, "c-ada", None).await.unwrap().claim.is_some());
    assert!(read("ada-principal", TENANT, "c-system", None).await.unwrap().claim.is_some());
    // Another participant's principal sees neither: not its claim, and not its session.
    assert_eq!(read("bo-principal", TENANT, "c-ada", None).await.unwrap_err(), "PARTICIPANT_NOT_VISIBLE");
    assert_eq!(read("bo-principal", TENANT, "c-system", None).await.unwrap_err(), "PARTICIPANT_NOT_VISIBLE");
    // A principal that acts as nobody reads as the system observer over no session.
    assert_eq!(read("stranger", TENANT, "c-system", None).await.unwrap_err(), "PARTICIPANT_NOT_VISIBLE");
    // A request cannot name a view the principal does not hold.
    assert_eq!(
        read("bo-principal", TENANT, "c-ada", Some(ada())).await.unwrap_err(),
        "OBSERVER_NOT_PERMITTED"
    );
    assert!(read("bo-principal", TENANT, "c-ada", Some(bo())).await.is_err());

    // Cross-tenant negative: another tenant neither reads the claim nor proposes a perspective
    // over a participant it does not hold.
    assert_eq!(read("ada-principal", OTHER_TENANT, "c-ada", None).await.unwrap_err(), "CLAIM_NOT_FOUND");
    assert_eq!(
        call::<ClaimRecord>(&h, "ada-principal", OTHER_TENANT, perspective_propose("x", "ada", ObserverRef::System))
            .await
            .unwrap_err(),
        "PARTICIPANT_NOT_VISIBLE"
    );
}

fn receipt_of(declaration: &str, axioms: &[&str], has_sorry: bool, verdict: ProofVerdict) -> ProofReceipt {
    ProofReceipt {
        declaration: declaration.to_string(),
        axioms: axioms.iter().map(|axiom| axiom.to_string()).collect(),
        has_sorry,
        verdict,
        ..proof_receipt()
    }
}

async fn record_receipt(h: &Harness, tenant: &str, claim: &str, evidence_id: &str, proof: ProofReceipt) {
    let _: ClaimRecord = call(
        h,
        "claim-writer",
        tenant,
        receipt_evidence(claim, evidence_id, EvidenceChannel::Process, true, proof),
    )
    .await
    .unwrap();
}

fn proven_op(axiom: Option<&str>, limit: Option<u32>) -> ClaimOp {
    ClaimOp::ProvenClaims { axiom: axiom.map(str::to_string), limit }
}

/// Verifies MEDULLA-DECISION-ENGINE-R652.13: `ProvenClaims` lists one row per claim and passing,
/// sorry-free receipt, filters by axiom, sorts by claim id then declaration, bounds its limit and
/// lists only the verified tenant's claims.
// spec: MEDULLA-DECISION-ENGINE-R652.13
#[tokio::test]
async fn proven_claims_lists_checked_claims_with_axioms_and_isolates_tenants() {
    use medulla_decision_types::claims::{ProofStanding, ProvenClaimsResult};
    let h = Harness::new().await;
    for claim in ["pc-b", "pc-a", "pc-sorry", "pc-fail"] {
        let _: ClaimRecord = writer(&h, propose(claim)).await.unwrap();
    }
    let standard = ["Classical.choice", "Quot.sound", "propext"];
    // Two receipts on one claim: one row each, sorted by declaration.
    record_receipt(&h, TENANT, "pc-a", "e-a2", receipt_of("Z.last", &standard, false, ProofVerdict::Pass)).await;
    record_receipt(&h, TENANT, "pc-a", "e-a1", receipt_of("A.first", &standard, false, ProofVerdict::Pass)).await;
    // A non-standard axiom is listed, as `unproven`, and always shows its axioms.
    record_receipt(&h, TENANT, "pc-b", "e-b", receipt_of("B.decl", &["Foo.axiom"], false, ProofVerdict::Pass)).await;
    // A `sorry` and a failing receipt are recorded but never listed.
    record_receipt(&h, TENANT, "pc-sorry", "e-s", receipt_of("S.decl", &[], true, ProofVerdict::Fail)).await;
    record_receipt(&h, TENANT, "pc-fail", "e-f", receipt_of("F.decl", &[], false, ProofVerdict::Fail)).await;
    // Another tenant's claim and receipt.
    let _: ClaimRecord = call(&h, "claim-writer", OTHER_TENANT, propose("pc-other")).await.unwrap();
    record_receipt(&h, OTHER_TENANT, "pc-other", "e-o", receipt_of("O.decl", &standard, false, ProofVerdict::Pass)).await;

    let all: ProvenClaimsResult = writer(&h, proven_op(None, None)).await.unwrap();
    let rows: Vec<(&str, &str, ProofStanding)> = all
        .claims
        .iter()
        .map(|row| (row.claim_id.as_str(), row.declaration.as_str(), row.standing))
        .collect();
    assert_eq!(
        rows,
        [
            ("pc-a", "A.first", ProofStanding::Proven),
            ("pc-a", "Z.last", ProofStanding::Proven),
            ("pc-b", "B.decl", ProofStanding::Unproven),
        ]
    );
    assert_eq!(all.claims[2].axioms, ["Foo.axiom"]);
    assert_eq!(all.claims[0].axioms, standard);
    assert_eq!(all.claims[0].checker_id, "kernel-checker");

    // The axiom filter keeps the rows whose receipt names the axiom.
    let foo: ProvenClaimsResult = writer(&h, proven_op(Some("Foo.axiom"), None)).await.unwrap();
    assert_eq!(foo.claims.len(), 1);
    assert_eq!(foo.claims[0].claim_id, "pc-b");
    let case: ProvenClaimsResult = writer(&h, proven_op(Some("foo.axiom"), None)).await.unwrap();
    assert!(case.claims.is_empty(), "axiom names compare exactly");

    // The limit cuts the answer; a larger one than the most is refused.
    let one: ProvenClaimsResult = writer(&h, proven_op(None, Some(1))).await.unwrap();
    assert_eq!(one.claims.len(), 1);
    assert_eq!(
        writer::<ProvenClaimsResult>(&h, proven_op(None, Some(1001))).await.unwrap_err(),
        "PARAMETER_INVALID"
    );

    // A second tenant sees only its own claim, and the first sees none of it.
    let other: ProvenClaimsResult = call(&h, "claim-writer", OTHER_TENANT, proven_op(None, None)).await.unwrap();
    assert_eq!(other.claims.len(), 1);
    assert_eq!(other.claims[0].claim_id, "pc-other");
    assert!(all.claims.iter().all(|row| row.claim_id != "pc-other"));
}

/// Verifies MEDULLA-DECISION-ENGINE-R652.14: the machine-checked standing journey. A passing receipt
/// reads `proven`; a `sorry`, a malformed receipt and a non-standard axiom leave standing as it was;
/// `ProvenClaims` filtered by axiom lists the non-standard receipt; a second tenant reads none.
// spec: MEDULLA-DECISION-ENGINE-R652.14
#[tokio::test]
async fn proof_journey() {
    use medulla_decision_types::claims::{ProofStanding, ProvenClaimsResult};
    let h = Harness::new().await;
    let id = "journey-claim";
    let _: ClaimRecord = writer(&h, propose(id)).await.unwrap();
    let standard = ["Classical.choice", "Quot.sound", "propext"];

    record_receipt(&h, TENANT, id, "j1", receipt_of("Main.thm", &standard, false, ProofVerdict::Pass)).await;
    assert_eq!(read_claim(&h, id).await.proof_standing, ProofStanding::Proven);

    // A pass with `sorry`, and a malformed receipt, are refused and change nothing.
    let before = read_claim(&h, id).await;
    let refused_sorry: Result<ClaimRecord, String> = writer(
        &h,
        receipt_evidence(
            id,
            "j2",
            EvidenceChannel::Process,
            true,
            receipt_of("Main.sorry", &[], true, ProofVerdict::Pass),
        ),
    )
    .await;
    assert_eq!(refused_sorry, Err("PROOF_SORRY_PRESENT".to_string()));
    let mut malformed = receipt_of("Main.bad", &[], false, ProofVerdict::Pass);
    malformed.artifact_digest = "xyz".to_string();
    let refused_invalid: Result<ClaimRecord, String> =
        writer(&h, receipt_evidence(id, "j3", EvidenceChannel::Process, true, malformed)).await;
    assert_eq!(refused_invalid, Err("PROOF_RECEIPT_INVALID".to_string()));
    assert_eq!(read_claim(&h, id).await, before);

    // A non-standard axiom records without refusal; the claim keeps its standing.
    record_receipt(&h, TENANT, id, "j4", receipt_of("Main.extra", &["Foo.axiom"], false, ProofVerdict::Pass)).await;
    assert_eq!(read_claim(&h, id).await.proof_standing, ProofStanding::Proven);

    // The list, filtered by axiom, shows the non-standard receipt as `unproven`.
    let foo: ProvenClaimsResult = writer(&h, proven_op(Some("Foo.axiom"), None)).await.unwrap();
    assert_eq!(foo.claims.len(), 1);
    assert_eq!(foo.claims[0].declaration, "Main.extra");
    assert_eq!(foo.claims[0].standing, ProofStanding::Unproven);
    let all: ProvenClaimsResult = writer(&h, proven_op(None, None)).await.unwrap();
    assert_eq!(all.claims.len(), 2);

    // A second tenant reads none of it.
    let other: ProvenClaimsResult = call(&h, "claim-writer", OTHER_TENANT, proven_op(None, None)).await.unwrap();
    assert!(other.claims.is_empty());
}

/// Serve one operation and return the memory events it committed.
async fn planned(
    h: &Harness,
    who: &str,
    op: ClaimOp,
) -> crate::decide::claims::ClaimsServed {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, TENANT);
    crate::decide::claims::handle_claims_planned(&h.state, 3, "default", &verified, op).await
}

// spec: MEDULLA-DECISION-ENGINE-R073.6
#[tokio::test]
async fn a_perspective_claim_transition_announces_one_event_after_commit() {
    use medulla_decision_types::memory_events::{ClaimChanged, MemoryEvent};
    let h = Harness::new().await;
    seed_participants(&h, TENANT, &[("ada", "ada-principal", "s1")]).await;
    let observer = ObserverRef::System;
    let _: ClaimRecord = call(&h, "ada-principal", TENANT, perspective_propose("c-event", "ada", observer.clone())).await.unwrap();
    let _: ClaimRecord = call(&h, "ada-principal", TENANT, evidence("c-event", "e1", Some(EvidenceChannel::Outcome), true, 900_000)).await.unwrap();
    // A refused transition announces nothing.
    let refused = planned(&h, "ada-principal", advance("c-event", ClaimState::Retired)).await;
    if refused.response.refusal_text().is_some() {
        assert!(refused.events.is_empty());
    }
    // The committed transition announces exactly one event naming the states and the perspective.
    let served = planned(&h, "ada-principal", advance("c-event", ClaimState::Supported)).await;
    assert!(served.response.refusal_text().is_none());
    assert_eq!(
        served.events,
        vec![MemoryEvent::ClaimChanged(ClaimChanged {
            claim_id: "c-event".into(),
            from: ClaimState::Proposed,
            to: ClaimState::Supported,
            subject: ParticipantId::new("ada").unwrap(),
            observer,
        })]
    );
    // A claim with no perspective, and a read, announce nothing.
    let _: ClaimRecord = writer(&h, propose("c-plain")).await.unwrap();
    let _: ClaimRecord = writer(&h, evidence("c-plain", "p1", Some(EvidenceChannel::Outcome), true, 900_000)).await.unwrap();
    assert!(planned(&h, "claim-writer", advance("c-plain", ClaimState::Supported)).await.events.is_empty());
    assert!(planned(&h, "claim-writer", get_claim("c-plain")).await.events.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R651.12
#[tokio::test]
async fn a_worker_proposes_conclusion_claims_with_turn_evidence() {
    use medulla_decision_stats::decision::participant_derive::{conclusion_claim_id, DeriveKey};
    let h = Harness::new().await;
    seed_participants(&h, TENANT, &[("ada", "worker-principal", "s1")]).await;
    let key = DeriveKey {
        scope_id: String::new(),
        session_id: "s1".into(),
        subject: ParticipantId::new("ada").unwrap(),
        observer: ObserverRef::System,
    };
    // Two derive jobs reach the same conclusion in different words; each cites its own turn.
    let mut stored = Vec::new();
    for (conclusion, turn) in [("Prefers  tea", "t1"), ("prefers TEA", "t4")] {
        let claim_id = conclusion_claim_id(&key, conclusion);
        let claim: ClaimRecord = call(
            &h,
            "worker-principal",
            TENANT,
            perspective_propose(&claim_id, "ada", ObserverRef::System),
        )
        .await
        .unwrap();
        let mut op = evidence(&claim_id, &format!("{claim_id}:{turn}"), Some(EvidenceChannel::Mined), true, 200_000);
        if let ClaimOp::RecordEvidence { session_id, source_digest, .. } = &mut op {
            *session_id = Some("s1".to_string());
            *source_digest = format!("turn:{turn}");
        }
        let _: ClaimRecord = call(&h, "worker-principal", TENANT, op).await.unwrap();
        stored.push((claim.claim_id, turn));
    }
    // One claim, two pieces of mined evidence, each citing its turn.
    assert_eq!(stored[0].0, stored[1].0);
    let got: ClaimGetResult = call(&h, "worker-principal", TENANT, get_claim(&stored[0].0)).await.unwrap();
    let record = got.claim.unwrap();
    assert_eq!(record.evidence.len(), 2);
    assert!(record.evidence.iter().all(|e| e.channel == EvidenceChannel::Mined));
    let digests: Vec<&str> = record.evidence.iter().map(|e| e.source_digest.as_str()).collect();
    assert_eq!(digests, ["turn:t1", "turn:t4"]);
}

async fn ask_as(
    h: &Harness,
    who: &str,
    tenant: &str,
    subject: &str,
    observer: ObserverRef,
    text: &str,
) -> Result<medulla_decision_types::participant_memory::ParticipantAnswer, String> {
    use medulla_decision_types::participant_memory::ParticipantQuestion;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, tenant);
    let question = ParticipantQuestion {
        subject: ParticipantId::new(subject).unwrap(),
        observer,
        question: text.to_string(),
        memory_scope: None,
        as_of_ms: None,
    };
    decode(crate::decide::handle_participant_ask(&h.state, 3, "default", &verified, question).await)
        .map_err(|text| text.split_once(':').map_or(text.as_str(), |(code, _)| code).to_string())
}

// spec: MEDULLA-DECISION-ENGINE-R655.2
#[tokio::test]
async fn ask_cites_claims_and_turns_or_abstains() {
    let h = Harness::new().await;
    seed_participants(&h, TENANT, &[("ada", "ada-principal", "s1"), ("bo", "bo-principal", "s2")]).await;
    let ada = || ObserverRef::Participant(ParticipantId::new("ada").unwrap());
    let mut propose_op = perspective_propose("c-tea", "ada", ada());
    if let ClaimOp::Propose { perspective: Some(perspective), .. } = &mut propose_op {
        perspective.statement = Some("Prefers green tea".to_string());
    }
    let _: ClaimRecord = call(&h, "ada-principal", TENANT, propose_op).await.unwrap();
    for (n, turn) in ["m1", "m4"].iter().enumerate() {
        let mut op = evidence("c-tea", &format!("e{n}"), Some(EvidenceChannel::Mined), true, 400_000);
        if let ClaimOp::RecordEvidence { session_id, source_digest, .. } = &mut op {
            *session_id = Some("s1".to_string());
            *source_digest = format!("turn:{turn}");
        }
        let _: ClaimRecord = call(&h, "ada-principal", TENANT, op).await.unwrap();
    }

    // A grounded question is answered with the claim and its turns.
    let answer = ask_as(&h, "ada-principal", TENANT, "ada", ada(), "what tea does she prefer").await.unwrap();
    assert_eq!(answer.text, "Prefers green tea.");
    assert!(answer.abstained.is_none());
    assert_eq!(answer.citations.len(), 1);
    assert_eq!(answer.citations[0].claim_id, "c-tea");
    assert_eq!(answer.citations[0].turn_ids, ["m1", "m4"]);

    // A question nothing grounds abstains and names the code.
    let none = ask_as(&h, "ada-principal", TENANT, "ada", ada(), "where was she born").await.unwrap();
    assert!(none.text.is_empty() && none.citations.is_empty());
    let reason = format!("{:?}", none.abstained.expect("an abstention"));
    assert!(reason.contains("NO_GROUNDED_CLAIMS"), "{reason}");

    // An absent subject and another tenant's caller read the same envelope.
    assert_eq!(
        ask_as(&h, "ada-principal", TENANT, "ghost", ada(), "tea").await,
        Err("PARTICIPANT_NOT_VISIBLE".to_string())
    );
    assert_eq!(
        ask_as(&h, "ada-principal", OTHER_TENANT, "ada", ada(), "tea").await,
        Err("PARTICIPANT_NOT_VISIBLE".to_string())
    );
    // A caller that does not act as the observer is refused.
    assert_eq!(
        ask_as(&h, "bo-principal", TENANT, "ada", ada(), "tea").await,
        Err("OBSERVER_NOT_PERMITTED".to_string())
    );
    // An empty question is an argument error.
    assert_eq!(
        ask_as(&h, "ada-principal", TENANT, "ada", ada(), "  ").await,
        Err("INVALID_ARGUMENT".to_string())
    );
}
