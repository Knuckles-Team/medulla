//! MEDULLA-FINANCE-PRIMITIVES-R901.3 / R613.3 / R018.3: the served banking, regime,
//! execution, pairs and factor-rank methods run the shared kernels through the
//! real signed `dispatch` surface, and a refusal carries the kernel's stable code.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/span_capture.rs"]
mod span_capture;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use serde_json::Value;

const SECRET: &str = "medulla-fin-served-numeric-kernels-secret";
const GRAPH: &str = "fin-numeric-kernels";

/// One signed session over a fresh durable state; calls share the graph.
struct Session {
    state: test_support::SharedState,
    next_id: u64,
}

impl Session {
    async fn open() -> Self {
        common::configure_authority();
        let state = test_support::durable_state(SECRET, common::current_isolation());
        let mut session = Self { state, next_id: 0 };
        let create = Method::CreateGraph {
            graph_name: GRAPH.to_string(),
            graph_type: GraphType::Global,
        };
        let created = session.send(create).await;
        assert!(
            created.error.is_none(),
            "CreateGraph: {:?}",
            created.error_detail
        );
        session
    }

    async fn send(&mut self, method: Method) -> Response {
        self.next_id += 1;
        test_support::dispatch(
            &self.state,
            test_support::request(SECRET, self.next_id, GRAPH, method),
        )
        .await
    }
}

async fn call(method: Method) -> Response {
    Session::open().await.send(method).await
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

fn refusal_code_in_detail(response: &Response, code: &str) {
    assert!(response.result.is_none());
    assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
    let detail = response.error_detail.as_deref().unwrap_or_default();
    assert!(detail.contains(code), "{code} not in {detail:?}");
}

fn aml(amount_minor: i64, threshold_minor: i64) -> Method {
    Method::FinanceAmlScreen {
        rule_id: "ctr".into(),
        version: 3,
        threshold_minor,
        amount_minor,
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R901.3
async fn banking_methods_run_their_kernels() {
    let structuring = body(&call(aml(8_500, 10_000)).await);
    assert_eq!(structuring["alert"]["kind"], "structuring");
    assert_eq!(structuring["alert"]["severity"], "warning");
    let threshold = body(&call(aml(12_000, 10_000)).await);
    assert_eq!(threshold["alert"]["severity"], "report-required");
    assert!(body(&call(aml(100, 10_000)).await)["alert"].is_null());

    let ratios = body(
        &call(Method::FinanceCapitalRatios {
            cet1: 60,
            at1: 20,
            tier2: 20,
            risk_weighted_assets: 1_000,
            total_exposure: 2_000,
        })
        .await,
    );
    assert_eq!(ratios["cet1_ratio"], 0.06);
    assert_eq!(ratios["total_capital_ratio"], 0.1);
    assert_eq!(ratios["cet1_meets_minimum"], true);

    let loss = call(Method::FinanceExpectedLoss {
        pd: 0.1,
        lgd: 0.5,
        ead_minor: 1_000,
    })
    .await;
    assert!(matches!(loss.result, Some(ResultPayload::Float(v)) if (v - 50.0).abs() < 1e-9));

    let legs = body(
        &call(Method::FinanceSettlementChain {
            origin: "A".into(),
            correspondents: vec!["B".into()],
            beneficiary: "C".into(),
            amount_minor: 500,
        })
        .await,
    );
    assert_eq!(legs.as_array().map(Vec::len), Some(2));
    assert_eq!(legs[1]["from"], "B");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R901.3
async fn banking_refusals_carry_the_kernel_code() {
    refusal_code_in_detail(&call(aml(1, 0)).await, "AML_INPUT_INVALID");
    let rwa = call(Method::FinanceCapitalRatios {
        cet1: 1,
        at1: 0,
        tier2: 0,
        risk_weighted_assets: 0,
        total_exposure: 1,
    })
    .await;
    refusal_code_in_detail(&rwa, "CAPITAL_RWA_INVALID");
    let pd = call(Method::FinanceExpectedLoss {
        pd: 1.5,
        lgd: 0.5,
        ead_minor: 1,
    })
    .await;
    refusal_code_in_detail(&pd, "CREDIT_INPUT_INVALID");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R613.3
async fn execution_methods_run_their_kernels() {
    let regime = body(
        &call(Method::FinanceRegimeChain {
            returns: vec![0.02, -0.02, 0.0, 0.02, 0.02, -0.02, 0.0, 0.02],
            threshold: 0.01,
            horizon: 2,
        })
        .await,
    );
    assert_eq!(regime["states"], serde_json::json!(["down", "flat", "up"]));
    assert_eq!(regime["transitions"], 7);
    assert_eq!(regime["solver_status"], "closed_form");

    let plan = body(
        &call(Method::FinanceExecutionSchedule {
            symbol: "AAPL".into(),
            total_shares: 1_000.0,
            horizon: 1.0,
            slices: 4,
            volatility: 1.0,
            risk_aversion: 1.0,
            temporary_impact: 1.0,
            permanent_impact: 0.0,
            continuous: false,
        })
        .await,
    );
    let traded: f64 = plan["trades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_f64().unwrap())
        .sum();
    assert!(
        (traded - 1_000.0).abs() < 1e-6,
        "trades sum to the position: {traded}"
    );
    assert_eq!(plan["solver_status"], "closed_form");

    let quote = body(
        &call(Method::FinanceMarketMakingQuote {
            symbol: "AAPL".into(),
            mid: 100.0,
            inventory: 0.0,
            risk_aversion: 0.1,
            volatility: 0.2,
            time_remaining: 1.0,
            order_arrival_decay: 1.5,
        })
        .await,
    );
    assert_eq!(quote["reservation_price"], 100.0);
    assert!(quote["bid"].as_f64().unwrap() < quote["ask"].as_f64().unwrap());

    let pairs = body(
        &call(Method::FinancePairsSignal {
            y_name: "Y".into(),
            x_name: "X".into(),
            y: vec![1.0, 2.0, 3.0, 4.0, 9.0],
            x: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            hedge_ratio: 1.0,
            entry_z: 1.0,
        })
        .await,
    );
    assert_eq!(pairs["side"], "short_spread");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R613.3
async fn execution_refusals_carry_the_kernel_code() {
    let bad = call(Method::FinanceExecutionSchedule {
        symbol: "AAPL".into(),
        total_shares: 1_000.0,
        horizon: 1.0,
        slices: 4,
        volatility: 1.0,
        risk_aversion: 0.0,
        temporary_impact: 1.0,
        permanent_impact: 0.0,
        continuous: false,
    })
    .await;
    refusal_code_in_detail(&bad, "EXEC_PARAM_INVALID: risk_aversion");
}

fn rank(signals: Vec<Vec<f64>>, forward: Vec<f64>) -> Method {
    let ids: Vec<String> = (0..signals.len()).map(|i| format!("f{i}")).collect();
    let keys = ids.iter().map(|id| vec![format!("src:{id}")]).collect();
    Method::FinanceFactorRank {
        factor_ids: ids,
        signals,
        provenance_keys: keys,
        forward_returns: forward,
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R018.3
async fn factor_rank_orders_by_absolute_information_ratio() {
    let forward = vec![0.01, 0.02, 0.03, 0.04, 0.05, 0.06];
    let strong = forward.clone();
    let inverse: Vec<f64> = forward.iter().map(|v| -v).collect();
    let noisy = vec![0.3, -0.1, 0.2, -0.4, 0.1, 0.0];
    let ranked = body(&call(rank(vec![noisy, strong, inverse], forward)).await);
    let records = ranked.as_array().expect("records");
    assert_eq!(records.len(), 3);
    assert_eq!(records[2]["factor_id"], "f0", "the noisy factor ranks last");
    assert_eq!(records[0]["rank"], 1);
    assert_eq!(
        records[0]["input_provenance_keys"].as_array().map(Vec::len),
        Some(1)
    );
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R018.3
async fn factor_rank_abstains_on_thin_input() {
    refusal_code_in_detail(
        &call(rank(vec![vec![1.0]], vec![1.0])).await,
        "FACTOR_INSUFFICIENT_DATA",
    );
    refusal_code_in_detail(
        &call(rank(vec![], vec![])).await,
        "FACTOR_INSUFFICIENT_DATA",
    );
}

fn schedule(slices: usize, risk_aversion: f64) -> Method {
    Method::FinanceExecutionSchedule {
        symbol: "AAPL".into(),
        total_shares: 1_000.0,
        horizon: 1.0,
        slices,
        volatility: 1.0,
        risk_aversion,
        temporary_impact: 1.0,
        permanent_impact: 0.0,
        continuous: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R613.5
async fn execution_results_carry_evidence_and_refusal_trace() {
    span_capture::install();
    // Success: the result carries its parameters, objective and solver status.
    let ok_method = schedule(4, 1.0);
    let plan = body(&call(ok_method.clone()).await);
    assert_eq!(plan["slices"], 4);
    assert_eq!(plan["risk_aversion"], 1.0);
    assert!(plan["objective"].as_f64().is_some_and(f64::is_finite));
    assert_eq!(plan["solver_status"], "closed_form");
    let ok_spans = span_capture::spans(
        "medulla.finance.execution_schedule",
        &span_capture::digest_of(&ok_method),
    );
    assert_eq!(ok_spans.len(), 1, "one evidence span: {ok_spans:?}");
    assert_eq!(
        ok_spans[0].fields.get("outcome").map(String::as_str),
        Some("ok")
    );

    // Refusal: a trace event names the code and the refused field.
    let bad_method = schedule(4, 0.0);
    refusal_code_in_detail(&call(bad_method.clone()).await, "EXEC_PARAM_INVALID");
    let bad_spans = span_capture::spans(
        "medulla.finance.execution_schedule",
        &span_capture::digest_of(&bad_method),
    );
    assert_eq!(bad_spans.len(), 1);
    assert_eq!(
        bad_spans[0].fields.get("outcome").map(String::as_str),
        Some("EXEC_PARAM_INVALID")
    );
    let events = span_capture::events_in("medulla.finance.execution_schedule");
    assert!(
        events.iter().any(|e| {
            e.fields.get("code").map(String::as_str) == Some("EXEC_PARAM_INVALID")
                && e.fields.get("field").map(String::as_str) == Some("risk_aversion")
        }),
        "refusal trace event: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R613.6
async fn r613_budget_inputs_complete() {
    // The bench inputs are accepted at full size: a 1000-slice schedule and a regime
    // fit over 1_000_000 bars.
    let plan = body(&call(schedule(1_000, 1.0)).await);
    assert_eq!(plan["trades"].as_array().map(Vec::len), Some(1_000));
    let returns: Vec<f64> = (0..1_000_000).map(|i| [0.02, -0.02, 0.0][i % 3]).collect();
    let regime = body(
        &call(Method::FinanceRegimeChain {
            returns,
            threshold: 0.01,
            horizon: 1,
        })
        .await,
    );
    assert_eq!(regime["transitions"], 999_999);
}

const FACTOR_IDS: [&str; 3] = ["f0", "f1", "f2"];

fn factor_signals() -> (Vec<Vec<f64>>, Vec<f64>) {
    let forward = vec![0.01, 0.02, 0.03, 0.04, 0.05, 0.06];
    let inverse = forward.iter().map(|v| -v).collect();
    let noisy = vec![0.3, -0.1, 0.2, -0.4, 0.1, 0.0];
    (vec![noisy, forward.clone(), inverse], forward)
}

fn record(provenance: Vec<Vec<String>>) -> Method {
    let (signals, forward_returns) = factor_signals();
    Method::FinanceFactorRecord {
        factor_ids: FACTOR_IDS.iter().map(|id| id.to_string()).collect(),
        signals,
        provenance_keys: provenance,
        forward_returns,
    }
}

fn provenance_of(factor: &str) -> Vec<String> {
    vec![format!("src:{factor}"), format!("bars:{factor}")]
}

/// Store the provenance nodes a factor record must resolve.
async fn seed_provenance(session: &mut Session) {
    let props = rmp_serde::to_vec_named(&serde_json::json!({"type": "Bar"})).expect("props");
    for factor in FACTOR_IDS {
        for node_id in provenance_of(factor) {
            let added = session
                .send(Method::AddNode {
                    node_id,
                    properties_msgpack: props.clone(),
                })
                .await;
            assert!(added.error.is_none(), "seed: {:?}", added.error_detail);
        }
    }
}

async fn node(session: &mut Session, node_id: &str) -> Option<Value> {
    let response = session
        .send(Method::GetNodeProperties {
            node_id: node_id.to_string(),
        })
        .await;
    assert!(response.error.is_none(), "get: {:?}", response.error_detail);
    match response.result.as_ref() {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("node"),
        _ => None,
    }
}

async fn successors(session: &mut Session, node_id: &str) -> Vec<String> {
    let response = session
        .send(Method::GetSuccessors {
            node_id: node_id.to_string(),
        })
        .await;
    match response.result {
        Some(ResultPayload::Ids(ids)) => ids,
        other => panic!("expected ids, got {other:?}"),
    }
}

fn all_provenance() -> Vec<Vec<String>> {
    FACTOR_IDS.iter().map(|id| provenance_of(id)).collect()
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R018.5
async fn factor_rank_records_nodes_with_edges_and_versions() {
    let mut session = Session::open().await;
    seed_provenance(&mut session).await;
    let ranked = session.send(record(all_provenance())).await;
    assert!(ranked.error.is_none(), "record: {:?}", ranked.error_detail);
    let stored = node(&mut session, "finance:Factor:f1")
        .await
        .expect("factor node");
    assert_eq!(stored["type"], "Factor");
    assert_eq!(
        stored["irKernelVersion"],
        medulla_finance_quant::factor::KERNEL_REVISION
    );
    assert_eq!(stored["icKernelVersion"], stored["irKernelVersion"]);
    let mut edges = successors(&mut session, "finance:Factor:f1").await;
    edges.sort();
    assert_eq!(edges, provenance_of("f1"));

    // An empty provenance list refuses and writes nothing.
    let mut refused = Session::open().await;
    seed_provenance(&mut refused).await;
    let mut provenance = all_provenance();
    provenance[1].clear();
    let response = refused.send(record(provenance)).await;
    assert!(response.result.is_none());
    let detail = response.error_detail.as_deref().unwrap_or_default();
    assert!(
        response.error.as_deref() == Some("FACTOR_RECORD_INVALID")
            || detail.contains("FACTOR_RECORD_INVALID"),
        "{response:?}"
    );
    assert!(node(&mut refused, "finance:Factor:f1").await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R018.6
async fn factor_rank_hot_path_writes_three_nodes() {
    let mut session = Session::open().await;
    seed_provenance(&mut session).await;
    // The read-only form writes none.
    let (signals, forward) = factor_signals();
    let read_only = session.send(rank(signals, forward)).await;
    assert!(read_only.error.is_none());
    for id in FACTOR_IDS {
        assert!(node(&mut session, &format!("finance:Factor:{id}"))
            .await
            .is_none());
    }
    // One record request writes three nodes and their edge sets, in rank order.
    let recorded = body(&session.send(record(all_provenance())).await);
    let order: Vec<&str> = recorded
        .as_array()
        .expect("ranked records")
        .iter()
        .map(|r| r["factor_id"].as_str().expect("id"))
        .collect();
    assert_eq!(order.last(), Some(&"f0"), "the noisy factor ranks last");
    for (index, id) in order.iter().enumerate() {
        let stored = node(&mut session, &format!("finance:Factor:{id}"))
            .await
            .expect("factor node");
        assert_eq!(stored["factorRank"], index as u64 + 1);
        let edges = successors(&mut session, &format!("finance:Factor:{id}")).await;
        assert_eq!(edges.len(), 2, "{id} edge set");
    }
}
