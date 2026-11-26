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
