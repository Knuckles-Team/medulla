//! MEDULLA-FINANCE-PRIMITIVES-R011.2.1 to R011.3: the served `FinanceAlertRaise` door
//! through the real signed `dispatch` surface. Each alert commits its event node
//! and its `finance.events.v1` outbox fact in one owner transaction; an equal raise
//! replays the first result; a refusal leaves neither record.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "finance",
    feature = "redb"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_transaction::OutboxClaimBudget;
use medulla_types::finance_event_envelope::{FinanceAlertEnvelope, FinanceAlertKind};

const SECRET: &str = "medulla-fin-r011-2-served-alert-secret";
const GRAPH: &str = "fin-r011-2";
const TOPIC: &str = "finance.events.v1";
const KINDS: [(FinanceAlertKind, &str); 4] = [
    (FinanceAlertKind::Price, "finance.alert.price.v1"),
    (FinanceAlertKind::TrendFlip, "finance.alert.trend_flip.v1"),
    (FinanceAlertKind::DcaDue, "finance.alert.dca_due.v1"),
    (
        FinanceAlertKind::MarginThreshold,
        "finance.alert.margin_threshold.v1",
    ),
];

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
        assert!(
            created.error.is_none(),
            "CreateGraph: {:?}",
            created.error_detail
        );
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

    async fn raise(&mut self, envelope: FinanceAlertEnvelope) -> Response {
        self.call(Method::FinanceAlertRaise { envelope }).await
    }

    async fn node(&mut self, node_id: &str) -> Option<serde_json::Value> {
        let response = self
            .call(Method::GetNodeProperties {
                node_id: node_id.to_string(),
            })
            .await;
        assert!(response.error.is_none(), "get refused: {response:?}");
        match response.result.as_ref() {
            Some(ResultPayload::Raw(bytes)) => {
                rmp_serde::from_slice::<Option<serde_json::Value>>(bytes).expect("node")
            }
            _ => None,
        }
    }

    /// Every outbox fact on `finance.events.v1`, claimed by the consumer `queue`:
    /// `(key, schema header)` in claim order. The consumer name is the queue
    /// `finance.alert.<kind>` bound to the `schema` header.
    async fn claim(&self, queue: &str) -> Vec<(String, String)> {
        let persistence = self
            .state
            .read()
            .await
            .persistence
            .clone()
            .expect("durable persistence");
        let graph = medulla_kernel::persist::sanitize(GRAPH);
        persistence
            .subscribe_mutation_outbox(&graph, queue, TOPIC)
            .await
            .expect("subscribe");
        let mut budget = OutboxClaimBudget::new(64, 5_000, 1).expect("budget");
        let outcome = persistence
            .claim_mutation_outbox(&graph, queue, &mut budget)
            .await
            .expect("claim");
        outcome
            .claims
            .into_iter()
            .filter(|lease| lease.record.intent.topic == TOPIC)
            .map(|lease| {
                (
                    lease.record.intent.key.clone(),
                    lease
                        .record
                        .intent
                        .headers
                        .get("schema")
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect()
    }
}

fn envelope(kind: FinanceAlertKind, id: &str) -> FinanceAlertEnvelope {
    FinanceAlertEnvelope {
        event_id: id.to_string(),
        kind,
        listing_id: "NASDAQ:AAPL".to_string(),
        occurred_at_unix_nanos: 1_760_000_000_000_000_000,
        message: "alert".to_string(),
    }
}

fn refused_with(response: &Response, code: &str) {
    assert!(
        response.result.is_none(),
        "expected a refusal, got {response:?}"
    );
    assert!(
        response
            .error_detail
            .as_deref()
            .is_some_and(|d| d.contains(code))
            || response.error.as_deref().is_some_and(|e| e.contains(code)),
        "expected {code} in {response:?}"
    );
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.2.1
#[tokio::test(flavor = "multi_thread")]
async fn intent_reaches_outbox_with_flip_identity() {
    let mut served = Served::new().await;
    let flip = "finance.flip:AAPL:2026-10-09";
    let raised = served
        .raise(envelope(FinanceAlertKind::TrendFlip, flip))
        .await;
    assert!(raised.error.is_none(), "{:?}", raised.error_detail);
    let facts = served.claim("finance.alert.trend_flip").await;
    assert_eq!(
        facts,
        vec![(flip.to_string(), "finance.alert.trend_flip.v1".to_string())]
    );
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.2.3
#[tokio::test(flavor = "multi_thread")]
async fn alert_door_accepts_four_kinds() {
    let mut served = Served::new().await;
    for (kind, _) in KINDS {
        let id = format!("alert-{kind:?}");
        let raised = served.raise(envelope(kind, &id)).await;
        assert!(
            raised.error.is_none(),
            "{kind:?}: {:?}",
            raised.error_detail
        );
        let node = served.node(&id).await.expect("event node");
        assert_eq!(node["event_id"], id);
    }
    let facts = served.claim("finance.alert.price").await;
    assert_eq!(facts.len(), 4);
    for (kind, schema) in KINDS {
        let id = format!("alert-{kind:?}");
        assert!(facts.contains(&(id, schema.to_string())), "{schema}");
    }
    // An invalid envelope refuses and writes nothing.
    let mut invalid = envelope(FinanceAlertKind::Price, "alert-invalid");
    invalid.message.clear();
    refused_with(&served.raise(invalid).await, "ALERT_ENVELOPE_INVALID");
    assert!(served.node("alert-invalid").await.is_none());
    // Another tenant's node under the same id is never overwritten.
    let foreign = serde_json::json!({
        "type": "FinanceAlert",
        "finance_tenant_scope": "another-tenant-scope",
    });
    let seeded = served
        .call(Method::AddNode {
            node_id: "alert-foreign".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&foreign).unwrap(),
        })
        .await;
    assert!(seeded.error.is_none(), "{:?}", seeded.error_detail);
    refused_with(
        &served
            .raise(envelope(FinanceAlertKind::Price, "alert-foreign"))
            .await,
        "ACCESS_DENIED",
    );
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.2.2
#[tokio::test(flavor = "multi_thread")]
async fn alert_and_outbox_commit_atomically() {
    let mut served = Served::new().await;
    let committed = served
        .raise(envelope(FinanceAlertKind::Price, "alert-atomic"))
        .await;
    assert!(committed.error.is_none(), "{:?}", committed.error_detail);
    // A conflicting revision under the same id refuses and publishes neither a
    // second node revision nor a second outbox fact.
    let mut changed = envelope(FinanceAlertKind::Price, "alert-atomic");
    changed.message = "a different revision".to_string();
    refused_with(&served.raise(changed).await, "ALERT_REVISION_CONFLICT");
    let node = served.node("alert-atomic").await.expect("event node");
    assert_eq!(node["message"], "alert");
    let facts = served.claim("finance.alert.price").await;
    assert_eq!(
        facts,
        vec![(
            "alert-atomic".to_string(),
            "finance.alert.price.v1".to_string()
        )]
    );
    // A refused raise leaves neither record.
    let mut invalid = envelope(FinanceAlertKind::DcaDue, "alert-atomic-refused");
    invalid.listing_id.clear();
    refused_with(&served.raise(invalid).await, "ALERT_ENVELOPE_INVALID");
    assert!(served.node("alert-atomic-refused").await.is_none());
    assert!(served
        .claim("finance.alert.dca_due")
        .await
        .iter()
        .all(|(key, _)| key != "alert-atomic-refused"));
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.3
#[tokio::test(flavor = "multi_thread")]
async fn replay_preserves_one_outbox_fact() {
    let mut served = Served::new().await;
    for (kind, _) in KINDS {
        let id = format!("replay-{kind:?}");
        let first = served.raise(envelope(kind, &id)).await;
        assert!(first.error.is_none(), "{:?}", first.error_detail);
        let second = served.raise(envelope(kind, &id)).await;
        assert!(second.error.is_none(), "{:?}", second.error_detail);
        assert_eq!(first.result, second.result, "{kind:?}");
    }
    let facts = served.claim("finance.alert.price").await;
    assert_eq!(facts.len(), 4, "one outbox fact per event id: {facts:?}");
}
