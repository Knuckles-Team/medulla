//! Certified-definition check of natural-language plans through the served
//! dispatch chain (MEDULLA-FEDERATED-QUERY-R058.6): a candidate that uses a
//! certified glossary term without its lineage goes back to the planner as one
//! typed feedback step, the answer lists the certified terms it used with their
//! version and lineage, and a loop that ends on the mismatch refuses
//! `NL_DEFINITION_CONFLICT`.
//!
//! Every case rides `Method::NlQueryExplain` -> `dispatch_query_family` ->
//! `handle_nl_query_plan` -> `plan_grounded_certified`.
//!
//! The same file proves the certified correction (MEDULLA-FEDERATED-QUERY-R058.7):
//! `Method::NlQueryCorrect` -> router scope check -> graph door -> one durable
//! `NlCorrection` node; the approved pair then rides the planning context of the
//! same question.
#![cfg(all(feature = "server", feature = "security", feature = "nl-query"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::nl_query::{NlCorrection, NlQueryExplainResult};

const SECRET: &str = "nl-query-certified-definitions-secret";
const GRAPH: &str = "__commons__";

const USES_ONE_NAME: &str = "MATCH (:Order) |> WHERE amount_paid IS NOT NULL";
const USES_LINEAGE: &str =
    "MATCH (:Order) |> WHERE amount_paid IS NOT NULL AND amount_refunded IS NOT NULL";

const APPROVED: &str = "MATCH (:Order) |> WHERE amount_refunded IS NOT NULL";

/// A planner that first answers with a query outside the certified definition.
/// It answers the lineage-respecting query only once the hint carries the typed
/// feedback, and never when the question says `stubborn`.
struct DefinitionPlanner;

impl medulla_nlq::nl::NlPlanner for DefinitionPlanner {
    fn id(&self) -> String {
        "scripted".to_string()
    }

    fn revision(&self) -> String {
        "1".to_string()
    }

    fn plan(&self, nl: &str, schema_hint: &str) -> Result<String, String> {
        assert_eq!(
            schema_hint.contains("Certified term `net revenue` (version 3)"),
            nl.contains("net revenue"),
            "the hint carries exactly the certified terms the question uses: {schema_hint}"
        );
        // An approved correction for this question rides the planning context
        // (R058.7); the planner adopts exactly the approved text.
        if let Some(rest) = schema_hint.split("for this question use `").nth(1) {
            let approved = rest.split('`').next().unwrap_or_default();
            assert_eq!(approved, APPROVED, "{schema_hint}");
            return Ok(approved.to_string());
        }
        let corrected = schema_hint.contains("NL_DEFINITION_CONFLICT");
        Ok(if corrected && !nl.contains("stubborn") {
            USES_LINEAGE.to_string()
        } else {
            USES_ONE_NAME.to_string()
        })
    }
}

fn state() -> test_support::SharedState {
    common::configure_authority();
    medulla_h_query::nl::set_nl_planner(Arc::new(DefinitionPlanner));
    test_support::durable_state(SECRET, common::current_isolation())
}

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::commons_request(SECRET, id, method)).await
}

async fn add_node(
    state: &test_support::SharedState,
    id: u64,
    node: &str,
    props: serde_json::Value,
) {
    let response = send(
        state,
        id,
        Method::AddNode {
            node_id: node.to_string(),
            properties_msgpack: test_support::json_bytes(props),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
}

async fn seed(state: &test_support::SharedState) {
    add_node(
        state,
        1,
        "o1",
        serde_json::json!({"type": "Order", "amount_paid": 10, "amount_refunded": 2}),
    )
    .await;
    add_node(
        state,
        2,
        "t1",
        serde_json::json!({"type": "GlossaryTerm", "name": "net revenue",
            "definition": "paid minus refunded", "status": "certified", "version": "3",
            "lineage": ["amount_paid", "amount_refunded"]}),
    )
    .await;
    // A draft term is not certified and is never used.
    add_node(
        state,
        3,
        "t2",
        serde_json::json!({"type": "GlossaryTerm", "name": "gross revenue",
            "definition": "paid", "status": "draft", "version": "1", "lineage": ["amount_paid"]}),
    )
    .await;
}

async fn explain(state: &test_support::SharedState, id: u64, text: &str) -> Response {
    send(
        state,
        id,
        Method::NlQueryExplain {
            text: text.into(),
            graph: GRAPH.into(),
            history: Vec::new(),
        },
    )
    .await
}

fn refusal(resp: &Response) -> String {
    format!("{:?} {:?}", resp.error, resp.error_detail)
}

// spec: MEDULLA-FEDERATED-QUERY-R058.6
#[tokio::test]
async fn mismatch_is_corrected_then_disclosed() {
    let state = state();
    seed(&state).await;

    // One feedback step: the first candidate misses `amount_refunded`, the second
    // honours the definition, and the result lists the term with version and lineage.
    let answered = explain(&state, 10, "what is the net revenue of orders").await;
    assert!(answered.error.is_none(), "{}", refusal(&answered));
    let body: NlQueryExplainResult = match &answered.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected Raw explain result, got {other:?}"),
    };
    assert_eq!(body.uql, USES_LINEAGE);
    assert_eq!(body.certified_terms.len(), 1, "{:?}", body.certified_terms);
    let used = &body.certified_terms[0];
    assert_eq!(used.term, "net revenue");
    assert_eq!(used.version, "3");
    assert_eq!(used.lineage, vec!["amount_paid", "amount_refunded"]);

    // A question that uses no certified term is planned as before and lists none.
    let plain = explain(&state, 11, "which orders have a paid amount").await;
    assert!(plain.error.is_none(), "{}", refusal(&plain));

    // The loop ends on the mismatch: the code, and every step's text.
    let stubborn = explain(&state, 12, "stubborn net revenue of orders").await;
    let text = refusal(&stubborn);
    assert!(text.contains("NL_DEFINITION_CONFLICT"), "{text}");
    assert!(text.contains("step 1") && text.contains("step 3"), "{text}");
    assert!(text.contains("amount_refunded"), "{text}");
    assert!(stubborn.result.is_none(), "a refused plan returns no rows");
}

async fn correct(
    state: &test_support::SharedState,
    id: u64,
    scopes: &[&str],
    text: &str,
    approved: &str,
) -> Response {
    let method = Method::NlQueryCorrect {
        text: text.into(),
        graph: GRAPH.into(),
        rejected_uql: USES_ONE_NAME.into(),
        approved_uql: approved.into(),
    };
    test_support::dispatch(
        state,
        common::signed_request_with_scopes(SECRET, id, GRAPH, scopes, method),
    )
    .await
}

fn explained(resp: &Response) -> NlQueryExplainResult {
    assert!(resp.error.is_none(), "{}", refusal(resp));
    match &resp.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected Raw explain result, got {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R058.7
#[tokio::test]
async fn recorded_correction_grounds_the_next_plan() {
    let state = state();
    seed(&state).await;
    const QUESTION: &str = "which orders were paid late";
    const REWORDED: &str = "Late, paid orders: which were?";
    const CERTIFIER: &[&str] = &["kg:read", "kg:write", "query:nl:certify"];

    // Before any correction the planner answers with the rejected query.
    let before = explained(&explain(&state, 20, QUESTION).await);
    assert_eq!(before.uql, USES_ONE_NAME);

    // Without the certify scope: the typed refusal, and nothing is written.
    let denied = correct(&state, 21, &["kg:read", "kg:write"], QUESTION, APPROVED).await;
    assert!(
        refusal(&denied).contains("NL_CORRECTION_UNAUTHORIZED"),
        "{}",
        refusal(&denied)
    );
    assert!(denied.result.is_none());
    let still = explained(&explain(&state, 22, QUESTION).await);
    assert_eq!(
        still.uql, USES_ONE_NAME,
        "a refused correction writes nothing"
    );

    // An approved query that does not parse is refused and writes nothing.
    let bad = correct(&state, 23, CERTIFIER, QUESTION, "MATCH (((").await;
    assert!(
        refusal(&bad).contains("INVALID_ARGUMENT"),
        "{}",
        refusal(&bad)
    );

    // The certifier records the correction: one record under the caller's authority.
    let recorded = correct(&state, 24, CERTIFIER, QUESTION, APPROVED).await;
    assert!(recorded.error.is_none(), "{}", refusal(&recorded));
    let record: NlCorrection = match &recorded.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected Raw correction, got {other:?}"),
    };
    assert_eq!(record.question, QUESTION);
    assert_eq!(record.rejected_uql, USES_ONE_NAME);
    assert_eq!(record.approved_uql, APPROVED);
    assert!(!record.certifier.is_empty());

    // The same question, and the same words in another order, now plan with the
    // approved pair in their context; a different question does not.
    for (id, text) in [(25, QUESTION), (26, REWORDED)] {
        let grounded = explained(&explain(&state, id, text).await);
        assert_eq!(grounded.uql, APPROVED, "{text}");
    }
    let other = explained(&explain(&state, 27, "which orders were refunded").await);
    assert_eq!(other.uql, USES_ONE_NAME);

    // Recording the same correction again returns the first record.
    let again = correct(&state, 28, CERTIFIER, QUESTION, APPROVED).await;
    let again: NlCorrection = match &again.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected Raw correction, got {other:?}"),
    };
    assert_eq!(again, record);

    // A correction never changes a certified definition.
    let certified = explained(&explain(&state, 29, "what is the net revenue of orders").await);
    assert_eq!(certified.uql, USES_LINEAGE);
    assert_eq!(certified.certified_terms.len(), 1);
    assert_eq!(certified.certified_terms[0].version, "3");
}

fn pack_term(source: &str, definition: &str) -> serde_json::Value {
    serde_json::json!({"type": "GlossaryTerm", "name": "churn rate", "source": source,
        "definition": definition, "status": "certified", "version": "1",
        "lineage": ["amount_paid"]})
}

// spec: MEDULLA-FEDERATED-QUERY-R058.9
#[tokio::test]
async fn conflicting_sources_record_one_conflict() {
    let state = state();
    seed(&state).await;
    let first = pack_term("pack-a", "customers lost in the month");
    let second = pack_term("pack-b", "revenue lost in the month");
    add_node(&state, 30, "churn-a", first.clone()).await;

    // One source defines the term: certified, nothing to reconcile.
    let core = medulla_graph_core::graph::GraphCore::new();
    let blob = |value: &serde_json::Value| rmp_serde::to_vec_named(value).unwrap();
    core.add_node("churn-a".into(), blob(&first));
    assert!(medulla_nlq::nl::certified::reconcile_term_definitions(&core.analysis_snapshot())
        .is_empty());

    // A second source defines it differently: exactly one conflict fact links both.
    add_node(&state, 31, "churn-b", second.clone()).await;
    core.add_node("churn-b".into(), blob(&second));
    let conflicts =
        medulla_nlq::nl::certified::reconcile_term_definitions(&core.analysis_snapshot());
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    let conflict = &conflicts[0];
    assert_eq!((conflict.left_id.as_str(), conflict.right_id.as_str()), ("churn-a", "churn-b"));
    assert_ne!(conflict.left_definition, conflict.right_definition);

    // Before the fact is recorded the term is still certified.
    let before = explained(&explain(&state, 32, "what is the churn rate of orders").await);
    assert_eq!(before.certified_terms.len(), 1, "{:?}", before.certified_terms);

    // Recording it makes the term uncertified for planning; recording the pair again is
    // the same node, and the reconcile no longer reports it.
    add_node(&state, 33, &conflict.node_id(), conflict.properties()).await;
    let after = explained(&explain(&state, 34, "what is the churn rate of orders").await);
    assert!(after.certified_terms.is_empty(), "{:?}", after.certified_terms);
    core.add_node(conflict.node_id(), blob(&conflict.properties()));
    assert!(medulla_nlq::nl::certified::reconcile_term_definitions(&core.analysis_snapshot())
        .is_empty());
}
