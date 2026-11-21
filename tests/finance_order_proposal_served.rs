//! MEDULLA-FINANCE-PRIMITIVES-R015.1.1 to R015.1.3 and R010.2: the served order
//! proposal door (`FinanceOrderStage`, `FinanceOrderPromote`) and the limit
//! verification (`FinanceOrderLimitCheck`) through the real signed `dispatch`
//! surface. Digests are computed here with the same domain-separated helper the
//! engine seals with; refusals are asserted by their wire code and by an unchanged
//! node count.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "finance",
    feature = "shacl"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;

use medulla_finance_market::market::digest;
use medulla_finance_types::finance_order::{
    EvidenceRef, FinanceOrder, GateReceipt, OrderLimitCheckResult, OrderProposalSnapshot,
    OrderPromoteRequest, OrderPromoteResult, OrderStageResult, TradingPolicy,
    ORDER_PAYLOAD_SCHEMA_ID, ORDER_PAYLOAD_SCHEMA_REVISION,
};
use medulla_finance_types::wire_finance::{
    FinanceRecord, FinanceRecordKind, FINANCE_V1_SCHEMA_VERSION,
};
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SECRET: &str = "medulla-fin-order-proposal-secret";
const TENANT: &str = "integration-test-tenant";
const GRAPH: &str = "fin-order";
const POLICY_ID: &str = "pol1";
const EVIDENCE: &str = "finance:TradingPolicy:pol1";

fn turtle(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn risk_limits() -> Value {
    json!([
        {"id": "qty", "metric": "position_size", "bound": 1000, "scope": "AAPL"},
        {"id": "notional", "metric": "position_size", "bound": 500000, "scope": "AAPL:notional"}
    ])
}

fn leverage_limits() -> Value {
    json!([{"instrument_id": "AAPL", "approvers": ["alice"], "cfd_unavailable_us_retail": true}])
}

fn policy() -> TradingPolicy {
    TradingPolicy {
        tenant_id: TENANT.to_string(),
        revision: 3,
        leverage_limits: leverage_limits(),
        risk_limits: risk_limits(),
        designated_approvers: vec!["alice".to_string()],
    }
}

fn policy_digest() -> String {
    digest::of_json("finance.trading_policy.v1", &policy())
}

fn snapshot(revision: u64, predecessor: Option<&str>, order: &FinanceOrder) -> OrderProposalSnapshot {
    let bytes = serde_json::to_vec(order).unwrap();
    OrderProposalSnapshot {
        tenant_id: TENANT.to_string(),
        proposal_id: "prop1".to_string(),
        revision,
        predecessor_digest: predecessor.map(str::to_string),
        decision_id: "dec1".to_string(),
        decision_revision: 1,
        decision_digest: "sha256:dec".to_string(),
        payload_schema_id: ORDER_PAYLOAD_SCHEMA_ID.to_string(),
        payload_schema_revision: ORDER_PAYLOAD_SCHEMA_REVISION.to_string(),
        payload_sha256: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
        canonical_payload_bytes: bytes,
        policy_id: POLICY_ID.to_string(),
        policy_revision: 3,
        policy_digest: policy_digest(),
        evidence_refs: vec![EvidenceRef {
            owner_requirement_id: "MEDULLA-DECISION-ENGINE-R019".to_string(),
            record_id: EVIDENCE.to_string(),
            revision: 1,
            digest: "sha256:ev".to_string(),
        }],
    }
}

fn order(quantity: i64, notional: i64) -> FinanceOrder {
    FinanceOrder {
        instrument_id: "AAPL".to_string(),
        signed_quantity: quantity,
        notional,
    }
}

fn seal(snapshot: &OrderProposalSnapshot) -> String {
    digest::of_json("finance.order_proposal_snapshot.v1", snapshot)
}

struct Served {
    state: test_support::SharedState,
    next_id: u64,
}

impl Served {
    async fn new() -> Self {
        common::configure_authority();
        let state = test_support::durable_state(SECRET, common::current_isolation());
        let mut served = Self { state, next_id: 1 };
        let created = served
            .call(Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            })
            .await;
        assert!(created.error.is_none(), "{:?}", created.error_detail);
        served.put_policy().await;
        served
    }

    async fn call(&mut self, method: Method) -> Response {
        self.next_id += 1;
        test_support::dispatch(
            &self.state,
            test_support::request(SECRET, self.next_id, GRAPH, method),
        )
        .await
    }

    async fn put_policy(&mut self) {
        let fields: BTreeMap<String, String> = [
            ("tradingPolicyRevision", turtle("3")),
            ("tradingPolicyLeverageLimits", turtle(&leverage_limits().to_string())),
            ("tradingPolicyRiskLimits", turtle(&risk_limits().to_string())),
            ("tradingPolicyApprovers", turtle(&json!(["alice"]).to_string())),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
        let response = self
            .call(Method::FinanceRecordPut {
                kind: FinanceRecordKind::TradingPolicy,
                schema_version: FINANCE_V1_SCHEMA_VERSION.to_string(),
                record: FinanceRecord {
                    record_id: POLICY_ID.to_string(),
                    tenant: TENANT.to_string(),
                    fields,
                },
                expected_head: None,
            })
            .await;
        assert!(response.error.is_none(), "{:?}", response.error_detail);
    }

    async fn stage(
        &mut self,
        snapshot: OrderProposalSnapshot,
        head_revision: Option<u64>,
        head_digest: Option<&str>,
    ) -> Response {
        self.call(Method::FinanceOrderStage {
            snapshot: Box::new(snapshot),
            expected_head_revision: head_revision,
            expected_head_digest: head_digest.map(str::to_string),
        })
        .await
    }

    async fn promote(&mut self, request: OrderPromoteRequest) -> Response {
        self.call(Method::FinanceOrderPromote {
            request: Box::new(request),
        })
        .await
    }

    async fn nodes(&mut self) -> String {
        format!("{:?}", self.call(Method::NodeCount).await.result)
    }

    async fn orders(&mut self) -> String {
        let response = self
            .call(Method::GetNodesByLabel {
                label: "Order".into(),
                after: None,
                limit: 100,
            })
            .await;
        assert!(response.error.is_none(), "{:?}", response.error_detail);
        format!("{:?}", response.result)
    }

    async fn refused(&mut self, response: Response, code: &str, before: &str) {
        let detail = response.error_detail.clone().unwrap_or_default();
        assert!(
            response.error.is_some() && detail.contains(code),
            "expected {code}, got {detail}"
        );
        assert_eq!(self.nodes().await, before, "{code} must write nothing");
    }
}

fn body<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    let value = match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => value.clone(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!("expected a body, got {other:?} / {:?}", response.error_detail),
    };
    serde_json::from_value(value).expect("typed body")
}

fn manifest_digest(ids: &[&str]) -> String {
    let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    digest::of_json("finance.gate_manifest.v1", &ids)
}

fn promote_request(revision: u64, snapshot_digest: &str, expected: Option<u64>) -> OrderPromoteRequest {
    OrderPromoteRequest {
        tenant_id: TENANT.to_string(),
        proposal_id: "prop1".to_string(),
        candidate_revision: revision,
        snapshot_digest: snapshot_digest.to_string(),
        expected_promoted_revision: expected,
        required_gate_manifest_digest: manifest_digest(&["risk", "compliance"]),
        manifest_gate_ids: vec!["risk".to_string(), "compliance".to_string()],
        gate_receipts: ["risk", "compliance"]
            .iter()
            .map(|gate| GateReceipt {
                gate_id: gate.to_string(),
                gate_revision: 1,
                candidate_digest: snapshot_digest.to_string(),
                pass_fail: true,
                evidence_digest: format!("sha256:{gate}"),
            })
            .collect(),
        approver: Some("alice".to_string()),
        veto_reason: None,
    }
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R015.1.1
#[tokio::test]
async fn stage_revision_chain_and_refusals() {
    let mut served = Served::new().await;
    let first = snapshot(1, None, &order(100, 20_000));
    let staged = served.stage(first.clone(), None, None).await;
    assert!(staged.error.is_none(), "{:?}", staged.error_detail);
    let result: OrderStageResult = body(&staged);
    assert_eq!(result.state, "staged");
    assert_eq!((result.revision, result.predecessor_digest), (1, None));
    assert_eq!(result.snapshot_digest, seal(&first));
    let read = served
        .call(Method::GetNodeProperties {
            node_id: "finance:OrderProposalSnapshot:prop1:1".to_string(),
        })
        .await;
    assert!(read.error.is_none() && !format!("{:?}", read.result).contains("Null"));

    let second = snapshot(2, Some(&seal(&first)), &order(120, 24_000));
    let staged = served.stage(second.clone(), Some(1), Some(&seal(&first))).await;
    assert!(staged.error.is_none(), "{:?}", staged.error_detail);
    assert_eq!(body::<OrderStageResult>(&staged).predecessor_digest, Some(seal(&first)));

    let count = served.nodes().await;
    let retry = served.stage(second.clone(), Some(1), Some(&seal(&first))).await;
    assert!(retry.error.is_none(), "{:?}", retry.error_detail);
    assert_eq!(served.nodes().await, count, "an equal retry writes nothing new");

    let other_bytes = snapshot(2, Some(&seal(&first)), &order(121, 24_100));
    let response = served.stage(other_bytes, Some(1), Some(&seal(&first))).await;
    served.refused(response, "PROPOSAL_REVISION_CONFLICT", &count).await;
    let third = snapshot(3, Some(&seal(&second)), &order(130, 26_000));
    let response = served.stage(third.clone(), Some(1), Some(&seal(&first))).await;
    served.refused(response, "PROPOSAL_HEAD_STALE", &count).await;
    let mut tampered = third.clone();
    tampered.payload_sha256 = "sha256:00".to_string();
    let response = served.stage(tampered, Some(2), Some(&seal(&second))).await;
    served.refused(response, "PROPOSAL_HASH_MISMATCH", &count).await;
    let mut noncanonical = third.clone();
    noncanonical.canonical_payload_bytes =
        b"{ \"instrument_id\":\"AAPL\", \"signed_quantity\":130, \"notional\":26000 }".to_vec();
    noncanonical.payload_sha256 = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(&noncanonical.canonical_payload_bytes))
    );
    let response = served.stage(noncanonical, Some(2), Some(&seal(&second))).await;
    served.refused(response, "PROPOSAL_HASH_MISMATCH", &count).await;
    let mut no_evidence = third.clone();
    no_evidence.evidence_refs[0].record_id = "finance:Nothing:x".to_string();
    let response = served.stage(no_evidence, Some(2), Some(&seal(&second))).await;
    served.refused(response, "PROPOSAL_EVIDENCE_MISSING", &count).await;
    let mut foreign = third;
    foreign.tenant_id = "another-tenant".to_string();
    let response = served.stage(foreign, Some(2), Some(&seal(&second))).await;
    served.refused(response, "ACCESS_DENIED", &count).await;
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R010.2
#[tokio::test]
async fn breaching_order_abstains_and_stage_writes_nothing() {
    let mut served = Served::new().await;
    let breaching = snapshot(1, None, &order(-1500, 900_000));
    let checked = served
        .call(Method::FinanceOrderLimitCheck {
            proposal: Box::new(breaching.clone()),
        })
        .await;
    let verdict: OrderLimitCheckResult = body(&checked);
    assert_eq!(verdict.outcome, "Abstained");
    let rules: Vec<(&str, i64, i64)> = verdict
        .reasons
        .iter()
        .map(|r| (r.rule_id.as_str(), r.observed, r.bound))
        .collect();
    assert_eq!(rules, [("qty", 1500, 1000), ("notional", 900_000, 500_000)]);
    let before = served.nodes().await;
    let response = served.stage(breaching, None, None).await;
    served.refused(response, "ORDER_LIMIT_BREACH", &before).await;
    let passing = served
        .call(Method::FinanceOrderLimitCheck {
            proposal: Box::new(snapshot(1, None, &order(10, 2_000))),
        })
        .await;
    assert_eq!(body::<OrderLimitCheckResult>(&passing).outcome, "Passed");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R015.1.2
#[tokio::test]
async fn promote_refuses_each_case_without_writes() {
    let mut served = Served::new().await;
    let first = snapshot(1, None, &order(100, 20_000));
    let digest_1 = seal(&first);
    assert!(served.stage(first, None, None).await.error.is_none());
    let before = served.nodes().await;

    let mut vetoed = promote_request(1, &digest_1, None);
    vetoed.veto_reason = Some("hook veto".to_string());
    let response = served.promote(vetoed).await;
    served.refused(response, "ORDER_PROMOTION_VETOED", &before).await;
    let response = served.promote(promote_request(1, "sha256:other", None)).await;
    served.refused(response, "PROMOTE_EVIDENCE_MISMATCH", &before).await;
    let response = served.promote(promote_request(1, &digest_1, Some(4))).await;
    served.refused(response, "PROMOTE_HEAD_STALE", &before).await;
    let mut no_manifest = promote_request(1, &digest_1, None);
    no_manifest.manifest_gate_ids.clear();
    let response = served.promote(no_manifest).await;
    served.refused(response, "PROMOTE_GATE_UNAVAILABLE", &before).await;
    let mut missing_receipt = promote_request(1, &digest_1, None);
    missing_receipt.gate_receipts.pop();
    let response = served.promote(missing_receipt).await;
    served.refused(response, "PROMOTE_GATE_UNAVAILABLE", &before).await;
    let mut incomplete = promote_request(1, &digest_1, None);
    incomplete.gate_receipts[0].candidate_digest = "sha256:elsewhere".to_string();
    let response = served.promote(incomplete).await;
    served.refused(response, "PROMOTE_RECEIPT_INCOMPLETE", &before).await;
    let mut failing = promote_request(1, &digest_1, None);
    failing.gate_receipts[1].pass_fail = false;
    let response = served.promote(failing).await;
    served.refused(response, "PROMOTE_GATE_FAILED", &before).await;
    let mut stranger = promote_request(1, &digest_1, None);
    stranger.approver = Some("mallory".to_string());
    let response = served.promote(stranger).await;
    served.refused(response, "ORDER_PROMOTION_VETOED", &before).await;

    let promoted = served.promote(promote_request(1, &digest_1, None)).await;
    assert!(promoted.error.is_none(), "{:?}", promoted.error_detail);
    let result: OrderPromoteResult = body(&promoted);
    assert_eq!(result.receipt.snapshot_digest, digest_1);
    assert_eq!(result.receipt.policy_digest, policy_digest());
    let after = served.nodes().await;
    let response = served.promote(promote_request(1, &digest_1, None)).await;
    served.refused(response, "PROMOTE_HEAD_STALE", &after).await;
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R015.1.3
#[tokio::test]
async fn admission_receipt_is_atomic_and_effect_free() {
    let mut served = Served::new().await;
    let first = snapshot(1, None, &order(100, 20_000));
    let digest_1 = seal(&first);
    assert!(served.stage(first, None, None).await.error.is_none());
    let before = served.orders().await;
    let promoted = served.promote(promote_request(1, &digest_1, None)).await;
    assert!(promoted.error.is_none(), "{:?}", promoted.error_detail);
    let result: OrderPromoteResult = body(&promoted);
    assert_eq!(result.receipt_id, "finance:OrderAdmissionReceipt:prop1:1");
    for node in [
        "finance:OrderAdmissionReceipt:prop1:1",
        "finance:OrderPromotion:prop1",
    ] {
        let read = served
            .call(Method::GetNodeProperties {
                node_id: node.to_string(),
            })
            .await;
        assert!(read.error.is_none() && !format!("{:?}", read.result).contains("Null"), "{node}");
    }
    assert_eq!(served.orders().await, before, "admission creates no order record");
    // A sealed receipt is read-only: re-creating it with other content is refused.
    let overwrite = served
        .call(Method::AddNode {
            node_id: "finance:OrderAdmissionReceipt:prop1:1".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&json!({
                "type": "OrderAdmissionReceipt", "receiptDigest": "sha256:forged"
            }))
            .unwrap(),
        })
        .await;
    assert!(overwrite.error.is_some());
}
