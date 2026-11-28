//! Served belief revision tests (MEDULLA-DECISION-ENGINE-R700.3, R700.4,
//! R700.6, R700.7): a pass queued by a committed batch reaches the handler, refusals
//! leave the ledger untouched, and a detected contradiction is tracked, read
//! through UQL and moved only in status order.

use std::collections::BTreeMap;

use super::*;
use crate::decide::belief_revise::{enqueue_touched, handle_belief_revise};
use crate::decide::claims::handle_claims;
use medulla_mutation_types::mutation_scope::MutationOutboxIntent;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::belief_revision::BeliefPassResult,
    medulla_decision_statistical_types::belief_revision::BeliefReviseRequest,
    medulla_decision_statistical_types::belief_revision::ContradictionMove,
    medulla_decision_statistical_types::belief_revision::ContradictionStatus,
    medulla_decision_statistical_types::belief_revision::PremiseRef,
};
use {
    medulla_decision_statistical_types::claims::ClaimOp,
    medulla_decision_statistical_types::claims::ClaimRecord,
    medulla_decision_statistical_types::claims::EvidenceChannel,
};

const WHO: &str = "belief-writer";
const GRAPH: &str = "kg-belief";
const HIGHER: &str = "cache-hit-rate-is-higher";
const LOWER: &str = "cache-hit-rate-is-lower";
const OTHER_TENANT: &str = "tenant-other";

fn verified(tenant: &str) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_in_tenant(WHO, tenant)
}

async fn belief_harness() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation(WHO)).await;
    h.state
        .write()
        .await
        .registry
        .create_graph(
            GRAPH,
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    h
}

async fn claim_op<T: serde::de::DeserializeOwned>(h: &Harness, op: ClaimOp) -> Result<T, String> {
    decode(handle_claims(&h.state, 3, "default", &verified(TENANT), op).await)
}

async fn claim_with_evidence(h: &Harness, claim_id: &str, supports: bool, weight_ppm: u32) {
    let _: ClaimRecord = claim_op(
        h,
        ClaimOp::Propose {
            claim_id: claim_id.to_string(),
            perspective: None,
        },
    )
    .await
    .unwrap();
    let _: ClaimRecord = claim_op(
        h,
        ClaimOp::RecordEvidence {
            evidence_id: format!("e-{claim_id}"),
            claim_id: claim_id.to_string(),
            channel: Some(EvidenceChannel::Outcome),
            supports,
            weight_ppm,
            source_digest: format!("sha256:{claim_id}"),
            session_id: None,
            stances: BTreeMap::new(),
            feed: None,
            proof: None,
        },
    )
    .await
    .unwrap();
}

/// The text of a claim: the two opposed claims read as long sentences so that
/// their token overlap clears `belief.opposition_similarity_min`.
fn text_of(claim_id: &str) -> String {
    let stem = "the cache hit rate of the read path in production is";
    match claim_id {
        HIGHER => format!("{stem} higher"),
        LOWER => format!("{stem} lower"),
        other => other.replace('-', " "),
    }
}

fn request(claims: &[&str]) -> BeliefReviseRequest {
    BeliefReviseRequest {
        graph: GRAPH.to_string(),
        claim_ids: claims.iter().map(|id| id.to_string()).collect(),
        texts: claims
            .iter()
            .map(|id| (id.to_string(), text_of(id)))
            .collect(),
        premises: Vec::new(),
        advance: Vec::new(),
    }
}

async fn pass(h: &Harness, request: BeliefReviseRequest) -> Result<BeliefPassResult, String> {
    pass_as(h, TENANT, request).await
}

async fn pass_as(
    h: &Harness,
    tenant: &str,
    request: BeliefReviseRequest,
) -> Result<BeliefPassResult, String> {
    decode(handle_belief_revise(&h.state, 4, &verified(tenant), request).await)
}

async fn claim_of(h: &Harness, claim_id: &str) -> ClaimRecord {
    claim_op(
        h,
        ClaimOp::Get {
            claim_id: Some(claim_id.into()),
            artifact_id: None,
            as_observer: None,
        },
    )
    .await
    .map(|got: medulla_decision_statistical_types::claims::ClaimGetResult| got.claim.unwrap())
    .unwrap()
}

fn touching(claims: &[&str]) -> MutationOutboxIntent {
    MutationOutboxIntent {
        topic: "graph.batch".to_string(),
        key: "batch-1".to_string(),
        payload: Vec::new(),
        headers: BTreeMap::new(),
    }
    .with_touched_claims(&claims.iter().map(|id| id.to_string()).collect::<Vec<_>>())
}

/// A committed batch names the claims it touched; the queue drains into the
/// next pass, which revises those claims through the handler.
// spec: MEDULLA-DECISION-ENGINE-R700.3
#[tokio::test]
async fn belief_pass_reaches_the_handler() {
    let h = belief_harness().await;
    claim_with_evidence(&h, "latency-improved", true, 1_000_000).await;
    claim_with_evidence(&h, "memory-grew", false, 500_000).await;

    let intent = touching(&["latency-improved", "memory-grew"]);
    assert_eq!(
        intent.touched_claims(),
        vec!["latency-improved".to_string(), "memory-grew".to_string()]
    );
    let dir = h.state.read().await.persist_dir.clone();
    let queued = enqueue_touched(dir.as_deref(), TENANT, WHO, GRAPH, &intent).unwrap();
    assert_eq!(queued, 2);

    let drained = pass(&h, request(&[])).await.unwrap();
    assert_eq!(drained.claims_visited, 2);
    let by_claim: BTreeMap<_, _> = drained
        .revisions
        .iter()
        .map(|r| (r.claim_id.as_str(), r.posterior_micro))
        .collect();
    assert_eq!(by_claim["latency-improved"], 1_000_000);
    assert_eq!(by_claim["memory-grew"], -500_000);

    // The queue is empty and unchanged evidence is not visited again.
    let again = pass(&h, request(&["latency-improved"])).await.unwrap();
    assert_eq!(again.claims_visited, 0);
    assert!(again.revisions.is_empty());
    let none = pass(&h, request(&[])).await;
    assert!(none.unwrap_err().starts_with("PARAMETER_INVALID"));
}

/// Both refusal codes, and a low posterior that proposes a retraction without
/// changing the claim.
// spec: MEDULLA-DECISION-ENGINE-R700.4
#[tokio::test]
async fn belief_refusals_leave_state_unchanged() {
    let h = belief_harness().await;
    // One piece of evidence weighs at most WEIGHT_PPM_MAX (1.0 nat at the
    // outcome prior), short of the -1.386 nat retraction threshold: two full
    // refutations put the claim below it.
    claim_with_evidence(&h, "disk-is-fast", false, 1_000_000).await;
    let _: ClaimRecord = claim_op(
        &h,
        ClaimOp::RecordEvidence {
            evidence_id: "e-disk-is-fast-2".to_string(),
            claim_id: "disk-is-fast".to_string(),
            channel: Some(EvidenceChannel::Outcome),
            supports: false,
            weight_ppm: 1_000_000,
            source_digest: "sha256:disk-is-fast-2".to_string(),
            session_id: None,
            stances: BTreeMap::new(),
            feed: None,
            proof: None,
        },
    )
    .await
    .unwrap();
    let before: ClaimRecord = claim_op(
        &h,
        ClaimOp::Get {
            claim_id: Some("disk-is-fast".into()),
            artifact_id: None,
            as_observer: None,
        },
    )
    .await
    .map(|got: medulla_decision_statistical_types::claims::ClaimGetResult| got.claim.unwrap())
    .unwrap();

    let unknown = pass(&h, request(&["no-such-claim"])).await;
    assert!(unknown.unwrap_err().starts_with("BELIEF_CLAIM_NOT_FOUND"));

    let mut missing = request(&["disk-is-fast"]);
    missing.premises.push(PremiseRef {
        claim_id: "disk-is-fast".into(),
        premise_id: "absent-premise".into(),
        supports: true,
        channel: EvidenceChannel::Health,
        weight_ppm: 100_000,
        class: EvidenceClass::Observation,
    });
    let error = pass(&h, missing).await.unwrap_err();
    assert!(error.starts_with("BELIEF_PREMISE_MISSING"));
    assert!(
        error.contains("absent-premise"),
        "the detail names the premise"
    );

    // Both refusals left the ledger empty: the first real pass is pass zero and
    // visits the claim.
    let proposal = pass(&h, request(&["disk-is-fast"])).await.unwrap();
    assert_eq!(proposal.claims_visited, 1);
    assert_eq!(proposal.retractions.len(), 1);
    assert_eq!(proposal.retractions[0].claim_id, "disk-is-fast");
    assert_eq!(proposal.retractions[0].evidence_class, EvidenceClass::Claim);

    let after: ClaimRecord = claim_op(
        &h,
        ClaimOp::Get {
            claim_id: Some("disk-is-fast".into()),
            artifact_id: None,
            as_observer: None,
        },
    )
    .await
    .map(|got: medulla_decision_statistical_types::claims::ClaimGetResult| got.claim.unwrap())
    .unwrap();
    assert_eq!(
        after, before,
        "a retraction proposal never mutates the claim"
    );
}

/// `CONTRADICTS <right>` over the graph: the claims linked to `right`.
#[cfg(feature = "epistemic")]
fn contradicting(core: &medulla_graph_core::graph::GraphCore, right: &str) -> Vec<String> {
    use medulla_query_types::wire::UqlResult;
    let graph = core.analysis_snapshot();
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let ctx = medulla_exec::exec::PlanCtx::new(&graph, &semantic);
    let text = format!("CONTRADICTS '{right}'");
    let stmt = medulla_uql::parse_statement(&text, &medulla_uql::Params::new()).unwrap();
    let UqlResult::Rows { rows, .. } =
        medulla_plan::uql::serve::run_statement(&stmt, &ctx).unwrap()
    else {
        panic!("rows")
    };
    rows.into_iter().map(|row| row.id).collect()
}

/// A detected contradiction persists, becomes a `CONTRADICTS` edge that UQL
/// reads, and moves `open`, `proposed`, `resolved` in that order only.
// spec: MEDULLA-DECISION-ENGINE-R700.7
#[cfg(feature = "epistemic")]
#[tokio::test]
async fn contradiction_is_tracked_and_status_is_ordered() {
    let h = belief_harness().await;
    let core = h
        .state
        .read()
        .await
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .clone();
    for id in [HIGHER, LOWER] {
        core.add_node(
            id.to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({"type": "Claim"})).unwrap(),
        );
    }
    claim_with_evidence(&h, HIGHER, true, 900_000).await;
    claim_with_evidence(&h, LOWER, false, 900_000).await;

    let result = pass(&h, request(&[HIGHER, LOWER])).await.unwrap();
    assert_eq!(result.friction.len(), 1);
    assert_eq!(result.contradictions.len(), 1);
    let record = &result.contradictions[0];
    assert_eq!(
        (record.left.as_str(), record.right.as_str()),
        (HIGHER, LOWER)
    );
    assert_eq!(record.status, ContradictionStatus::Open);
    assert_eq!(record.evidence.len(), 2);

    let linked = contradicting(&core, LOWER);
    assert_eq!(linked, vec![HIGHER.to_string()], "UQL returns the pair");

    let step = |to| {
        let mut moving = request(&[]);
        moving.advance.push(ContradictionMove {
            contradiction_id: record.contradiction_id.clone(),
            to,
        });
        moving
    };
    let skipped = pass(&h, step(ContradictionStatus::Resolved)).await;
    assert!(skipped
        .unwrap_err()
        .starts_with("CONTRADICTION_STATUS_ORDER"));
    let proposed = pass(&h, step(ContradictionStatus::Proposed)).await.unwrap();
    assert_eq!(
        proposed.contradictions[0].status,
        ContradictionStatus::Proposed
    );
    let resolved = pass(&h, step(ContradictionStatus::Resolved)).await.unwrap();
    assert_eq!(
        resolved.contradictions[0].status,
        ContradictionStatus::Resolved
    );
    let backwards = pass(&h, step(ContradictionStatus::Open)).await;
    assert!(backwards
        .unwrap_err()
        .starts_with("CONTRADICTION_STATUS_ORDER"));
}

/// The hot path end to end (MEDULLA-DECISION-ENGINE-R700.6): supporting and
/// opposing evidence is committed, one pass revises the claims and cites a
/// premise, reports a friction finding between the opposed pair and proposes a
/// retraction for a refuted claim. Both refusals change nothing, no claim is
/// ever retracted, and a second tenant neither sees nor shares the first
/// tenant's claims or ledger.
// spec: MEDULLA-DECISION-ENGINE-R700.6
#[tokio::test]
async fn belief_pass_proposes_and_never_retracts() {
    let h = belief_harness().await;
    claim_with_evidence(&h, HIGHER, true, 900_000).await;
    claim_with_evidence(&h, LOWER, false, 900_000).await;
    claim_with_evidence(&h, "premise-claim", true, 400_000).await;
    claim_with_evidence(&h, "disk-is-fast", false, 1_000_000).await;
    let _: ClaimRecord = claim_op(
        &h,
        ClaimOp::RecordEvidence {
            evidence_id: "e-disk-is-fast-2".to_string(),
            claim_id: "disk-is-fast".to_string(),
            channel: Some(EvidenceChannel::Outcome),
            supports: false,
            weight_ppm: 1_000_000,
            source_digest: "sha256:disk-is-fast-2".to_string(),
            session_id: None,
            stances: BTreeMap::new(),
            feed: None,
            proof: None,
        },
    )
    .await
    .unwrap();
    let ids = [HIGHER, LOWER, "disk-is-fast"];
    let before: Vec<ClaimRecord> = {
        let mut all = Vec::new();
        for id in ids {
            all.push(claim_of(&h, id).await);
        }
        all
    };

    // Refusals first: they change no durable state.
    let unknown = pass(&h, request(&["no-such-claim"])).await;
    assert!(unknown.unwrap_err().starts_with("BELIEF_CLAIM_NOT_FOUND"));
    let mut missing = request(&[HIGHER]);
    missing.premises.push(PremiseRef {
        claim_id: HIGHER.into(),
        premise_id: "absent-premise".into(),
        supports: true,
        channel: EvidenceChannel::Health,
        weight_ppm: 100_000,
        class: EvidenceClass::Observation,
    });
    let error = pass(&h, missing).await.unwrap_err();
    assert!(error.starts_with("BELIEF_PREMISE_MISSING"));
    assert!(error.contains("absent-premise"));

    // One pass over the committed evidence, citing an existing claim as a premise.
    let mut cited = request(&ids);
    cited.premises.push(PremiseRef {
        claim_id: HIGHER.into(),
        premise_id: "premise-claim".into(),
        supports: true,
        channel: EvidenceChannel::Health,
        weight_ppm: 100_000,
        class: EvidenceClass::Observation,
    });
    let result = pass(&h, cited).await.unwrap();
    assert_eq!(
        result.claims_visited, 3,
        "the refusals left no ledger entry"
    );
    let by_claim: BTreeMap<_, _> = result
        .revisions
        .iter()
        .map(|r| (r.claim_id.as_str(), r))
        .collect();
    assert!(by_claim[HIGHER].posterior_micro > 0);
    assert!(by_claim[LOWER].posterior_micro < 0);
    assert!(
        by_claim[HIGHER]
            .support
            .iter()
            .any(|e| e.evidence_id == "premise:premise-claim"),
        "the cited premise is part of the stored explanation"
    );
    assert_eq!(result.friction.len(), 1);
    assert!(result.friction[0].opposes);
    assert_eq!(result.retractions.len(), 1);
    assert_eq!(result.retractions[0].claim_id, "disk-is-fast");
    assert_eq!(result.retractions[0].evidence_class, EvidenceClass::Claim);

    // Proposing never retracts: every claim is byte-equal afterwards.
    for (id, claim_before) in ids.iter().zip(&before) {
        assert_eq!(&claim_of(&h, id).await, claim_before);
    }

    // Another tenant cannot revise the first tenant's claims, and its own pass
    // on a claim of the same name is independent of the first tenant's ledger.
    let foreign = pass_as(&h, OTHER_TENANT, request(&[HIGHER])).await;
    assert!(foreign.unwrap_err().starts_with("BELIEF_CLAIM_NOT_FOUND"));
    let _: ClaimRecord = decode(
        handle_claims(
            &h.state,
            3,
            "default",
            &verified(OTHER_TENANT),
            ClaimOp::Propose {
                claim_id: HIGHER.to_string(),
                perspective: None,
            },
        )
        .await,
    )
    .unwrap();
    let theirs = pass_as(&h, OTHER_TENANT, request(&[HIGHER])).await.unwrap();
    assert!(theirs.revisions.iter().all(|r| r.support.is_empty()));
    assert!(theirs.friction.is_empty() && theirs.retractions.is_empty());
}
