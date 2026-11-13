//! Bounded NL planning through the served dispatch chain
//! (MEDULLA-FEDERATED-QUERY-R058.8): a follow-up question with earlier turns is
//! rewritten to one standalone question before planning, and the rewrite is
//! disclosed by `NlQueryPlan`; without history the question plans unchanged.
#![cfg(all(feature = "server", feature = "security", feature = "nl-query"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::nl_query::{NlDialect, NlQueryPlanResult, NlTurn};
use medulla_types::wire::UqlResult;

const SECRET: &str = "nl-query-bounded-steps-secret";
const GRAPH: &str = "__commons__";

/// Resolves "them" against the earlier turn when asked to rewrite; otherwise
/// echoes the question back as the UQL to plan.
struct ScriptedPlanner;

impl medulla_nlq::nl::NlPlanner for ScriptedPlanner {
    fn plan(&self, nl: &str, schema_hint: &str) -> Result<String, String> {
        if schema_hint.starts_with("Rewrite the question") {
            assert!(
                schema_hint.contains("Which documents exist?"),
                "{schema_hint}"
            );
            assert_eq!(nl, "show two of them");
            return Ok("MATCH (:Doc) |> LIMIT 2".to_string());
        }
        // A first answer that does not parse; the corrected one only after the
        // engine sent the typed refusal back (MEDULLA-FEDERATED-QUERY-R058.4.1).
        if nl == "two documents, corrected" {
            return Ok(if schema_hint.contains("A previous answer was refused") {
                "MATCH (:Doc) |> LIMIT 2".to_string()
            } else {
                "MATCH (:Doc".to_string()
            });
        }
        Ok(nl.to_string())
    }
}

fn state() -> test_support::SharedState {
    common::configure_authority();
    test_support::durable_state(SECRET, common::current_isolation())
}

async fn plan(
    state: &test_support::SharedState,
    id: u64,
    text: &str,
    history: Vec<NlTurn>,
) -> Response {
    let method = Method::NlQueryPlan {
        text: text.into(),
        graph: GRAPH.into(),
        history,
    };
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn plan_body(resp: &Response) -> NlQueryPlanResult {
    match &resp.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected Raw plan result, got {other:?} / {:?}", resp.error),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R058.8
#[tokio::test]
async fn follow_up_is_rewritten_and_disclosed() {
    medulla_h_query::nl::set_nl_planner(Arc::new(ScriptedPlanner));
    let state = state();

    let history = vec![NlTurn {
        question: "Which documents exist?".into(),
        answer_summary: Some("Three documents.".into()),
    }];
    let rewritten = plan(&state, 1, "show two of them", history).await;
    let body = plan_body(&rewritten);
    assert_eq!(
        body.rewritten_question.as_deref(),
        Some("MATCH (:Doc) |> LIMIT 2")
    );
    assert_eq!(body.uql, "MATCH (:Doc) |> LIMIT 2");

    let plain = plan(&state, 2, "MATCH (:Doc) |> LIMIT 1", Vec::new()).await;
    let body = plan_body(&plain);
    assert_eq!(body.rewritten_question, None);
    assert_eq!(body.uql, "MATCH (:Doc) |> LIMIT 1");

    let too_many = vec![
        NlTurn {
            question: "q".into(),
            answer_summary: None,
        };
        5
    ];
    let refused = plan(&state, 3, "show two of them", too_many).await;
    let error = format!("{:?} {:?}", refused.error, refused.error_detail);
    assert!(error.contains("INVALID_ARGUMENT"), "{error}");
    assert!(error.contains("max_history_turns"), "{error}");
}

async fn nl_with_steps(
    state: &test_support::SharedState,
    id: u64,
    text: &str,
    max_steps: Option<u32>,
) -> Response {
    let method = Method::NlQuery {
        text: text.into(),
        graph: GRAPH.into(),
        history: Vec::new(),
        dialect: NlDialect::Auto,
        max_steps,
    };
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn refusal(resp: &Response) -> String {
    format!("{:?} {:?}", resp.error, resp.error_detail)
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.1
#[tokio::test]
async fn second_step_corrects_parse_error() {
    medulla_h_query::nl::set_nl_planner(Arc::new(ScriptedPlanner));
    let state = state();
    for (i, node) in ["d1", "d2", "d3"].iter().enumerate() {
        let added = test_support::dispatch(
            &state,
            test_support::commons_request(
                SECRET,
                10 + i as u64,
                Method::AddNode {
                    node_id: (*node).to_string(),
                    properties_msgpack: test_support::json_bytes(
                        serde_json::json!({"type": "Doc", "year": 2000 + i}),
                    ),
                },
            ),
        )
        .await;
        assert!(added.error.is_none(), "{:?}", added.error);
    }

    // The default budget (3) lets the second step correct the parse error; the
    // rows equal a direct `Uql` call of the corrected text.
    let corrected = nl_with_steps(&state, 20, "two documents, corrected", None).await;
    let nl_rows: Vec<(String, Option<f32>)> = match &corrected.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected Raw rows, got {other:?} / {:?}", corrected.error),
    };
    let direct = test_support::dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            21,
            Method::Uql {
                text: "MATCH (:Doc) |> LIMIT 2".into(),
                params: Default::default(),
                prepare: false,
                foreign_executions: Default::default(),
            },
        ),
    )
    .await;
    let UqlResult::Rows { rows, .. } = (match direct.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice::<UqlResult>(&bytes).unwrap(),
        other => panic!("expected Raw UQL result, got {other:?}"),
    }) else {
        panic!("expected rows");
    };
    assert_eq!(nl_rows.len(), 2);
    assert_eq!(
        nl_rows.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
        rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
    );

    // Two steps are enough, one is not: the budget is the request's `max_steps`.
    let two = nl_with_steps(&state, 22, "two documents, corrected", Some(2)).await;
    assert!(two.error.is_none(), "{}", refusal(&two));
    let one = nl_with_steps(&state, 23, "two documents, corrected", Some(1)).await;
    let text = refusal(&one);
    assert!(text.contains("NL_QUERY_STEPS_EXHAUSTED"), "{text}");
    assert!(text.contains("step 1"), "{text}");
    assert!(one.result.is_none(), "an exhausted budget returns no rows");

    // Outside 1..=4 the request is refused before any planning.
    for (id, bad) in [(24u64, 0u32), (25, 5)] {
        let refused = nl_with_steps(&state, id, "two documents, corrected", Some(bad)).await;
        let text = refusal(&refused);
        assert!(text.contains("NL_STEPS_INVALID"), "{text}");
        assert!(refused.result.is_none());
    }
}

/// Answers the question text as the UQL, so each case chooses its candidate.
struct EchoPlanner;

impl medulla_nlq::nl::NlPlanner for EchoPlanner {
    fn plan(&self, nl: &str, _schema_hint: &str) -> Result<String, String> {
        Ok(nl.to_string())
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.4
// Each case runs in its own process (nextest), so this test owns the planner slot:
// the unconfigured case runs first, then one planner is injected for the others.
#[tokio::test]
async fn refusals_return_code_and_no_rows() {
    let state = state();

    let unconfigured = nl_with_steps(&state, 30, "MATCH (:Doc) |> LIMIT 1", None).await;
    let text = refusal(&unconfigured);
    assert!(text.contains("NL_PLANNER_UNCONFIGURED"), "{text}");
    assert!(unconfigured.result.is_none(), "{text}");

    medulla_h_query::nl::set_nl_planner(Arc::new(EchoPlanner));

    // Every step refused and the budget spent: the code and every step's text.
    let exhausted = nl_with_steps(&state, 31, "MATCH (:Doc", Some(2)).await;
    let text = refusal(&exhausted);
    assert!(text.contains("NL_QUERY_STEPS_EXHAUSTED"), "{text}");
    assert!(text.contains("step 1") && text.contains("step 2"), "{text}");
    assert!(exhausted.result.is_none(), "{text}");

    // A mutation-shaped candidate is refused at once, before the budget runs out.
    for candidate in ["CREATE (:Doc)", "MATCH (:Doc) |> SET year = 1900"] {
        let refused = nl_with_steps(&state, 32, candidate, Some(4)).await;
        let text = refusal(&refused);
        assert!(text.contains("NL_MUTATION_REFUSED"), "{text}");
        assert!(!text.contains("step 2"), "{text}");
        assert!(refused.result.is_none(), "{text}");
    }
}
