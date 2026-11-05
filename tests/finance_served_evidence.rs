//! MEDULLA-FINANCE-PRIMITIVES-R900.5: each served `FinancePositionSize` and
//! `FinanceDistributionShift` call emits one evidence span carrying the tenant, the
//! SHA-256 digest of the canonical request bytes, the outcome and (sizing) whether
//! the risk cap applied, and no sample value.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/span_capture.rs"]
mod span_capture;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response};

const SECRET: &str = "medulla-fin-r900-5-served-evidence-secret";
const GRAPH: &str = "fin-r900-5";

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

fn one_span(name: &str, method: &Method) -> span_capture::Captured {
    let digest = span_capture::digest_of(method);
    assert_eq!(digest.len(), 64);
    assert!(digest
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    let spans = span_capture::spans(name, &digest);
    assert_eq!(spans.len(), 1, "one {name} span: {spans:?}");
    let span = spans.into_iter().next().unwrap();
    assert!(span.fields.get("tenant").is_some_and(|t| !t.is_empty()));
    // No sample value is recorded: only the evidence attributes.
    for key in span.fields.keys() {
        assert!(
            ["tenant", "inputs_digest", "outcome", "cap_applied"].contains(&key.as_str()),
            "unexpected span field {key}"
        );
    }
    span
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R900.5
async fn sizing_and_shift_spans_carry_digest_cap_and_outcome() {
    span_capture::install();
    // p=0.6, b=2 -> full Kelly 0.4, quarter Kelly 0.1, capped at 0.05.
    let capped = Method::FinancePositionSize {
        accuracy: 0.6,
        win_loss_ratio: 2.0,
        fraction: Some(0.25),
        max_risk: Some(0.05),
    };
    assert!(call(capped.clone()).await.error.is_none());
    let span = one_span("medulla.finance.position_size", &capped);
    assert_eq!(span.fields.get("outcome").map(String::as_str), Some("ok"));
    assert_eq!(
        span.fields.get("cap_applied").map(String::as_str),
        Some("true")
    );

    let shift = Method::FinanceDistributionShift {
        reference: vec![0.0, 0.25, 0.5, 0.75],
        current: vec![5.0, 5.25, 5.5, 5.75],
        threshold: 0.5,
    };
    assert!(call(shift.clone()).await.error.is_none());
    let span = one_span("medulla.finance.distribution_shift", &shift);
    assert_eq!(span.fields.get("outcome").map(String::as_str), Some("ok"));

    // A bad input records the refusal code as the outcome.
    let bad = Method::FinancePositionSize {
        accuracy: 1.5,
        win_loss_ratio: 2.0,
        fraction: None,
        max_risk: None,
    };
    assert!(call(bad.clone()).await.result.is_none());
    let span = one_span("medulla.finance.position_size", &bad);
    assert_eq!(
        span.fields.get("outcome").map(String::as_str),
        Some("SIZING_INPUT_INVALID")
    );
}
