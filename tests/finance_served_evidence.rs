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

/// The one banking span of `method`, its evidence attributes only (no amount).
fn banking_span(name: &str, method: &Method) -> span_capture::Captured {
    let spans = span_capture::spans(name, &span_capture::digest_of(method));
    assert_eq!(spans.len(), 1, "one {name} span: {spans:?}");
    let span = spans.into_iter().next().unwrap();
    assert!(span.fields.get("tenant").is_some_and(|t| !t.is_empty()));
    for key in span.fields.keys() {
        assert!(
            ["tenant", "inputs_digest", "outcome", "rule_set_version"].contains(&key.as_str()),
            "unexpected span field {key}"
        );
    }
    span
}

fn field<'a>(span: &'a span_capture::Captured, key: &str) -> Option<&'a str> {
    span.fields.get(key).map(String::as_str)
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R901.5
async fn banking_spans_carry_rule_set_digest_and_outcome() {
    span_capture::install();
    let aml = |amount_minor: i64, threshold_minor: i64| Method::FinanceAmlScreen {
        rule_id: "ctr-r901-5".into(),
        version: 7,
        threshold_minor,
        amount_minor,
    };
    // Structuring band: an alert is the outcome; the AML version is the rule set.
    let alerting = aml(8_501, 10_000);
    assert!(call(alerting.clone()).await.error.is_none());
    let span = banking_span("medulla.finance.aml_screen", &alerting);
    assert_eq!(field(&span, "outcome"), Some("alert"));
    assert_eq!(field(&span, "rule_set_version"), Some("7"));
    let clean = aml(101, 10_000);
    assert!(call(clean.clone()).await.error.is_none());
    let span = banking_span("medulla.finance.aml_screen", &clean);
    assert_eq!(field(&span, "outcome"), Some("ok"));
    let refused = aml(1, 0);
    assert!(call(refused.clone()).await.result.is_none());
    let span = banking_span("medulla.finance.aml_screen", &refused);
    assert_eq!(field(&span, "outcome"), Some("AML_INPUT_INVALID"));

    let ratios = Method::FinanceCapitalRatios {
        cet1: 61,
        at1: 20,
        tier2: 20,
        risk_weighted_assets: 1_000,
        total_exposure: 2_000,
    };
    assert!(call(ratios.clone()).await.error.is_none());
    let span = banking_span("medulla.finance.capital_ratios", &ratios);
    assert_eq!(field(&span, "outcome"), Some("ok"));
    assert_eq!(field(&span, "rule_set_version"), Some("banking-minimums-1"));
    let rwa = Method::FinanceCapitalRatios {
        cet1: 2,
        at1: 0,
        tier2: 0,
        risk_weighted_assets: 0,
        total_exposure: 1,
    };
    assert!(call(rwa.clone()).await.result.is_none());
    let span = banking_span("medulla.finance.capital_ratios", &rwa);
    assert_eq!(field(&span, "outcome"), Some("CAPITAL_RWA_INVALID"));

    let loss = Method::FinanceExpectedLoss {
        pd: 0.2,
        lgd: 0.5,
        ead_minor: 1_000,
    };
    assert!(call(loss.clone()).await.error.is_none());
    let span = banking_span("medulla.finance.expected_loss", &loss);
    assert_eq!(field(&span, "outcome"), Some("ok"));
    assert_eq!(field(&span, "rule_set_version"), Some("none"));
    let bad_pd = Method::FinanceExpectedLoss {
        pd: 1.5,
        lgd: 0.5,
        ead_minor: 1,
    };
    assert!(call(bad_pd.clone()).await.result.is_none());
    let span = banking_span("medulla.finance.expected_loss", &bad_pd);
    assert_eq!(field(&span, "outcome"), Some("CREDIT_INPUT_INVALID"));

    let chain = Method::FinanceSettlementChain {
        origin: "A".into(),
        correspondents: vec!["B".into()],
        beneficiary: "C".into(),
        amount_minor: 501,
    };
    assert!(call(chain.clone()).await.error.is_none());
    let span = banking_span("medulla.finance.settlement_chain", &chain);
    assert_eq!(field(&span, "outcome"), Some("ok"));
    let party = Method::FinanceSettlementChain {
        origin: String::new(),
        correspondents: vec![],
        beneficiary: "C".into(),
        amount_minor: 501,
    };
    assert!(call(party.clone()).await.result.is_none());
    let span = banking_span("medulla.finance.settlement_chain", &party);
    assert_eq!(field(&span, "outcome"), Some("SETTLEMENT_PARTY_INVALID"));
}
