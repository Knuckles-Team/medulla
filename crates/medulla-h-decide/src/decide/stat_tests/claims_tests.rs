//! Served claims tests (MEDULLA-DECISION-ENGINE-R652.3 .. .8): the lifecycle,
//! evidence on five channels, refusals that change no state, a read through a
//! second principal and the sealed artifact, all through the handler.

use std::collections::BTreeMap;

use super::*;
use crate::decide::claims::handle_claims;
use medulla_types::claims::{
    ClaimGetResult, ClaimOp, ClaimRecord, ClaimState, EvidenceChannel, ResearchArtifact, Stance,
};

const OTHER_TENANT: &str = "tenant-claims-other";

async fn call<T: serde::de::DeserializeOwned>(
    h: &Harness,
    who: &str,
    tenant: &str,
    op: ClaimOp,
) -> Result<T, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, tenant);
    decode(handle_claims(&h.state, 3, &verified, op).await)
}

async fn writer<T: serde::de::DeserializeOwned>(h: &Harness, op: ClaimOp) -> Result<T, String> {
    call(h, "claim-writer", TENANT, op).await
}

fn propose(claim_id: &str) -> ClaimOp {
    ClaimOp::Propose {
        claim_id: claim_id.to_string(),
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
        stances: BTreeMap::new(),
        feed: None,
    }
}

fn get_claim(claim_id: &str) -> ClaimOp {
    ClaimOp::Get {
        claim_id: Some(claim_id.to_string()),
        artifact_id: None,
    }
}

fn get_artifact(artifact_id: &str) -> ClaimOp {
    ClaimOp::Get {
        claim_id: None,
        artifact_id: Some(artifact_id.to_string()),
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
        *slot = Some(medulla_types::claims::FeedItem {
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
    assert_eq!(read.transitions[0].actor, "claim-writer");
    assert_eq!(
        read.contradiction_pairs,
        vec![("south".to_string(), "north".to_string())]
    );
    assert_eq!(
        call::<ClaimGetResult>(&h, "claim-reader", OTHER_TENANT, get_claim(id)).await,
        Err("CLAIM_NOT_FOUND".to_string())
    );
}
