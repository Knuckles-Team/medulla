//! The committed-decision reads of the served `DecisionLog`: `List`,
//! `Provenance` and `WhyNot` (MEDULLA-DECISION-ENGINE-R870.1 to R870.4).

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

/// Commit the assembled fixture record of `tenant-a` through the store, the way a
/// served `DecisionCommit` stores it, and return it.
fn commit_assembly(h: &Harness) -> medulla_decision_types::decision::DecisionRecord {
    use medulla_persistence_records::decision_record::tests as v1;
    v1::seed_library(&h.store);
    let record = v1::decided(&h.store).record;
    let digest = record.inputs.catalog_digest.clone();
    h.store
        .commit_decision_record(v1::commit_context(&h.store, "why-not", 1), &record, &digest, None)
        .expect("commits");
    record
}

async fn why_not(
    h: &Harness,
    tenant: &'static str,
    record_id: &str,
    option_id: &str,
    budget_ms: Option<u32>,
) -> Response {
    dispatch_as(
        h,
        tenant,
        Method::DecisionLog {
            op: Box::new(DecisionLogOp::WhyNot {
                tenant_id: tenant.to_string(),
                record_id: record_id.to_string(),
                option_id: option_id.to_string(),
                budget_ms,
            }),
        },
    )
    .await
}

// spec: MEDULLA-DECISION-ENGINE-R870.3
#[tokio::test]
async fn why_not_is_bounded_read_only_and_refuses_retired_inputs() {
    use medulla_decision_types::decision::statistical::log::{DecisionWhyNot, WhyNotVerdict};
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let record = commit_assembly(&h);
    let tenant: &'static str = medulla_persistence_records::decision_record::tests::TENANT;
    let before = h.store.decision_record_body(tenant, &record.record_id).unwrap();
    let medulla_decision_types::decision::DecisionOutcome::Solved { slots, .. } = &record.outcome
    else {
        panic!("the fixture is solved")
    };
    let chosen: Vec<String> = slots
        .iter()
        .map(|slot| slot.component.component_id.clone())
        .collect();
    for candidate in record.inputs.candidates.as_slice() {
        let answer: DecisionWhyNot = decode(
            why_not(&h, tenant, &record.record_id, &candidate.component_id, None).await,
        )
        .expect("a committed assembly re-solves");
        assert_eq!(answer.record_id, record.record_id);
        assert_eq!(answer.option_id, candidate.component_id);
        // The budget is the knob's, never more.
        assert!(answer.budget_used_ms <= 2_000 || answer.budget_exhausted);
        assert_eq!(
            answer.verdict == WhyNotVerdict::Selected,
            chosen.contains(&candidate.component_id),
            "{answer:?}"
        );
        if answer.verdict != WhyNotVerdict::Selected {
            assert!(!answer.reasons.is_empty(), "{answer:?}");
        }
    }
    // An option outside the record's candidates is not visible.
    let unseen = why_not(&h, tenant, &record.record_id, "no-such-component", None).await;
    assert!(unseen.error.unwrap().contains("OPTION_NOT_VISIBLE"));
    // An unknown id and another tenant's id refuse with the same body.
    let unknown = why_not(&h, tenant, "decision:missing", "tool-web", Some(100)).await;
    let foreign = why_not(&h, OTHER_TENANT, &record.record_id, "tool-web", Some(100)).await;
    let unknown = unknown.error.unwrap();
    assert!(unknown.contains("DECISION_NOT_FOUND"), "{unknown}");
    assert_eq!(unknown, foreign.error.unwrap());
    // The call never changes the record.
    assert_eq!(
        h.store.decision_record_body(tenant, &record.record_id).unwrap(),
        before
    );
}

// spec: MEDULLA-DECISION-ENGINE-R870.4
#[tokio::test]
async fn decision_reads_end_to_end() {
    use medulla_decision_types::decision::statistical::log::DecisionWhyNot;
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = fixture().await;
    let listed_record = committed(&h, &asked, 0).await;
    let list_page: DecisionLogList =
        decode(list(&h, TENANT, list_op(TENANT, 10, None)).await).unwrap();
    assert!(list_page
        .records
        .iter()
        .any(|item| item.record_id == listed_record.record_id));
    let provenance: DecisionProvenance = decode(
        dispatch_as(
            &h,
            TENANT,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::Provenance {
                    tenant_id: TENANT.to_string(),
                    record_id: listed_record.record_id.clone(),
                }),
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(provenance.record_digest, listed_record.record_digest);
    for (fact, row) in provenance
        .facts
        .iter()
        .zip(listed_record.inputs.graph_rows.as_slice())
    {
        assert_eq!(fact.revision, row.graph_version);
    }
    // The assembly record of the same tenant: why-not names an exclusion, and a
    // second tenant gets DECISION_NOT_FOUND from each read.
    let assembly = commit_assembly(&h);
    let tenant: &'static str = medulla_persistence_records::decision_record::tests::TENANT;
    let medulla_decision_types::decision::DecisionOutcome::Solved { slots, .. } = &assembly.outcome
    else {
        panic!("the fixture is solved")
    };
    let excluded = assembly
        .inputs
        .candidates
        .as_slice()
        .iter()
        .find(|c| !slots.iter().any(|s| s.component.component_id == c.component_id));
    if let Some(excluded) = excluded {
        let answer: DecisionWhyNot = decode(
            why_not(&h, tenant, &assembly.record_id, &excluded.component_id, None).await,
        )
        .unwrap();
        assert!(!answer.reasons.is_empty(), "{answer:?}");
    }
    for refusal in [
        why_not(&h, OTHER_TENANT, &assembly.record_id, "tool-web", None).await,
        list(&h, OTHER_TENANT, list_op(OTHER_TENANT, 10, None)).await,
    ] {
        // An empty page or a not-found: never the first tenant's data.
        if let Some(error) = refusal.error {
            assert!(error.contains("DECISION_NOT_FOUND"), "{error}");
        } else {
            let page: DecisionLogList = decode(refusal).unwrap();
            assert!(page.records.is_empty());
        }
    }
}
