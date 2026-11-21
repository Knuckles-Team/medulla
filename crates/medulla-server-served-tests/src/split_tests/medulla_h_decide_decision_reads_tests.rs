//! The committed-decision reads of the served `DecisionLog`: `List` and
//! `Provenance` (MEDULLA-DECISION-ENGINE-R870.1, R870.2).

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::statistical::log::{
    DecisionLogCommitted, DecisionLogList, DecisionLogOp, DecisionProvenance,
};
use medulla_decision_types::decision::statistical::{
    DecideRequest, DecisionBatch, QuestionSafety, StatisticalDecisionRecord,
};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_h_decide::decide::stat_test_support::{
    decode, publish_route_schema, request, Harness, TENANT,
};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

const OTHER_TENANT: &str = "tenant-other";

async fn dispatch_as(h: &Harness, tenant: &str, method: Method) -> Response {
    let method = rmp_serde::from_slice(&rmp_serde::to_vec_named(&method).unwrap()).unwrap();
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 870,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some("decider".into()),
                method,
            },
            VerifiedRequestContext::verified_for_test_with_scopes(
                "decider",
                tenant,
                &["kg:read", "agent:decision-write", "agent:decision-read"],
            ),
        ),
    )
    .await
}

async fn fixture() -> (Harness, DecideRequest) {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    h.publish_test_component(
        "tool-search",
        AgentComponentKind::Tool,
        "web search engine",
        None::<&()>,
        None,
    )
    .unwrap();
    let (schema_pin, _) = publish_route_schema(&h);
    let asked = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    (h, asked)
}

/// Decide and commit one record for question `n`.
async fn committed(h: &Harness, asked: &DecideRequest, n: usize) -> StatisticalDecisionRecord {
    let mut asked = asked.clone();
    asked.question.question_id = format!("{}-{n}", asked.question.question_id);
    let batch: DecisionBatch = decode(
        dispatch_as(
            h,
            TENANT,
            Method::Decide {
                request: Box::new(asked),
            },
        )
        .await,
    )
    .expect("a served decision");
    let record = batch.records.as_slice()[0].clone();
    let response = dispatch_as(
        h,
        TENANT,
        Method::DecisionLog {
            op: Box::new(DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
                approval: None,
            }),
        },
    )
    .await;
    let _: DecisionLogCommitted = decode(response).expect("the record commits");
    record
}

fn list_op(tenant: &str, limit: u32, cursor: Option<String>) -> DecisionLogOp {
    DecisionLogOp::List {
        tenant_id: tenant.to_string(),
        question_kind: None,
        resolution_kind: None,
        from_ms: None,
        to_ms: None,
        limit,
        cursor,
    }
}

async fn list(h: &Harness, tenant: &str, op: DecisionLogOp) -> Response {
    dispatch_as(h, tenant, Method::DecisionLog { op: Box::new(op) }).await
}

// spec: MEDULLA-DECISION-ENGINE-R870.1
#[tokio::test]
async fn list_pages_filters_and_isolates_tenants() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = fixture().await;
    let mut ids = Vec::new();
    for n in 0..3 {
        ids.push(committed(&h, &asked, n).await.record_id);
    }
    // Pages of two follow the order and the cursor reaches every record once.
    let first: DecisionLogList = decode(list(&h, TENANT, list_op(TENANT, 2, None)).await).unwrap();
    assert_eq!(first.records.len(), 2);
    let cursor = first.next_cursor.clone().expect("a third record remains");
    let second: DecisionLogList =
        decode(list(&h, TENANT, list_op(TENANT, 2, Some(cursor))).await).unwrap();
    assert_eq!(second.records.len(), 1);
    assert!(second.next_cursor.is_none());
    let mut seen: Vec<String> = first
        .records
        .iter()
        .chain(&second.records)
        .map(|item| item.record_id.clone())
        .collect();
    let keys: Vec<(std::cmp::Reverse<u64>, &str)> = first
        .records
        .iter()
        .chain(&second.records)
        .map(|item| (std::cmp::Reverse(item.decided_at_ms), item.record_id.as_str()))
        .collect();
    assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "{keys:?}");
    seen.sort();
    ids.sort();
    assert_eq!(seen, ids);
    // A filter that matches nothing, and a bad range, limit and cursor.
    let none = DecisionLogOp::List {
        tenant_id: TENANT.to_string(),
        question_kind: None,
        resolution_kind: Some(medulla_decision_types::decision::ResolutionKind::Abstention),
        from_ms: None,
        to_ms: None,
        limit: 50,
        cursor: None,
    };
    let none: DecisionLogList = decode(list(&h, TENANT, none).await).unwrap();
    assert!(none.records.iter().all(|item| matches!(
        item.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Abstention
    )));
    for bad in [
        list_op(TENANT, 0, None),
        list_op(TENANT, 201, None),
        list_op(TENANT, 10, Some("not-a-cursor".into())),
        DecisionLogOp::List {
            tenant_id: TENANT.to_string(),
            question_kind: None,
            resolution_kind: None,
            from_ms: Some(10),
            to_ms: Some(5),
            limit: 10,
            cursor: None,
        },
    ] {
        let response = list(&h, TENANT, bad).await;
        let error = response.error.expect("a bad argument refuses");
        assert!(error.contains("INVALID_ARGUMENT"), "{error}");
    }
    // A request naming another tenant is denied; the other tenant sees nothing.
    let denied = list(&h, TENANT, list_op(OTHER_TENANT, 10, None)).await;
    assert!(denied.error.expect("denied").contains("ACCESS_DENIED"));
    let theirs: DecisionLogList =
        decode(list(&h, OTHER_TENANT, list_op(OTHER_TENANT, 10, None)).await).unwrap();
    assert!(theirs.records.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R870.2
#[tokio::test]
async fn provenance_keeps_used_revision_and_hides_other_tenants() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = fixture().await;
    let record = committed(&h, &asked, 0).await;
    let read = |tenant: &'static str, id: String| {
        let h = &h;
        async move {
            dispatch_as(
                h,
                tenant,
                Method::DecisionLog {
                    op: Box::new(DecisionLogOp::Provenance {
                        tenant_id: tenant.to_string(),
                        record_id: id,
                    }),
                },
            )
            .await
        }
    };
    let found: DecisionProvenance = decode(read(TENANT, record.record_id.clone()).await).unwrap();
    assert_eq!(found.record_id, record.record_id);
    assert_eq!(found.record_digest, record.record_digest);
    assert_eq!(found.premises.len(), record.premises.as_slice().len());
    assert_eq!(found.facts.len(), record.inputs.graph_rows.as_slice().len());
    for (fact, row) in found.facts.iter().zip(record.inputs.graph_rows.as_slice()) {
        assert_eq!(fact.node_id, row.node_id);
        assert_eq!(fact.revision, row.graph_version);
    }
    assert_eq!(found.approver, None);
    // An unknown id and another tenant's id refuse with the same body.
    let unknown = read(TENANT, "decision:missing".into()).await.error.unwrap();
    let foreign = read(OTHER_TENANT, record.record_id.clone()).await.error.unwrap();
    assert!(unknown.contains("DECISION_NOT_FOUND"), "{unknown}");
    assert_eq!(unknown, foreign);
}
