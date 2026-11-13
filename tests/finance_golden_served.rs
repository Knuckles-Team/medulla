//! MEDULLA-FINANCE-PRIMITIVES-R013.2.6 and the R013.2 golden families: the served
//! `FinanceAccountProject` projection through the real signed `dispatch` surface.
//! Every figure carries source, as-of and session; each family replays the vectors
//! of `tests/fixtures/finance_golden/<family>/` and compares exact ticks against
//! expected values computed here by an independent integer oracle (or typed from
//! the cited published rule), never by the code under test.
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

use medulla_finance::golden_fixture::{compare_broker_statement_figure, GoldenFixtureFigure};
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_types::wire::{FinanceRecord, FinanceRecordKind, FINANCE_V1_SCHEMA_VERSION};
use serde_json::Value;
use sha2::{Digest, Sha256};

const SECRET: &str = "medulla-fin-golden-served-secret";
const TENANT: &str = "integration-test-tenant";
const SCALE: i128 = 100_000_000;

fn fixture(family: &str, file: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/finance_golden/{family}/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The manifest names its source, version, as-of, session, currency, scale and
/// rounding, and its digest is computed over the vectors file now.
fn load(family: &str) -> Value {
    let manifest: Value = serde_json::from_str(&fixture(family, "manifest.json")).unwrap();
    let vectors = fixture(family, "vectors.json");
    assert_eq!(
        manifest["sha256"].as_str().unwrap(),
        hex::encode(Sha256::digest(vectors.as_bytes())),
        "{family}: vectors digest"
    );
    for key in [
        "source_url",
        "version",
        "as_of",
        "session",
        "currency",
        "rounding",
    ] {
        assert!(
            manifest[key].as_str().is_some_and(|v| !v.is_empty()),
            "{family}: manifest {key}"
        );
    }
    assert_eq!(manifest["scale"], 100_000_000);
    assert_eq!(manifest["currency"], "USD");
    serde_json::from_str(&vectors).unwrap()
}

/// Decimal text as ticks, by integer arithmetic.
fn ticks(text: &str) -> i128 {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let fraction = format!("{fraction:0<8}");
    whole.parse::<i128>().unwrap() * SCALE + fraction.parse::<i128>().unwrap()
}

struct Served {
    state: test_support::SharedState,
    next_id: u64,
}

impl Served {
    async fn new(graph: &str) -> Self {
        common::configure_authority();
        let state = test_support::durable_state(SECRET, common::current_isolation());
        let mut served = Self { state, next_id: 1 };
        let created = served
            .call(
                graph,
                Method::CreateGraph {
                    graph_name: graph.to_string(),
                    graph_type: GraphType::Global,
                },
            )
            .await;
        assert!(created.error.is_none(), "{:?}", created.error_detail);
        served
    }

    async fn call(&mut self, graph: &str, method: Method) -> Response {
        self.next_id += 1;
        test_support::dispatch(
            &self.state,
            test_support::request(SECRET, self.next_id, graph, method),
        )
        .await
    }

    async fn put(
        &mut self,
        graph: &str,
        kind: FinanceRecordKind,
        id: &str,
        fields: Vec<(&str, String)>,
    ) {
        let record = FinanceRecord {
            record_id: id.to_string(),
            tenant: TENANT.to_string(),
            fields: fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect::<BTreeMap<_, _>>(),
        };
        let response = self
            .call(
                graph,
                Method::FinanceRecordPut {
                    kind,
                    schema_version: FINANCE_V1_SCHEMA_VERSION.to_string(),
                    record,
                    expected_head: None,
                },
            )
            .await;
        assert!(
            response.error.is_none(),
            "put {id}: {:?}",
            response.error_detail
        );
    }

    async fn account(&mut self, graph: &str, calendar: Option<&Value>) {
        let mut fields = vec![
            ("accountId", "\"acct-1\"".to_string()),
            ("baseCurrency", "\"USD\"".to_string()),
            ("accountKind", "\"taxable\"".to_string()),
        ];
        if let Some(calendar) = calendar {
            let json = serde_json::to_string(calendar)
                .unwrap()
                .replace('"', "\\\"");
            fields.push(("accountCalendar", format!("\"{json}\"")));
        }
        self.put(graph, FinanceRecordKind::Account, "acct-1", fields)
            .await;
    }

    async fn project(
        &mut self,
        graph: &str,
        as_of: &str,
        known_at: &str,
        policy: &str,
    ) -> Response {
        self.call(
            graph,
            Method::FinanceAccountProject {
                account: "acct-1".into(),
                as_of: as_of.into(),
                known_at: known_at.into(),
                basis_policy: policy.into(),
                base_currency: "USD".into(),
            },
        )
        .await
    }
}

fn dt(text: &str) -> String {
    format!("\"{text}\"^^xsd:dateTime")
}

fn dec(text: &str) -> String {
    format!("\"{text}\"^^xsd:decimal")
}

fn int(value: i64) -> String {
    format!("\"{value}\"^^xsd:integer")
}

/// The base fields of one activity of the account on `position`.
fn activity(position: &str, kind: &str, at: &str, quantity: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "activityPosition",
            format!("<http://example.org/markets#{position}>"),
        ),
        (
            "activityAccount",
            "<http://example.org/markets#acct-1>".to_string(),
        ),
        ("activityKind", format!("\"{kind}\"")),
        ("activityQuantity", dec(quantity)),
        ("activityTradeDate", dt(at)),
        ("activityCurrency", "\"USD\"".to_string()),
        ("recordSource", "\"golden-fixture\"".to_string()),
        ("activitySession", "\"regular\"".to_string()),
    ]
}

fn body(response: &Response) -> Value {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => value.clone(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!(
            "expected a projection, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

fn figure(projection: &Value, position: &str, name: &str) -> Value {
    projection["figures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["position"] == position && f["name"] == name)
        .unwrap_or_else(|| panic!("no figure {name} of {position} in {projection}"))
        .clone()
}

fn assert_provenance(projection: &Value) {
    let figures = projection["figures"].as_array().unwrap();
    assert!(!figures.is_empty());
    for f in figures {
        for key in ["source", "as_of", "session"] {
            assert!(
                f[key].as_str().is_some_and(|v| !v.is_empty()),
                "{key} of {f}"
            );
        }
    }
}

fn value(projection: &Value, position: &str, name: &str) -> i128 {
    figure(projection, position, name)["value_ticks"]
        .as_i64()
        .unwrap()
        .into()
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.6
async fn projection_returns_provenanced_figures() {
    let graph = "fin-golden-project";
    let mut served = Served::new(graph).await;
    served.account(graph, None).await;
    // Two buys and one sell of the same position, FIFO.
    for (id, kind, at, qty, price) in [
        ("a1", "buy", "2026-01-05T15:00:00Z", "10", "100"),
        ("a2", "buy", "2026-01-06T15:00:00Z", "10", "120"),
        ("a3", "sell", "2026-01-07T15:00:00Z", "15", "130"),
    ] {
        let mut fields = activity("ACME", kind, at, qty);
        fields.push(("activityPrice", dec(price)));
        served
            .put(graph, FinanceRecordKind::Activity, id, fields)
            .await;
    }
    let response = served
        .project(
            graph,
            "2026-02-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
            "fifo",
        )
        .await;
    let projection = body(&response);
    assert_provenance(&projection);
    assert_eq!(projection["scale"], 100_000_000);
    // Independent oracle: FIFO closes 10@100 then 5@120 at 130.
    let realized = (10 * (130 - 100) + 5 * (130 - 120)) * SCALE;
    assert_eq!(value(&projection, "ACME", "realized_gain"), realized);
    assert_eq!(value(&projection, "ACME", "open_quantity"), 5 * SCALE);
    assert_eq!(value(&projection, "ACME", "cost_basis"), 5 * 120 * SCALE);
    assert_eq!(value(&projection, "ACME", "market_value"), 5 * 130 * SCALE);
    assert_eq!(
        figure(&projection, "ACME", "cost_basis")["source"],
        "golden-fixture"
    );
    // LIFO closes the newest lot first: 10@120 then 5@100.
    let lifo = body(
        &served
            .project(
                graph,
                "2026-02-01T00:00:00Z",
                "2026-02-01T00:00:00Z",
                "lifo",
            )
            .await,
    );
    assert_eq!(
        value(&lifo, "ACME", "realized_gain"),
        (10 * 10 + 5 * 30) * SCALE
    );
    // The replay is byte-identical.
    let again = body(
        &served
            .project(
                graph,
                "2026-02-01T00:00:00Z",
                "2026-02-01T00:00:00Z",
                "fifo",
            )
            .await,
    );
    assert_eq!(
        serde_json::to_vec(&projection).unwrap(),
        serde_json::to_vec(&again).unwrap()
    );
    // An as-of before the sell shows the earlier state.
    let early = body(
        &served
            .project(
                graph,
                "2026-01-06T16:00:00Z",
                "2026-02-01T00:00:00Z",
                "fifo",
            )
            .await,
    );
    assert_eq!(value(&early, "ACME", "open_quantity"), 20 * SCALE);

    // Refusals: unsupported basis policy; a figure without a source, a session
    // or an as-of.
    let policy = served
        .project(
            graph,
            "2026-02-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
            "average_cost",
        )
        .await;
    assert_eq!(
        policy.error.as_deref(),
        Some("BASIS_POLICY_UNSUPPORTED"),
        "{policy:?}"
    );
    let blank = served
        .project(graph, "", "2026-02-01T00:00:00Z", "fifo")
        .await;
    assert_eq!(
        blank.error.as_deref(),
        Some("FIGURE_PROVENANCE_MISSING"),
        "{blank:?}"
    );

    let graph = "fin-golden-project-gaps";
    let mut gaps = Served::new(graph).await;
    gaps.account(graph, None).await;
    let mut no_source = activity("NOSRC", "buy", "2026-01-05T15:00:00Z", "1");
    no_source.retain(|(name, _)| *name != "recordSource");
    no_source.push(("activityPrice", dec("1")));
    gaps.put(graph, FinanceRecordKind::Activity, "n1", no_source)
        .await;
    let refused = gaps
        .project(
            graph,
            "2026-02-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
            "fifo",
        )
        .await;
    assert_eq!(
        refused.error.as_deref(),
        Some("FIGURE_PROVENANCE_MISSING"),
        "{refused:?}"
    );
    assert!(refused.error_detail.as_deref().unwrap().contains("source"));

    let graph = "fin-golden-project-nosession";
    let mut nosession = Served::new(graph).await;
    nosession.account(graph, None).await;
    let mut fields = activity("NOSES", "buy", "2026-01-05T15:00:00Z", "1");
    fields.retain(|(name, _)| *name != "activitySession");
    fields.push(("activityPrice", dec("1")));
    nosession
        .put(graph, FinanceRecordKind::Activity, "s1", fields)
        .await;
    let refused = nosession
        .project(
            graph,
            "2026-02-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
            "fifo",
        )
        .await;
    assert_eq!(
        refused.error.as_deref(),
        Some("FIGURE_PROVENANCE_MISSING"),
        "{refused:?}"
    );
    assert!(refused.error_detail.as_deref().unwrap().contains("session"));
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.2
async fn corporate_action_matches_published_reference() {
    let v = load("corporate_action");
    let graph = "fin-golden-corporate";
    let mut served = Served::new(graph).await;
    served.account(graph, None).await;

    let buy = &v["buy"];
    let mut fields = activity(
        "ACME",
        "buy",
        buy["at"].as_str().unwrap(),
        buy["quantity"].as_str().unwrap(),
    );
    fields.push(("activityPrice", dec(buy["price"].as_str().unwrap())));
    served
        .put(graph, FinanceRecordKind::Activity, "buy", fields)
        .await;

    let split = &v["split"];
    let mut fields = activity("ACME", "split", split["effective"].as_str().unwrap(), "0");
    fields.push(("activitySplitFrom", int(split["from"].as_i64().unwrap())));
    fields.push(("activitySplitTo", int(split["to"].as_i64().unwrap())));
    fields.push(("activityRevision", int(0)));
    fields.push(("recordKnownAt", dt(split["known_at"].as_str().unwrap())));
    served
        .put(graph, FinanceRecordKind::Activity, "split0", fields)
        .await;

    let fix = &v["split_correction"];
    let mut fields = activity("ACME", "split", split["effective"].as_str().unwrap(), "0");
    fields.push(("activitySplitFrom", int(split["from"].as_i64().unwrap())));
    fields.push(("activitySplitTo", int(fix["to"].as_i64().unwrap())));
    fields.push(("activityRevision", int(fix["revision"].as_i64().unwrap())));
    fields.push(("recordKnownAt", dt(fix["known_at"].as_str().unwrap())));
    served
        .put(graph, FinanceRecordKind::Activity, "split1", fields)
        .await;

    let div = &v["dividend"];
    let mut fields = activity("ACME", "dividend", div["effective"].as_str().unwrap(), "0");
    fields.push((
        "activityDividendPerShare",
        dec(div["per_share"].as_str().unwrap()),
    ));
    fields.push(("recordKnownAt", dt(div["known_at"].as_str().unwrap())));
    served
        .put(graph, FinanceRecordKind::Activity, "div", fields)
        .await;

    let as_of = v["as_of"].as_str().unwrap();
    // Independent oracle of the split and dividend formulas.
    let (qty0, price0) = (
        ticks(buy["quantity"].as_str().unwrap()),
        ticks(buy["price"].as_str().unwrap()),
    );
    let basis = qty0 * price0 / SCALE;
    let per_share = ticks(div["per_share"].as_str().unwrap());
    for (known, ratio_to) in [
        (
            v["known_before_correction"].as_str().unwrap(),
            split["to"].as_i64().unwrap(),
        ),
        (
            v["known_after_correction"].as_str().unwrap(),
            fix["to"].as_i64().unwrap(),
        ),
    ] {
        let ratio_from = split["from"].as_i64().unwrap() as i128;
        let qty = qty0 * ratio_to as i128 / ratio_from;
        let adjusted_price = price0 * ratio_from / ratio_to as i128;
        let projection = body(&served.project(graph, as_of, known, "fifo").await);
        assert_provenance(&projection);
        assert_eq!(value(&projection, "ACME", "open_quantity"), qty, "{known}");
        assert_eq!(value(&projection, "ACME", "cost_basis"), basis, "{known}");
        assert_eq!(
            value(&projection, "ACME", "dividend_cash"),
            qty * per_share / SCALE,
            "{known}"
        );
        assert_eq!(
            value(&projection, "ACME", "market_value"),
            qty * adjusted_price / SCALE,
            "{known}"
        );
        // A mutated reference fails the comparison; a missing field fails admission.
        let computed = figure(&projection, "ACME", "open_quantity");
        let golden = GoldenFixtureFigure {
            value_ticks: computed["value_ticks"].as_i64().unwrap(),
            source: computed["source"].as_str().unwrap().into(),
            as_of: computed["as_of"].as_str().unwrap().into(),
            session: computed["session"].as_str().unwrap().into(),
        };
        assert!(compare_broker_statement_figure(&golden, qty as i64).is_ok());
        assert!(compare_broker_statement_figure(&golden, qty as i64 + 1).is_err());
        let mut no_session = golden.clone();
        no_session.session.clear();
        assert!(compare_broker_statement_figure(&no_session, qty as i64).is_err());
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.3
async fn dst_session_matches_published_reference() {
    let v = load("dst_session");
    let graph = "fin-golden-dst";
    let mut served = Served::new(graph).await;
    served.account(graph, Some(&v["calendar"])).await;
    let boundaries = v["boundaries"].as_array().unwrap();
    for (index, boundary) in boundaries.iter().enumerate() {
        let mut fields = activity(
            &format!("B{index:02}"),
            "buy",
            boundary["at"].as_str().unwrap(),
            "1",
        );
        fields.retain(|(name, _)| *name != "activitySession");
        fields.push(("activityPrice", dec("1")));
        served
            .put(
                graph,
                FinanceRecordKind::Activity,
                &format!("b{index:02}"),
                fields,
            )
            .await;
    }
    let as_of = v["as_of"].as_str().unwrap();
    let projection = body(&served.project(graph, as_of, as_of, "fifo").await);
    assert_provenance(&projection);
    for (index, boundary) in boundaries.iter().enumerate() {
        let f = figure(&projection, &format!("B{index:02}"), "open_quantity");
        assert_eq!(f["session"], boundary["label"], "{}", boundary["at"]);
        assert_eq!(f["value_ticks"].as_i64().unwrap() as i128, SCALE);
    }
    // A changed reference label no longer matches.
    let changed = figure(&projection, "B00", "open_quantity");
    assert_ne!(changed["session"], "regular");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.5
async fn futures_roll_matches_published_reference() {
    let v = load("futures_roll");
    let graph = "fin-golden-roll";
    let mut served = Served::new(graph).await;
    served.account(graph, None).await;
    let (e, s) = (&v["expiring"], &v["successor"]);
    let tick = v["tick_size"].as_str().unwrap();
    let mut fields = activity(
        "ESZ6",
        "transfer_out",
        v["roll_at"].as_str().unwrap(),
        v["quantity"].as_str().unwrap(),
    );
    fields.push((
        "activityRollSuccessor",
        "<http://example.org/markets#ESH7>".to_string(),
    ));
    fields.push(("activitySettlement", dec(e["settlement"].as_str().unwrap())));
    fields.push(("activityExpiry", dt(e["expiry"].as_str().unwrap())));
    fields.push(("activityTickSize", dec(tick)));
    fields.push(("activityMultiplier", int(v["multiplier"].as_i64().unwrap())));
    fields.push((
        "activitySuccessorSettlement",
        dec(s["settlement"].as_str().unwrap()),
    ));
    fields.push(("activitySuccessorExpiry", dt(s["expiry"].as_str().unwrap())));
    fields.push(("activitySuccessorTickSize", dec(tick)));
    fields.push((
        "activitySuccessorMultiplier",
        int(v["multiplier"].as_i64().unwrap()),
    ));
    served
        .put(graph, FinanceRecordKind::Activity, "roll", fields)
        .await;
    let as_of = v["as_of"].as_str().unwrap();
    let projection = body(&served.project(graph, as_of, as_of, "fifo").await);
    assert_provenance(&projection);
    // Independent oracle: quantity x multiplier x settlement difference.
    let qty = ticks(v["quantity"].as_str().unwrap());
    let mult = v["multiplier"].as_i64().unwrap() as i128;
    let (se, ss) = (
        ticks(e["settlement"].as_str().unwrap()),
        ticks(s["settlement"].as_str().unwrap()),
    );
    assert_eq!(
        value(&projection, "ESZ6", "roll_cash"),
        qty * mult * (se - ss) / SCALE
    );
    assert_eq!(
        value(&projection, "ESZ6", "roll_position_value"),
        qty * mult * ss / SCALE
    );
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R013
async fn every_figure_carries_provenance() {
    let graph = "fin-golden-every";
    let mut served = Served::new(graph).await;
    served.account(graph, None).await;
    let mut fields = activity("ACME", "buy", "2026-01-05T15:00:00Z", "3");
    fields.push(("activityPrice", dec("10")));
    served
        .put(graph, FinanceRecordKind::Activity, "e1", fields)
        .await;
    let projection = body(
        &served
            .project(
                graph,
                "2026-02-01T00:00:00Z",
                "2026-02-01T00:00:00Z",
                "lifo",
            )
            .await,
    );
    assert_provenance(&projection);
    assert_eq!(projection["figures"].as_array().unwrap().len(), 5);
}
