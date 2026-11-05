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
use medulla_types::nl_query::{NlQueryPlanResult, NlTurn};

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
