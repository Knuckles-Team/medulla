//! MEDULLA-FINANCE-PRIMITIVES-R750.752 / R750.753 / R750.754: the served
//! `FinancePatternClassify` and `FinanceProfitAttribution` methods run the shared pure
//! kernels through the real signed `dispatch` surface, and each invalid input returns
//! its stable refusal code with no partial result.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_result_types::compute_result::finance::GroupInput;
use serde_json::Value;

const SECRET: &str = "medulla-fin-served-research-kernels-secret";
const GRAPH: &str = "fin-research-kernels";

async fn call(method: Method) -> Response {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let create = Method::CreateGraph {
        graph_name: GRAPH.to_string(),
        graph_type: GraphType::Global,
    };
    let created =
        test_support::dispatch(&state, test_support::request(SECRET, 1, GRAPH, create)).await;
    assert!(
        created.error.is_none(),
        "CreateGraph: {:?}",
        created.error_detail
    );
    test_support::dispatch(&state, test_support::request(SECRET, 2, GRAPH, method)).await
}

fn body(response: &Response) -> Value {
    match response.result.as_ref() {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!(
            "expected a raw result, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

fn refusal_code(response: &Response, code: &str) {
    assert!(response.result.is_none(), "a refusal returns no result");
    assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
    let detail = response.error_detail.as_deref().unwrap_or_default();
    assert!(detail.contains(code), "{code} not in {detail:?}");
}

fn bars(rows: &[(f64, f64, f64, f64)]) -> Method {
    Method::FinancePatternClassify {
        open: rows.iter().map(|r| r.0).collect(),
        high: rows.iter().map(|r| r.1).collect(),
        low: rows.iter().map(|r| r.2).collect(),
        close: rows.iter().map(|r| r.3).collect(),
    }
}

fn group(name: &str, w_p: f64, w_b: f64, r_p: f64, r_b: f64) -> GroupInput {
    GroupInput {
        group: name.into(),
        w_p,
        w_b,
        r_p,
        r_b,
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R750.753
// spec: MEDULLA-FINANCE-PRIMITIVES-R750.754
async fn both_kernels_run_through_dispatch() {
    // A bearish bar followed by a bar that engulfs it: one bullish engulfing at index 1.
    let matches = body(&call(bars(&[(10.0, 10.5, 8.5, 9.0), (8.8, 11.2, 8.7, 11.0)])).await);
    let list = matches.as_array().expect("a pattern list");
    assert_eq!(list.len(), 1, "{matches}");
    assert_eq!(list[0]["index"], 1);
    assert_eq!(list[0]["kind"], "bullish_engulfing");
    assert!((list[0]["strength"].as_f64().unwrap() - 0.88).abs() < 1e-9);

    let breakdown = body(
        &call(Method::FinanceProfitAttribution {
            groups: vec![
                group("equity", 0.6, 0.5, 0.08, 0.06),
                group("bonds", 0.4, 0.5, 0.02, 0.03),
            ],
        })
        .await,
    );
    let active = 0.6 * 0.08 + 0.4 * 0.02 - (0.5 * 0.06 + 0.5 * 0.03);
    assert!((breakdown["active_return"].as_f64().unwrap() - active).abs() < 1e-12);
    let total = breakdown["allocation"].as_f64().unwrap()
        + breakdown["selection"].as_f64().unwrap()
        + breakdown["interaction"].as_f64().unwrap();
    assert!((total - active).abs() < 1e-12);
    assert_eq!(breakdown["groups"].as_array().map(Vec::len), Some(2));
    assert_eq!(breakdown["groups"][0]["group"], "equity");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R750.752
async fn each_invalid_input_returns_its_code() {
    refusal_code(
        &call(bars(&[(1.0, 2.0, 0.5, 1.5)])).await,
        "PATTERN_WINDOW_SHORT",
    );
    // Unequal slice lengths.
    let ragged = Method::FinancePatternClassify {
        open: vec![1.0, 1.0],
        high: vec![2.0],
        low: vec![0.5, 0.5],
        close: vec![1.5, 1.5],
    };
    refusal_code(&call(ragged).await, "PATTERN_INPUT_INVALID");
    // high below low.
    refusal_code(
        &call(bars(&[(1.0, 0.5, 2.0, 1.5), (1.0, 2.0, 0.5, 1.5)])).await,
        "PATTERN_INPUT_INVALID",
    );
    // A non-finite value.
    refusal_code(
        &call(bars(&[(f64::NAN, 2.0, 0.5, 1.5), (1.0, 2.0, 0.5, 1.5)])).await,
        "PATTERN_INPUT_INVALID",
    );

    refusal_code(
        &call(Method::FinanceProfitAttribution {
            groups: vec![
                group("a", 0.6, 0.5, 0.1, 0.1),
                group("b", 0.3, 0.5, 0.1, 0.1),
            ],
        })
        .await,
        "ATTRIBUTION_WEIGHTS_INVALID",
    );
    refusal_code(
        &call(Method::FinanceProfitAttribution {
            groups: vec![
                group("a", 1.5, 0.5, 0.1, 0.1),
                group("b", -0.5, 0.5, 0.1, 0.1),
            ],
        })
        .await,
        "ATTRIBUTION_WEIGHTS_INVALID",
    );
    refusal_code(
        &call(Method::FinanceProfitAttribution { groups: vec![] }).await,
        "ATTRIBUTION_INPUT_INVALID",
    );
    refusal_code(
        &call(Method::FinanceProfitAttribution {
            groups: vec![
                group("a", 0.5, 0.5, 0.1, 0.1),
                group("a", 0.5, 0.5, 0.1, 0.1),
            ],
        })
        .await,
        "ATTRIBUTION_INPUT_INVALID",
    );
    refusal_code(
        &call(Method::FinanceProfitAttribution {
            groups: vec![group("a", 1.0, 1.0, f64::INFINITY, 0.1)],
        })
        .await,
        "ATTRIBUTION_INPUT_INVALID",
    );
}
