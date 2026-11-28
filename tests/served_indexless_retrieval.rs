//! MEDULLA-DECISION-ENGINE-R144: the served indexless retrieval through the real
//! `dispatch` shell. Raw reads, an id-ordered corpus scan and span digest checks run
//! over the `Document` nodes the caller may see, under a budget clamped to the
//! tenant ceiling, and every answer carries the cost report: an accepted one in its
//! body, a refused one in the refusal detail.
#![cfg(feature = "query")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use medulla_agent_types::agent_component::{
    AgentComponentKind, AgentComponentPublishRequest, ComponentDependency, DecisionPolicyRef,
};
use medulla_decision_program_types::decision::knob_families::indexless::MAX_ROWS;
use medulla_decision_statistical_types::decision::digest::policy_part::policy_digest;
use medulla_decision_statistical_types::decision::knobs::KnobValue;
use medulla_decision_statistical_types::decision::policy::DECISION_POLICY_ATTRIBUTE;
use medulla_decision_statistical_types::decision::DecisionPolicy;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::indexless_cost::{CostReport, IndexlessBudget};
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_program_types::decision::indexless_retrieval::IndexlessOp,
    medulla_decision_program_types::decision::indexless_retrieval::IndexlessRequest,
    medulla_decision_program_types::decision::indexless_retrieval::IndexlessResult,
};

const SECRET: &str = "indexless-retrieval-served-secret";
const GRAPH: &str = "__commons__";
/// Visible `Document` nodes the corpus holds.
const CORPUS: u64 = 200;

fn state() -> Arc<RwLock<ServerState>> {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn add(state: &Arc<RwLock<ServerState>>, id: u64, node: &str, props: serde_json::Value) {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: test_support::json_bytes(props),
            },
        ),
    )
    .await;
    assert!(
        response.error.is_none(),
        "seed failed: {:?}",
        response.error
    );
}

fn doc_id(n: u64) -> String {
    format!("doc-{n:03}")
}

fn doc_text(n: u64) -> String {
    format!("document {n} caf\u{e9}")
}

/// `CORPUS` visible documents in id order, then `extra` under `doc-hidden`, then a
/// Note that sorts after every document.
async fn seed(state: &Arc<RwLock<ServerState>>, extra: serde_json::Value) {
    for n in 0..CORPUS {
        add(
            state,
            n + 1,
            &doc_id(n),
            serde_json::json!({ "type": "Document", "text": doc_text(n) }),
        )
        .await;
    }
    add(state, 500, "doc-hidden", extra).await;
    add(
        state,
        501,
        "zz-note",
        serde_json::json!({ "type": "Note", "text": "a note" }),
    )
    .await;
}

/// The node another tenant owns: a caller of the test tenant may not see it.
fn foreign_document() -> serde_json::Value {
    serde_json::json!({
        "type": "Document",
        "text": "another tenant's text",
        "_tenant": "tenant-other",
    })
}

fn budget(max_rows: u64, max_visits: u64, max_cost_units: u64) -> IndexlessBudget {
    IndexlessBudget {
        max_rows,
        max_visits,
        max_cost_units,
    }
}

fn request(ops: Vec<IndexlessOp>, budget: Option<IndexlessBudget>) -> IndexlessRequest {
    IndexlessRequest {
        tenant_id: common::TEST_TENANT.to_string(),
        graph: GRAPH.to_string(),
        ops: BoundedVec::new(ops).expect("at most 64 ops"),
        budget,
        policy: DecisionPolicyRef::Default,
    }
}

fn scan(limit: u64) -> IndexlessOp {
    IndexlessOp::CorpusScan { limit }
}

fn raw(document_id: &str) -> IndexlessOp {
    IndexlessOp::RawRead {
        document_id: document_id.to_string(),
    }
}

async fn retrieve(state: &Arc<RwLock<ServerState>>, request: IndexlessRequest) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            900,
            Method::IndexlessRetrieve {
                request: Box::new(request),
            },
        ),
    )
    .await
}

fn decode(response: Response) -> IndexlessResult {
    assert!(
        response.error.is_none(),
        "retrieval failed: {:?} {:?}",
        response.error,
        response.error_detail
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

/// The refusal code, its detail and the cost report the detail carries.
fn refused(response: Response) -> (String, String, Option<CostReport>) {
    let code = response.error.expect("the retrieval must refuse");
    let detail = response.error_detail.unwrap_or_default();
    let report = detail
        .split_once("; cost ")
        .map(|(_, json)| serde_json::from_str(json).expect("a cost report"));
    (code, detail, report)
}

fn row_ids(result: &IndexlessResult) -> Vec<String> {
    result
        .rows
        .iter()
        .map(|row| {
            row["document_id"]
                .as_str()
                .expect("a document id")
                .to_string()
        })
        .collect()
}

/// Publish `policy` as a `DecisionPolicy` component of the test tenant and return its pin.
async fn publish_policy(
    state: &Arc<RwLock<ServerState>>,
    id: &str,
    policy: &DecisionPolicy,
) -> ComponentDependency {
    let store = state.write().await.ensure_agent_library().expect("library");
    let mut draft = test_component_draft(common::TEST_TENANT, id);
    draft.kind = AgentComponentKind::DecisionPolicy;
    draft.content_digest = policy_digest(policy);
    draft.attributes.insert(
        DECISION_POLICY_ATTRIBUTE.to_string(),
        serde_json::to_string(policy).expect("policy json"),
    );
    let request = AgentComponentPublishRequest {
        context: mutation_context(
            &store,
            common::TEST_TENANT,
            &format!("publish-{id}"),
            1,
            0,
            "agent-component:publish",
        ),
        component: draft,
        evaluation_receipt_digest: None,
        body: None,
    };
    let committed = store
        .publish_component(request, None)
        .expect("publish policy");
    ComponentDependency {
        component_id: id.to_string(),
        kind: AgentComponentKind::DecisionPolicy,
        definition_digest: committed.result.component.definition_digest,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R144.1
// spec: MEDULLA-DECISION-ENGINE-R144.2
#[tokio::test(flavor = "multi_thread")]
async fn indexless_retrieval_reports_cost_and_refuses_over_budget() {
    let state = state();
    seed(&state, foreign_document()).await;
    let limit = budget(100, 1_000, 1_000);

    // A scan of 50 rows under max_rows 100 succeeds and reports what it spent.
    let fifty = decode(retrieve(&state, request(vec![scan(50)], Some(limit))).await);
    assert_eq!(row_ids(&fifty), (0..50).map(doc_id).collect::<Vec<_>>());
    assert_eq!(fifty.cost.rows_visited, 50);
    assert_eq!(fifty.cost.visits, 50);
    assert_eq!(
        fifty.cost.cost_used, 50,
        "indexless.scan_cost defaults to 1"
    );
    assert_eq!(fifty.cost.remaining, budget(50, 950, 950));
    assert_eq!(fifty.cost.applied_budget, limit);

    // A scan of 101 rows refuses, and the meter is reported unchanged.
    let (code, detail, report) =
        refused(retrieve(&state, request(vec![scan(101)], Some(limit))).await);
    assert_eq!(code, "INDEXLESS_BUDGET_EXHAUSTED");
    assert!(detail.starts_with("operation 0:corpus_scan"), "{detail}");
    let report = report.expect("the refusal carries the cost report");
    assert_eq!(report.rows_visited, 0);
    assert_eq!(report.remaining, limit);
    assert!(report.operation_ids.is_empty());

    // An empty op list refuses with its stable code.
    let (code, _, _) = refused(retrieve(&state, request(Vec::new(), Some(limit))).await);
    assert_eq!(code, "INDEXLESS_OPS_EMPTY");

    // A span off a character boundary, or with another digest, refuses.
    let text = doc_text(7);
    let start = text.find('\u{e9}').expect("an accented letter") as u64;
    let span = |end: u64, digest: String| IndexlessOp::VerifySpan {
        document_id: doc_id(7),
        start_byte: start,
        end_byte: end,
        expected_digest: digest,
    };
    let good = hex::encode(Sha256::digest("\u{e9}".as_bytes()));
    decode(retrieve(&state, request(vec![span(start + 2, good.clone())], None)).await);
    let (code, _, _) = refused(retrieve(&state, request(vec![span(start + 1, good)], None)).await);
    assert_eq!(code, "INDEXLESS_SPAN_INVALID");
    let (code, _, _) =
        refused(retrieve(&state, request(vec![span(start + 2, "0".repeat(64))], None)).await);
    assert_eq!(code, "INDEXLESS_SPAN_INVALID");
}

// spec: MEDULLA-DECISION-ENGINE-R144.3
#[tokio::test(flavor = "multi_thread")]
async fn indexless_routes_cost_report() {
    let state = state();
    seed(&state, foreign_document()).await;
    let text = doc_text(0);
    let ops = vec![
        raw(&doc_id(0)),
        scan(3),
        IndexlessOp::VerifySpan {
            document_id: doc_id(0),
            start_byte: 0,
            end_byte: text.len() as u64,
            expected_digest: hex::encode(Sha256::digest(text.as_bytes())),
        },
    ];
    let limit = budget(100, 100, 100);
    let result = decode(retrieve(&state, request(ops, Some(limit))).await);
    assert_eq!(result.rows.len(), 5);
    assert_eq!(result.rows[0]["text"], text);
    assert_eq!(result.rows[0]["operation_id"], "0:raw_read");
    assert_eq!(
        row_ids(&result)[1..4].to_vec(),
        [doc_id(0), doc_id(1), doc_id(2)]
    );
    assert_eq!(result.rows[4]["verified"], true);
    assert_eq!(result.rows[4]["operation_id"], "2:verify_span");
    // 1 + 3 + 1 rows and visits; read 1 + scan 3 x 1 + verify 2 cost units.
    assert_eq!(result.cost.rows_visited, 5);
    assert_eq!(result.cost.visits, 5);
    assert_eq!(result.cost.cost_used, 6);
    assert_eq!(result.cost.remaining, budget(95, 95, 94));
    assert_eq!(
        result.cost.operation_ids.as_slice().to_vec(),
        ["0:raw_read", "1:corpus_scan", "2:verify_span"]
    );
    assert!(result.cost.authorization_revision > 0);

    // A full scan returns the caller's documents only: the other tenant's is absent.
    let all = decode(retrieve(&state, request(vec![scan(1_000)], None)).await);
    assert_eq!(all.rows.len() as u64, CORPUS);
    assert!(!row_ids(&all).contains(&"doc-hidden".to_string()));

    // Cross-tenant negative: a request for another tenant refuses before any read.
    let mut foreign = request(vec![scan(1)], None);
    foreign.tenant_id = "tenant-other".to_string();
    let (code, detail, report) = refused(retrieve(&state, foreign).await);
    assert_eq!(code, "ACCESS_DENIED");
    assert!(report.is_none(), "nothing was read: {detail}");
}

// spec: MEDULLA-DECISION-ENGINE-R144.4
#[tokio::test(flavor = "multi_thread")]
async fn budget_exhaustion_refuses() {
    // World A holds another tenant's Document; world B a plain Note in its place,
    // so both worlds took the same writes.
    let (a, b) = (state(), state());
    seed(&a, foreign_document()).await;
    seed(
        &b,
        serde_json::json!({ "type": "Note", "text": "another tenant's text" }),
    )
    .await;

    // Exhaustion: the second scan finds the rows dimension spent.
    let limit = budget(100, 1_000, 1_000);
    let (code, detail, report) =
        refused(retrieve(&a, request(vec![scan(100), scan(1)], Some(limit))).await);
    assert_eq!(code, "INDEXLESS_BUDGET_EXHAUSTED");
    assert!(
        detail.starts_with("operation 1:corpus_scan"),
        "the detail names the first refused operation: {detail}"
    );
    let report = report.expect("a cost report");
    assert_eq!(report.rows_visited, 100);
    assert_eq!(
        report.remaining.max_rows, 0,
        "zero in the exhausted dimension"
    );
    assert_eq!(report.remaining.max_visits, 900);
    assert_eq!(report.operation_ids.as_slice().to_vec(), ["0:corpus_scan"]);

    // A denied document answers exactly as an absent one.
    for op in [
        raw("doc-hidden"),
        IndexlessOp::VerifySpan {
            document_id: "doc-hidden".to_string(),
            start_byte: 0,
            end_byte: 1,
            expected_digest: "0".repeat(64),
        },
    ] {
        let in_a = retrieve(&a, request(vec![op.clone()], None)).await;
        let in_b = retrieve(&b, request(vec![op], None)).await;
        assert_eq!(in_a.error.as_deref(), Some("DOCUMENT_NOT_FOUND"));
        assert_eq!(in_a.error, in_b.error);
        assert_eq!(in_a.error_detail, in_b.error_detail);
    }
    let missing = retrieve(&a, request(vec![raw("doc-never")], None)).await;
    assert_eq!(missing.error.as_deref(), Some("DOCUMENT_NOT_FOUND"));

    // An unauthorized graph refuses before any read: no cost report, no rows.
    let mut elsewhere = request(vec![scan(1)], None);
    elsewhere.graph = "another-graph".to_string();
    let (code, detail, report) = refused(retrieve(&a, elsewhere).await);
    assert_eq!(code, "ACCESS_DENIED");
    assert!(report.is_none(), "zero reads: {detail}");

    // Cross-tenant negative: another tenant's request is refused the same way.
    let mut foreign = request(vec![raw(&doc_id(0))], None);
    foreign.tenant_id = "tenant-other".to_string();
    let (code, _, report) = refused(retrieve(&a, foreign).await);
    assert_eq!(code, "ACCESS_DENIED");
    assert!(report.is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R144.5
#[tokio::test(flavor = "multi_thread")]
async fn cost_reports_bind_budget() {
    let state = state();
    seed(&state, foreign_document()).await;
    let limit = budget(500, 500, 500);
    let ops = || vec![scan(10), raw(&doc_id(42))];

    let first = decode(retrieve(&state, request(ops(), Some(limit))).await);
    assert_eq!(first.cost.applied_budget, limit);
    assert_eq!(
        first.cost.operation_ids.as_slice().to_vec(),
        ["0:corpus_scan", "1:raw_read"]
    );
    assert_eq!(first.cost.rows_visited, 11);
    assert_eq!(first.cost.visits, 11);
    assert_eq!(first.cost.cost_used, 11);
    assert_eq!(first.cost.remaining, budget(489, 489, 489));

    // The authorization revision is the snapshot read: stable without writes,
    // newer after one.
    let again = decode(retrieve(&state, request(ops(), Some(limit))).await);
    assert_eq!(again.cost, first.cost);
    add(
        &state,
        902,
        "doc-late",
        serde_json::json!({ "type": "Document", "text": "late" }),
    )
    .await;
    let later = decode(retrieve(&state, request(ops(), Some(limit))).await);
    assert!(later.cost.authorization_revision > first.cost.authorization_revision);

    // A refused response carries the same report fields.
    let (_, _, report) = refused(
        retrieve(
            &state,
            request(vec![scan(10), raw("doc-hidden")], Some(limit)),
        )
        .await,
    );
    let report = report.expect("a cost report");
    assert_eq!(report.applied_budget, limit);
    assert_eq!(report.operation_ids.as_slice().to_vec(), ["0:corpus_scan"]);
    assert_eq!(report.rows_visited, 10);

    // Cross-tenant negative: the other tenant's document is not counted.
    let all = decode(retrieve(&state, request(vec![scan(10_000)], None)).await);
    assert_eq!(all.cost.rows_visited, CORPUS + 1, "the corpus and doc-late");
}

// spec: MEDULLA-DECISION-ENGINE-R144.7
#[tokio::test(flavor = "multi_thread")]
async fn consumer_budget_clamped_to_tenant_ceiling() {
    let state = state();
    seed(&state, foreign_document()).await;
    let ceiling = budget(10_000, 100_000, 1_000_000);

    // An over-ceiling budget is clamped field-wise, never refused.
    let asked = budget(20_000, 50, 2_000_000);
    let clamped = decode(retrieve(&state, request(vec![scan(5)], Some(asked))).await);
    assert_eq!(clamped.cost.applied_budget, budget(10_000, 50, 1_000_000));
    assert_eq!(
        clamped.cost.premises,
        [
            "knob:indexless.max_rows=10000",
            "knob:indexless.max_visits=100000",
            "knob:indexless.max_cost_units=1000000",
        ]
    );

    // No budget applies the ceiling.
    let open = decode(retrieve(&state, request(vec![scan(5)], None)).await);
    assert_eq!(open.cost.applied_budget, ceiling);

    // A pinned tenant policy lowers the ceiling the request is clamped to.
    let mut policy = DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(MAX_ROWS.knob_name(), KnobValue::Integer(150));
    let pin = publish_policy(&state, "policy-indexless", &policy).await;
    let mut pinned = request(vec![scan(150)], Some(budget(1_000, 1_000, 1_000)));
    pinned.policy = DecisionPolicyRef::Pinned {
        component: pin.clone(),
    };
    let under_pin = decode(retrieve(&state, pinned.clone()).await);
    assert_eq!(under_pin.cost.applied_budget, budget(150, 1_000, 1_000));
    assert_eq!(under_pin.cost.premises[0], "knob:indexless.max_rows=150");
    assert_eq!(under_pin.rows.len(), 150);
    pinned.ops = BoundedVec::new(vec![scan(151)]).expect("one op");
    let (code, _, _) = refused(retrieve(&state, pinned).await);
    assert_eq!(code, "INDEXLESS_BUDGET_EXHAUSTED");

    // Cross-tenant negative: another tenant cannot borrow this tenant's ceiling.
    let mut foreign = request(vec![scan(5)], None);
    foreign.tenant_id = "tenant-other".to_string();
    let (code, _, _) = refused(retrieve(&state, foreign).await);
    assert_eq!(code, "ACCESS_DENIED");
}
