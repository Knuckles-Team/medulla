//! The served human-correction journey: a verified `corrector` records a
//! correction, the router commits its four nodes, three edges and vector as one
//! batch, `Nearest` finds it, a self-reported correction is refused and another
//! tenant never sees it (MEDULLA-DECISION-ENGINE-R702.6).

use medulla_decision_types::correction::{
    Correction, CorrectionNearest, CorrectionOp, CorrectionRead, CorrectionRecorded,
    CorrectionSource,
};
use medulla_h_decide::decide::stat_test_support::{decode, Harness, TENANT};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

const GRAPH: &str = "__commons__";

async fn dispatch(h: &Harness, tenant: &str, roles: &[&str], op: CorrectionOp) -> Response {
    let method = Method::CorrectionRecord { op: Box::new(op) };
    let method = rmp_serde::from_slice(&rmp_serde::to_vec_named(&method).unwrap()).unwrap();
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 702,
                graph: GRAPH.into(),
                auth_token: String::new(),
                agent_id: Some("decider".into()),
                method,
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_roles("decider", tenant, roles),
        ),
    )
    .await
}

fn correction(id: &str, after: &str) -> Correction {
    Correction {
        correction_id: id.into(),
        target: "answer:7".into(),
        before: "the wrong answer".into(),
        after: after.into(),
        produced_by: "agent:planner".into(),
        actor: "user:ada".into(),
        at_ms: 1_700_000_000_000,
        source: CorrectionSource::Human,
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.6: record, nearest and a self-reported
/// record through the router; the commit is atomic and cross-tenant reads find
/// nothing.
// spec: MEDULLA-DECISION-ENGINE-R702.6
#[tokio::test]
async fn correction_commits_three_effects_atomically() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let record = |c: Correction| CorrectionOp::Record { correction: c };

    let recorded: CorrectionRecorded = decode(
        dispatch(&h, TENANT, &["corrector"], record(correction("c1", "right"))).await,
    )
    .unwrap();
    let receipt = recorded.receipt_id.clone();
    {
        let guard = h.state.read().await;
        let core = &guard.registry.get(GRAPH).unwrap().core;
        for prefix in ["correction", "rule", "outcome", "case"] {
            assert!(
                core.get_node_properties(&format!("{prefix}:{receipt}")).is_some(),
                "{prefix} node is committed"
            );
        }
        assert!(core
            .semantic_store
            .read()
            .get_embedding(&format!("correction:{receipt}"))
            .is_some());
    }

    // Another tenant's correction of the same target does not answer `Nearest`.
    let _: CorrectionRecorded = decode(
        dispatch(&h, "tenant-other", &["corrector"], record(correction("c2", "other"))).await,
    )
    .unwrap();
    let nearest: CorrectionNearest = decode(
        dispatch(
            &h,
            TENANT,
            &["corrector"],
            CorrectionOp::Nearest { target: "answer:7".into() },
        )
        .await,
    )
    .unwrap();
    assert_eq!(nearest.node_ids, vec![format!("correction:{receipt}")]);

    // A read by receipt under a read-only principal returns all of it.
    let read: CorrectionRead = decode(
        dispatch(
            &h,
            TENANT,
            &["test"],
            CorrectionOp::Get { receipt_id: receipt.clone() },
        )
        .await,
    )
    .unwrap();
    assert_eq!(read.effect, recorded.effect);

    // Refusals commit nothing: a self-reported correction, and a caller without
    // the role.
    let mut own = correction("c3", "right");
    own.produced_by = own.actor.clone();
    let refused = dispatch(&h, TENANT, &["corrector"], record(own)).await;
    assert!(refused.refusal_text().unwrap().starts_with("OUTCOME_SELF_REPORTED"));
    let unprivileged =
        dispatch(&h, TENANT, &["test"], record(correction("c4", "right"))).await;
    assert!(unprivileged
        .refusal_text()
        .unwrap()
        .starts_with("CORRECTION_ROLE_REQUIRED"));

    // The foreign tenant reads the receipt as absent.
    let foreign = dispatch(
        &h,
        "tenant-other",
        &["corrector"],
        CorrectionOp::Get { receipt_id: receipt },
    )
    .await;
    assert!(foreign.refusal_text().unwrap().starts_with("CORRECTION_NOT_FOUND"));
}
