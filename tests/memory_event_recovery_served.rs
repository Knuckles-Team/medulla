//! MEDULLA-DECISION-ENGINE-R073.6: a claim transition whose event was never published (the process
//! stopped after the claim store committed) is published by the recovery pass the leader tick runs.
//! The claim is served straight through the handler, which commits the claim and its outbox row but
//! leaves the publish to the caller, so the row is exactly what a crash leaves behind. One pass
//! writes one `MemoryEvent` node, a second pass finds nothing pending, and the pass never writes a
//! second node for the same row.

#![cfg(all(feature = "redb", feature = "jobs", feature = "statechart"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;

use medulla_h_jobs::memory_event_publisher::publish_claim_events;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_work_types::participant::ParticipantId;
use medulla_work_types::participant::{Participant, ParticipantKind};
use {
    medulla_decision_statistical_types::claims::ClaimOp,
    medulla_decision_statistical_types::claims::ClaimPerspective,
    medulla_decision_statistical_types::claims::ClaimState,
    medulla_decision_statistical_types::claims::EvidenceChannel,
    medulla_decision_statistical_types::claims::ObserverRef,
};

const SECRET: &str = "memory-event-recovery-secret";
const ENCRYPTION_KEY: &str = "memory-event-recovery-encryption-key-0123456789";
const GRAPH: &str = "recoverygraph";

fn slug(value: &str) -> ParticipantId {
    ParticipantId::new(value).expect("a valid slug")
}

async fn memory_event_nodes(state: &test_support::SharedState) -> Vec<String> {
    let core = {
        let current = state.read().await;
        current.registry.get(GRAPH).expect("graph").core.clone()
    };
    core.nodes_by_property("type", "MemoryEvent")
        .unwrap_or_default()
}

// spec: MEDULLA-DECISION-ENGINE-R073.6
#[tokio::test(flavor = "multi_thread")]
async fn an_unpublished_claim_event_is_published_once_by_the_recovery_pass() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-event-recovery",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let tenant = common::TEST_TENANT;
    let core = {
        let current = state.read().await;
        current.registry.get(GRAPH).expect("graph").core.clone()
    };
    let participant = Participant::new(
        "ada",
        tenant,
        "ws-main",
        ParticipantKind::Person,
        "ada",
        Some("person:ada"),
        vec!["claim-writer".to_string()],
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

    let verified = VerifiedRequestContext::verified_for_test_in_tenant("claim-writer", tenant);
    let serve = |op: ClaimOp| {
        let (state, verified) = (state.clone(), verified.clone());
        async move {
            let served = medulla_h_decide::decide::claims::handle_claims_planned(
                &state, 1, GRAPH, &verified, op,
            )
            .await;
            assert!(
                served.response.refusal_text().is_none(),
                "{:?}",
                served.response.refusal_text()
            );
            served
        }
    };
    serve(ClaimOp::Propose {
        claim_id: "c-recover".to_string(),
        perspective: Some(ClaimPerspective {
            subject: slug("ada"),
            observer: ObserverRef::System,
            statement: None,
        }),
    })
    .await;
    serve(ClaimOp::RecordEvidence {
        evidence_id: "e1".to_string(),
        claim_id: "c-recover".to_string(),
        channel: Some(EvidenceChannel::Outcome),
        supports: true,
        weight_ppm: 900_000,
        source_digest: "sha256:e1".to_string(),
        session_id: None,
        stances: BTreeMap::new(),
        feed: None,
        proof: None,
    })
    .await;
    let served = serve(ClaimOp::Advance {
        claim_id: "c-recover".to_string(),
        to: ClaimState::Supported,
    })
    .await;
    assert_eq!(served.events.len(), 1, "the transition announced one event");

    // The process stopped before anything published it: the graph holds no event yet.
    assert!(memory_event_nodes(&state).await.is_empty());

    // The recovery pass publishes the row once.
    assert_eq!(
        publish_claim_events(&state)
            .await
            .expect("the pass publishes"),
        1
    );
    let nodes = memory_event_nodes(&state).await;
    assert_eq!(nodes.len(), 1);
    let properties = core.get_node_properties(&nodes[0]).expect("the event node");
    let properties: serde_json::Value = rmp_serde::from_slice(&properties).unwrap();
    assert_eq!(properties["topic"], "memory.claim.changed");
    assert_eq!(properties["tenant_scope"], tenant);

    // A second pass finds the row acknowledged and writes nothing.
    assert_eq!(
        publish_claim_events(&state)
            .await
            .expect("the pass is idempotent"),
        0
    );
    assert_eq!(memory_event_nodes(&state).await, nodes);
}
