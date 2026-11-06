//! MEDULLA-FINANCE-PRIMITIVES-R004.3.2.1 to R004.3.2.8: the served finance record
//! entry points `FinanceRecordPut` / `FinanceRecordGet` through the real signed
//! `dispatch` surface. Client-writable kinds round-trip every required field;
//! derived kinds refuse a put and leave the prior get unchanged; every kind refuses
//! an unknown schema version, another tenant and an invalid shape.
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

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_types::wire::{FinanceRecord, FinanceRecordKind, FINANCE_V1_SCHEMA_VERSION};

const SECRET: &str = "medulla-fin-r004-3-2-served-records-secret";
const GRAPH: &str = "fin-r004-3-2";
/// The verified tenant of every signed test request (`tests/common/mod.rs`).
const TENANT: &str = "integration-test-tenant";
const COMMON: &[(&str, &str)] = &[
    ("recordSchemaVersion", "\"finance-v1\""),
    ("recordTenant", "\"integration-test-tenant\""),
    ("recordSource", "\"broker-export\""),
    (
        "recordEffectiveAt",
        "\"2026-10-01T00:00:00Z\"^^xsd:dateTime",
    ),
    ("recordKnownAt", "\"2026-10-02T00:00:00Z\"^^xsd:dateTime"),
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

    async fn put(
        &mut self,
        kind: FinanceRecordKind,
        version: &str,
        record: FinanceRecord,
        expected_head: Option<u64>,
    ) -> Response {
        self.call(Method::FinanceRecordPut {
            kind,
            schema_version: version.to_string(),
            record,
            expected_head,
        })
        .await
    }

    async fn get(&mut self, kind: FinanceRecordKind, record_id: &str) -> Response {
        self.call(Method::FinanceRecordGet {
            kind,
            record_id: record_id.to_string(),
        })
        .await
    }

    /// Seed a record node owned by another tenant scope, as an engine-internal
    /// projection of another tenant would be.
    async fn seed_foreign(&mut self, kind: FinanceRecordKind, record_id: &str) {
        let props = serde_json::json!({
            "type": kind.class_name(),
            "finance_tenant_scope": "another-tenant-scope",
            "finance_head": 1,
        });
        let seeded = self
            .call(Method::AddNode {
                node_id: kind.node_id(record_id),
                properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
            })
            .await;
        assert!(seeded.error.is_none(), "seed: {:?}", seeded.error_detail);
    }
}

fn record(record_id: &str, tenant: &str, fields: &[(&str, &str)]) -> FinanceRecord {
    FinanceRecord {
        record_id: record_id.to_string(),
        tenant: tenant.to_string(),
        fields: fields
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn with_common(fields: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    fields.iter().chain(COMMON.iter()).copied().collect()
}

fn refused_with(response: &Response, class: &str, code: &str) {
    assert!(
        response.result.is_none(),
        "expected a refusal, got {response:?}"
    );
    assert_eq!(response.error.as_deref(), Some(class), "{response:?}");
    if !code.is_empty() {
        assert!(
            response
                .error_detail
                .as_deref()
                .is_some_and(|detail| detail.contains(code)),
            "expected {code} in {response:?}"
        );
    }
}

fn stored(response: &Response) -> Option<serde_json::Value> {
    assert!(response.error.is_none(), "get refused: {response:?}");
    match response.result.as_ref() {
        Some(ResultPayload::Raw(bytes)) => Some(rmp_serde::from_slice(bytes).expect("record")),
        _ => None,
    }
}

/// A client-writable kind: put + get round-trip every field, a second put needs
/// the current head, and the version, tenant and shape refusals hold.
async fn client_writable_round_trip(
    kind: FinanceRecordKind,
    fields: &[(&'static str, &'static str)],
    broken: (&'static str, &'static str),
) {
    let mut served = Served::new().await;
    let id = "rec_1";
    let put = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record(id, TENANT, fields),
            None,
        )
        .await;
    assert!(put.error.is_none(), "put: {put:?}");
    assert!(
        matches!(put.result, Some(ResultPayload::Count(1))),
        "{put:?}"
    );
    let got = stored(&served.get(kind, id).await).expect("stored record");
    for (name, value) in fields {
        assert_eq!(got["fields"][*name], *value, "{name}");
    }
    assert_eq!(got["schema_version"], FINANCE_V1_SCHEMA_VERSION);
    assert_eq!(got["tenant"], TENANT);

    // A stale head is a conflict; the current head advances the record.
    let stale = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record(id, TENANT, fields),
            None,
        )
        .await;
    refused_with(&stale, "RECORD_HEAD_CONFLICT", "");
    let next = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record(id, TENANT, fields),
            Some(1),
        )
        .await;
    assert!(
        matches!(next.result, Some(ResultPayload::Count(2))),
        "{next:?}"
    );

    let unknown = served
        .put(kind, "finance-v0", record("rec_2", TENANT, fields), None)
        .await;
    refused_with(&unknown, "FINANCE_RECORD_VERSION_UNKNOWN", "");

    let foreign = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record("rec_3", "another-tenant", fields),
            None,
        )
        .await;
    refused_with(&foreign, "ACCESS_DENIED", "");

    let mut invalid: Vec<(&str, &str)> = fields
        .iter()
        .filter(|(name, _)| *name != broken.0)
        .copied()
        .collect();
    invalid.push(broken);
    let shape = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record("rec_4", TENANT, &invalid),
            None,
        )
        .await;
    refused_with(&shape, "RECORD_SHAPE_INVALID", "");
    assert_eq!(stored(&served.get(kind, "rec_4").await), None);

    served.seed_foreign(kind, "rec_5").await;
    refused_with(&served.get(kind, "rec_5").await, "ACCESS_DENIED", "");
}

/// A derived kind: a put is refused before any write and the prior get is
/// unchanged; the version gate runs first; another tenant's projection is refused.
async fn derived_refusals(kind: FinanceRecordKind, fields: &[(&'static str, &'static str)]) {
    let mut served = Served::new().await;
    let before = stored(&served.get(kind, "rec_1").await);
    let put = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record("rec_1", TENANT, fields),
            None,
        )
        .await;
    refused_with(&put, "RECORD_DERIVED_READONLY", "");
    assert_eq!(stored(&served.get(kind, "rec_1").await), before);

    let unknown = served
        .put(kind, "finance-v0", record("rec_1", TENANT, fields), None)
        .await;
    refused_with(&unknown, "FINANCE_RECORD_VERSION_UNKNOWN", "");

    // An invalid record is still refused as derived: the policy refusal precedes
    // the shape check, so nothing about a derived kind is ever written.
    let invalid = served
        .put(
            kind,
            FINANCE_V1_SCHEMA_VERSION,
            record("rec_1", TENANT, &[]),
            None,
        )
        .await;
    refused_with(&invalid, "RECORD_DERIVED_READONLY", "");

    served.seed_foreign(kind, "rec_2").await;
    refused_with(&served.get(kind, "rec_2").await, "ACCESS_DENIED", "");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.1
async fn account_served_round_trip_and_refusals() {
    client_writable_round_trip(
        FinanceRecordKind::Account,
        &[
            ("accountId", "\"acct-1\""),
            ("baseCurrency", "\"USD\""),
            ("accountKind", "\"taxable\""),
        ],
        ("accountKind", "\"brokerage\""),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.2
async fn activity_served_round_trip_and_refusals() {
    client_writable_round_trip(
        FinanceRecordKind::Activity,
        &[
            ("activityPosition", "<http://example.org/markets#pos1>"),
            ("activityKind", "\"buy\""),
            ("activityQuantity", "\"10\"^^xsd:decimal"),
            (
                "activityTradeDate",
                "\"2026-10-01T14:30:00Z\"^^xsd:dateTime",
            ),
        ],
        ("activityKind", "\"gift\""),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.3
async fn lot_served_round_trip_and_refusals() {
    derived_refusals(
        FinanceRecordKind::Lot,
        &[
            ("lotPosition", "<http://example.org/markets#pos1>"),
            ("lotOpenedBy", "<http://example.org/markets#act1>"),
            ("lotQuantity", "\"10\"^^xsd:decimal"),
            ("lotUnitCostBasis", "\"100.00\"^^xsd:decimal"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.4
async fn position_served_round_trip_and_refusals() {
    derived_refusals(
        FinanceRecordKind::Position,
        &[
            ("positionAccount", "<http://example.org/markets#acct1>"),
            ("positionInstrument", "<http://example.org/markets#btc>"),
            ("positionQuantity", "\"10\"^^xsd:decimal"),
            ("costBasisMethod", "\"fifo\""),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.5
async fn portfolio_group_served_round_trip_and_refusals() {
    client_writable_round_trip(
        FinanceRecordKind::PortfolioGroup,
        &with_common(&[
            ("portfolioGroupName", "\"Retirement\""),
            ("portfolioView", "\"holdings\""),
            ("portfolioMember", "<http://example.org/markets#acct1>"),
        ]),
        ("portfolioView", "\"everything\""),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.6
async fn strategy_spec_served_round_trip_and_refusals() {
    client_writable_round_trip(
        FinanceRecordKind::StrategySpec,
        &with_common(&[
            ("strategySpecId", "\"dca-weekly\""),
            ("strategySpecVersion", "\"1\"^^xsd:integer"),
            ("strategySpecKind", "\"dca_fixed_amount\""),
            (
                "strategySpecDigest",
                "\"sha256:0000000000000000000000000000000000000000000000000000000000000000\"",
            ),
            ("strategySpecUniverse", "<http://example.org/markets#btc>"),
            ("strategySpecCurrency", "\"USD\""),
        ]),
        ("strategySpecDigest", "\"md5:00\""),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.7
async fn strategy_run_served_round_trip_and_refusals() {
    derived_refusals(
        FinanceRecordKind::StrategyRun,
        &with_common(&[
            ("strategyRunStatus", "\"proposed\""),
            (
                "strategyRunInputDigest",
                "\"sha256:0000000000000000000000000000000000000000000000000000000000000000\"",
            ),
        ]),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.8
async fn leverage_terms_served_round_trip_and_refusals() {
    client_writable_round_trip(
        FinanceRecordKind::LeverageTerms,
        &with_common(&[
            ("leverageInstrument", "<http://example.org/markets#btc>"),
            ("leverageProductType", "\"margin\""),
            ("leveragePolicyVersion", "\"us-2026-10\""),
            ("leverageMultiplier", "\"2.0\"^^xsd:decimal"),
            ("leverageCurrency", "\"USD\""),
            ("leverageInitialMargin", "\"5000.00\"^^xsd:decimal"),
            ("leverageMaintenanceMargin", "\"4000.00\"^^xsd:decimal"),
        ]),
        ("leverageMultiplier", "\"0\"^^xsd:decimal"),
    )
    .await;
}

/// MEDULLA-FINANCE-PRIMITIVES-R751.754: the five create-only research kinds. JSON
/// text fields, a stored record with its kernel version, the version gate, the
/// create-only policy and the tenant check.
#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R751.754
async fn research_entries_round_trip_and_refuse() {
    let mut served = Served::new().await;
    let evidence = served
        .call(Method::AddNode {
            node_id: "ev_1".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"type": "Evidence"}))
                .unwrap(),
        })
        .await;
    assert!(evidence.error.is_none(), "{evidence:?}");
    let components = r#"[{"source":"news","score":0.5,"weight":2.0,"evidenceRef":"ev_1"}]"#;
    // (kind, record id, field overrides)
    type Entry<'a> = (FinanceRecordKind, &'a str, Vec<(&'a str, &'a str)>);
    let entries: [Entry<'_>; 5] = [
        (
            FinanceRecordKind::DebateRound,
            "d9_r1",
            vec![
                ("debateId", r#""d9""#),
                ("roundIndex", "1"),
                ("debateSide", r#""bull""#),
                ("debateArgument", r#""rates fall""#),
                ("evidenceRefs", r#"["ev_1"]"#),
            ],
        ),
        (
            FinanceRecordKind::TradeJournalEntry,
            "j_1",
            vec![
                ("journalId", r#""j_1""#),
                ("entryAt", r#""2026-10-01T00:00:00Z""#),
                ("journalRationale", r#""trend""#),
                (
                    "biasAudits",
                    r#"[{"biasKind":"recency","finding":"weighted last week","evidenceRefs":["ev_1"]}]"#,
                ),
            ],
        ),
        (
            FinanceRecordKind::SentimentFusion,
            "s_1",
            vec![("subjectListing", r#""l-1""#), ("components", components)],
        ),
        (
            FinanceRecordKind::GeopoliticalRisk,
            "g_1",
            vec![("geoRegion", r#""emea""#), ("components", components)],
        ),
        (
            FinanceRecordKind::StrategyCard,
            "c_1",
            vec![
                ("cardId", r#""c_1""#),
                ("strategySpecRef", r#""spec-1""#),
                ("licenceClass", r#""open""#),
                ("cardSummary", r#""momentum""#),
            ],
        ),
    ];
    for (kind, id, fields) in entries {
        let put = served
            .put(
                kind,
                FINANCE_V1_SCHEMA_VERSION,
                record(id, TENANT, &fields),
                None,
            )
            .await;
        assert!(put.error.is_none(), "{id}: {put:?}");
        assert!(
            matches!(put.result, Some(ResultPayload::Count(1))),
            "{put:?}"
        );
        let got = stored(&served.get(kind, id).await).expect("stored research record");
        assert_eq!(got["schema_version"], FINANCE_V1_SCHEMA_VERSION);
        assert_eq!(got["tenant"], TENANT);
        assert_eq!(got["research_kernel_version"], "research-1");

        let unknown = served
            .put(kind, "finance-v0", record("rec_x", TENANT, &fields), None)
            .await;
        refused_with(&unknown, "FINANCE_RECORD_VERSION_UNKNOWN", "");
        let foreign = served
            .put(
                kind,
                FINANCE_V1_SCHEMA_VERSION,
                record("rec_y", "another-tenant", &fields),
                None,
            )
            .await;
        refused_with(&foreign, "ACCESS_DENIED", "");
        let again = served
            .put(
                kind,
                FINANCE_V1_SCHEMA_VERSION,
                record(id, TENANT, &fields),
                None,
            )
            .await;
        refused_with(&again, "RECORD_IMMUTABLE", "");
    }
}
