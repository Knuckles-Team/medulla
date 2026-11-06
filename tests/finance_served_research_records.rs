//! MEDULLA-FINANCE-PRIMITIVES-R751.752, R751.753 and R751.755: the strategy research
//! records through the real signed `dispatch` surface. Every invalid record returns
//! its stable code and writes nothing, a debate-to-strategy-card journey stores the
//! fused scores and the `derivedFrom` edges, and one write emits the
//! `medulla.finance.record_write` span.
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
use std::sync::{Arc, Mutex, OnceLock};

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_types::wire::{FinanceRecord, FinanceRecordKind, FINANCE_V1_SCHEMA_VERSION};

const SECRET: &str = "medulla-fin-r751-served-research-records-secret";
const GRAPH: &str = "fin-r751";
/// The verified tenant of every signed test request (`tests/common/mod.rs`).
const TENANT: &str = "integration-test-tenant";

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

    /// Write one plain evidence node; `scope` stamps another tenant's scope on it.
    async fn evidence(&mut self, id: &str, scope: Option<&str>) {
        let mut props = serde_json::json!({"type": "Evidence"});
        if let Some(scope) = scope {
            props["finance_tenant_scope"] = serde_json::json!(scope);
        }
        let written = self
            .call(Method::AddNode {
                node_id: id.to_string(),
                properties_msgpack: test_support::json_bytes(props),
            })
            .await;
        assert!(
            written.error.is_none(),
            "evidence: {:?}",
            written.error_detail
        );
    }

    async fn put(
        &mut self,
        kind: FinanceRecordKind,
        id: &str,
        fields: &[(&str, String)],
    ) -> Response {
        self.put_as(kind, id, TENANT, fields).await
    }

    async fn put_as(
        &mut self,
        kind: FinanceRecordKind,
        id: &str,
        tenant: &str,
        fields: &[(&str, String)],
    ) -> Response {
        self.call(Method::FinanceRecordPut {
            kind,
            schema_version: FINANCE_V1_SCHEMA_VERSION.to_string(),
            record: FinanceRecord {
                record_id: id.to_string(),
                tenant: tenant.to_string(),
                fields: fields
                    .iter()
                    .map(|(name, text)| (name.to_string(), text.clone()))
                    .collect::<BTreeMap<_, _>>(),
            },
            expected_head: None,
        })
        .await
    }

    async fn stored(&mut self, kind: FinanceRecordKind, id: &str) -> Option<serde_json::Value> {
        let got = self
            .call(Method::FinanceRecordGet {
                kind,
                record_id: id.to_string(),
            })
            .await;
        assert!(got.error.is_none(), "get refused: {got:?}");
        match got.result {
            Some(ResultPayload::Raw(bytes)) => Some(rmp_serde::from_slice(&bytes).expect("record")),
            _ => None,
        }
    }
}

fn json(value: serde_json::Value) -> String {
    value.to_string()
}

fn components(rows: &[(f64, f64, &str)]) -> String {
    json(serde_json::Value::Array(
        rows.iter()
            .map(|(score, weight, evidence)| {
                serde_json::json!({
                    "source": "news",
                    "score": score,
                    "weight": weight,
                    "evidenceRef": evidence,
                })
            })
            .collect(),
    ))
}

fn sentiment(rows: &[(f64, f64, &str)]) -> Vec<(&'static str, String)> {
    vec![
        ("subjectListing", json(serde_json::json!("listing-1"))),
        ("components", components(rows)),
    ]
}

fn card(licence: Option<&str>) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("cardId", json(serde_json::json!("card-1"))),
        ("strategySpecRef", json(serde_json::json!("spec-1"))),
        ("cardSummary", json(serde_json::json!("momentum"))),
    ];
    if let Some(licence) = licence {
        fields.push(("licenceClass", json(serde_json::json!(licence))));
    }
    fields
}

fn round(index: u32, side: &str, evidence: &[&str]) -> Vec<(&'static str, String)> {
    vec![
        ("debateId", json(serde_json::json!("d1"))),
        ("roundIndex", json(serde_json::json!(index))),
        ("debateSide", json(serde_json::json!(side))),
        ("debateArgument", json(serde_json::json!("the case"))),
        ("evidenceRefs", json(serde_json::json!(evidence))),
    ]
}

fn refused_with(response: &Response, code: &str) {
    assert!(
        response.result.is_none(),
        "expected a refusal: {response:?}"
    );
    assert_eq!(response.error.as_deref(), Some(code), "{response:?}");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R751.752
async fn each_invalid_record_returns_its_code() {
    use FinanceRecordKind as Kind;
    let mut served = Served::new().await;
    served.evidence("ev_1", None).await;
    served
        .evidence("ev_foreign", Some("another-tenant-scope"))
        .await;

    let missing = served
        .put(
            Kind::SentimentFusion,
            "s_missing",
            &sentiment(&[(0.5, 1.0, "ev_absent")]),
        )
        .await;
    refused_with(&missing, "SCORE_EVIDENCE_MISSING");
    let range = served
        .put(
            Kind::SentimentFusion,
            "s_range",
            &sentiment(&[(2.0, 1.0, "ev_1")]),
        )
        .await;
    refused_with(&range, "SCORE_RANGE_INVALID");
    let weights = served
        .put(
            Kind::SentimentFusion,
            "s_weights",
            &sentiment(&[(0.5, 0.0, "ev_1")]),
        )
        .await;
    refused_with(&weights, "SCORE_WEIGHTS_INVALID");
    let order = served
        .put(Kind::DebateRound, "d1_r2", &round(2, "bear", &["ev_1"]))
        .await;
    refused_with(&order, "DEBATE_ROUND_OUT_OF_ORDER");
    let licence = served
        .put(Kind::StrategyCard, "card_none", &card(None))
        .await;
    refused_with(&licence, "STRATEGY_CARD_LICENCE_MISSING");
    let tenant = served
        .put(
            Kind::GeopoliticalRisk,
            "g_foreign",
            &[
                ("geoRegion", json(serde_json::json!("emea"))),
                ("components", components(&[(0.4, 1.0, "ev_foreign")])),
            ],
        )
        .await;
    refused_with(&tenant, "RECORD_TENANT_MISMATCH");
    let supplied = {
        let mut fields = sentiment(&[(0.5, 1.0, "ev_1")]);
        fields.push(("sentimentScore", json(serde_json::json!(0.9))));
        served
            .put(Kind::SentimentFusion, "s_supplied", &fields)
            .await
    };
    refused_with(&supplied, "RECORD_SHAPE_INVALID");

    let first = served
        .put(Kind::StrategyCard, "card_1", &card(Some("open")))
        .await;
    assert!(first.error.is_none(), "{first:?}");
    let again = served
        .put(Kind::StrategyCard, "card_1", &card(Some("restricted")))
        .await;
    refused_with(&again, "RECORD_IMMUTABLE");

    // Each refusal wrote nothing.
    for (kind, id) in [
        (Kind::SentimentFusion, "s_missing"),
        (Kind::SentimentFusion, "s_range"),
        (Kind::SentimentFusion, "s_weights"),
        (Kind::DebateRound, "d1_r2"),
        (Kind::StrategyCard, "card_none"),
        (Kind::GeopoliticalRisk, "g_foreign"),
        (Kind::SentimentFusion, "s_supplied"),
    ] {
        assert_eq!(served.stored(kind, id).await, None, "{id}");
    }
    let kept = served
        .stored(Kind::StrategyCard, "card_1")
        .await
        .expect("card_1");
    assert_eq!(kept["fields"]["licenceClass"], "open");
}

#[tokio::test(flavor = "multi_thread")]
/// Verifies MEDULLA-FINANCE-PRIMITIVES-R751.753: Hot-path proof
// spec: MEDULLA-FINANCE-PRIMITIVES-R751.753
async fn debate_to_strategy_card_round_trip() {
    use FinanceRecordKind as Kind;
    let mut served = Served::new().await;
    served.evidence("ev_1", None).await;
    served.evidence("ev_2", None).await;

    let one = served
        .put(Kind::DebateRound, "d1_r1", &round(1, "bull", &["ev_1"]))
        .await;
    assert!(one.error.is_none(), "{one:?}");
    let two = served
        .put(Kind::DebateRound, "d1_r2", &round(2, "bear", &["ev_2"]))
        .await;
    assert!(two.error.is_none(), "{two:?}");
    let fused = served
        .put(
            Kind::SentimentFusion,
            "s_1",
            &sentiment(&[(0.5, 1.0, "ev_1"), (-0.5, 3.0, "ev_2")]),
        )
        .await;
    assert!(fused.error.is_none(), "{fused:?}");
    let risk = served
        .put(
            Kind::GeopoliticalRisk,
            "g_1",
            &[
                ("geoRegion", json(serde_json::json!("emea"))),
                (
                    "components",
                    components(&[(0.25, 2.0, "ev_1"), (0.75, 2.0, "ev_2"), (1.0, 4.0, "ev_2")]),
                ),
            ],
        )
        .await;
    assert!(risk.error.is_none(), "{risk:?}");
    let stored_card = served
        .put(Kind::StrategyCard, "card_1", &card(Some("open")))
        .await;
    assert!(stored_card.error.is_none(), "{stored_card:?}");

    let sentiment_back = served
        .stored(Kind::SentimentFusion, "s_1")
        .await
        .expect("s_1");
    assert_eq!(sentiment_back["fields"]["sentimentScore"], -0.25);
    assert_eq!(sentiment_back["research_kernel_version"], "research-1");
    let risk_back = served
        .stored(Kind::GeopoliticalRisk, "g_1")
        .await
        .expect("g_1");
    assert_eq!(risk_back["fields"]["geoRiskScore"], 0.75);
    let round_back = served
        .stored(Kind::DebateRound, "d1_r2")
        .await
        .expect("d1_r2");
    assert_eq!(round_back["fields"]["debateSide"], "bear");
    assert!(served.stored(Kind::StrategyCard, "card_1").await.is_some());

    let edges = served.call(Method::GetEdges).await;
    let derived: Vec<(String, String)> = test_support::edge_rows(&edges)
        .into_iter()
        .map(|(source, target, _)| (source, target))
        .collect();
    for (source, target) in [
        ("finance:DebateRound:d1_r1", "ev_1"),
        ("finance:DebateRound:d1_r2", "ev_2"),
        ("finance:SentimentFusion:s_1", "ev_1"),
        ("finance:SentimentFusion:s_1", "ev_2"),
        ("finance:GeopoliticalRisk:g_1", "ev_2"),
    ] {
        assert!(
            derived.contains(&(source.to_string(), target.to_string())),
            "missing derivedFrom {source} -> {target}: {derived:?}"
        );
    }

    // One cross-tenant denial: a record naming another tenant is refused.
    let foreign = served
        .put_as(
            Kind::StrategyCard,
            "card_2",
            "another-tenant",
            &card(Some("open")),
        )
        .await;
    refused_with(&foreign, "ACCESS_DENIED");
}

/// Captured tracing output; the subscriber is process-global because requests run
/// on the runtime's worker threads.
#[derive(Clone, Default)]
struct SpanLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SpanLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn span_log() -> &'static SpanLog {
    static LOG: OnceLock<SpanLog> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = SpanLog::default();
        let writer = log.clone();
        let _ = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_max_level(tracing_subscriber::filter::LevelFilter::INFO)
            .try_init();
        log
    })
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R751.755
async fn write_span_and_edges_are_recorded() {
    use FinanceRecordKind as Kind;
    let log = span_log();
    let mut served = Served::new().await;
    served.evidence("ev_1", None).await;
    let written = served
        .put(
            Kind::SentimentFusion,
            "s_span",
            &sentiment(&[(0.5, 1.0, "ev_1")]),
        )
        .await;
    assert!(written.error.is_none(), "{written:?}");

    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    let line = text
        .lines()
        .find(|line| line.contains("medulla.finance.record_write") && line.contains("s_span"))
        .unwrap_or_else(|| panic!("no record_write span for s_span in:\n{text}"));
    assert!(line.contains(TENANT), "{line}");
    assert!(line.contains("SentimentFusion"), "{line}");
    assert!(line.contains("committed"), "{line}");

    let back = served
        .stored(Kind::SentimentFusion, "s_span")
        .await
        .expect("s_span");
    assert_eq!(back["research_kernel_version"], "research-1");
    assert_eq!(back["evidence_ids"], serde_json::json!(["ev_1"]));
    let edges = test_support::edge_rows(&served.call(Method::GetEdges).await);
    assert!(edges
        .iter()
        .any(|(source, target, _)| source == "finance:SentimentFusion:s_span" && target == "ev_1"));
}
