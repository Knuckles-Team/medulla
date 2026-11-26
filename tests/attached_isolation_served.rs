//! Per-source quota metering and fault isolation (MEDULLA-UNIFIED-DATA-PLANE-R031.2): one source
//! that hangs, one that fails and one that breaches its row quota each refuse with their own
//! typed code, and a healthy source registered beside them keeps answering within its quota.
//! Everything runs through the served `RegisterForeignSource` and `UnifiedQuery` methods.
#![cfg(all(feature = "federation-sql", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;
use std::time::Duration;

use medulla_query_types::wire::{Op, Plan};
use medulla_exec::rowset::RowSet;
use medulla_federation::federation_opt::mcp::{
    register_connector_row_source, ConnectorRowRequest, ConnectorRowSource, ConnectorRows,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_catalog_types::consolidation_isolation::SourceQuota;
use medulla_query_types::wire::{ForeignOperationCapabilities, ForeignSourceSpec, HttpFieldMap};

const SECRET: &str = "attached-isolation-secret";

/// A connector port whose `hung` tool sleeps for the whole time budget it was given and then
/// fails, whose `big` tool answers five rows and whose `ok` tool answers three.
struct Port;

impl ConnectorRowSource for Port {
    fn declared_arguments(&self, _server: &str, _tool: &str) -> Option<Vec<String>> {
        Some(vec!["keys".into(), "limit".into()])
    }

    fn rows(&self, request: &ConnectorRowRequest) -> Result<ConnectorRows, String> {
        let rows = |ids: &[&str]| ConnectorRows {
            source_label: request.source_label.clone(),
            rows: RowSet::from_ids(ids.iter().map(|id| id.to_string())),
            truncated: false,
            missing_source: None,
        };
        match request.server.as_str() {
            "hung" => {
                std::thread::sleep(Duration::from_millis(request.budget.timeout_ms));
                Err("the connector did not answer in time".into())
            }
            "big" => Ok(rows(&["b1", "b2", "b3", "b4", "b5"])),
            _ => Ok(rows(&["ok1", "ok2", "ok3"])),
        }
    }
}

fn mcp(server: &str) -> ForeignSourceSpec {
    ForeignSourceSpec::Mcp {
        server: server.into(),
        tool: "list".into(),
        capabilities: ForeignOperationCapabilities::default(),
    }
}

async fn register(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    source: ForeignSourceSpec,
    quota: Option<SourceQuota>,
) -> Response {
    let method = Method::RegisterForeignSource {
        name: name.into(),
        source,
        quota,
    };
    test_support::dispatch(state, test_support::commons_request(SECRET, id, method)).await
}

async fn read(state: &test_support::SharedState, id: u64, name: &str) -> Response {
    let method = Method::UnifiedQuery {
        plan: Plan::new(vec![Op::Foreign { name: name.into() }]),
        foreign_execution: None,
        foreign_executions: Default::default(),
        code_map: None,
        retrieval: None,
        latency_class: Default::default(),
    };
    test_support::dispatch(state, test_support::commons_request(SECRET, id, method)).await
}

fn ids(response: &Response) -> Vec<String> {
    let Some(ResultPayload::Raw(bytes)) = &response.result else {
        panic!("expected rows, got {:?}", response.refusal_text());
    };
    let rows: Vec<(String, Option<f32>)> = rmp_serde::from_slice(bytes).unwrap();
    let mut ids: Vec<String> = rows.into_iter().map(|(id, _)| id).collect();
    ids.sort();
    ids
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R031.2
#[tokio::test]
async fn quota_breach_and_failure_do_not_degrade_other_source() {
    let _allow =
        medulla_server_api::test_support::FederationAllowGuard::acquire("127.0.0.1:9").await;
    register_connector_row_source(Arc::new(Port));
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let quota = |max_concurrent, max_rows, max_wall_ms| SourceQuota {
        max_concurrent,
        max_rows,
        max_wall_ms,
    };

    // A quota outside its bounds is refused at registration with its own code.
    let refused = register(&state, 1, "bad", mcp("ok"), Some(quota(0, 10, 1_000))).await;
    assert!(
        refused
            .refusal_text()
            .is_some_and(|text| text.contains("SOURCE_QUOTA_INVALID")),
        "{:?}",
        refused.refusal_text()
    );

    // Source A hangs past its wall; C is refused at connect; D answers more rows than its
    // quota; B is healthy and registered without a quota, so it takes the knob defaults.
    let a = register(&state, 2, "a_hung", mcp("hung"), Some(quota(2, 100, 300))).await;
    let c = ForeignSourceSpec::HttpJson {
        url: "http://127.0.0.1:9/down".into(),
        json_path: "data".into(),
        field_map: HttpFieldMap {
            id: "id".into(),
            score: None,
            columns: Default::default(),
        },
    };
    let c = register(&state, 3, "c_down", c, Some(quota(2, 100, 5_000))).await;
    let d = register(&state, 4, "d_big", mcp("big"), Some(quota(2, 2, 5_000))).await;
    let b = register(&state, 5, "b_ok", mcp("ok"), None).await;
    for response in [&a, &c, &d, &b] {
        assert!(response.error.is_none(), "{:?}", response.refusal_text());
    }

    let hung = read(&state, 6, "a_hung").await;
    let text = hung.refusal_text().expect("a hung source must refuse");
    assert!(text.contains("SOURCE_QUOTA_EXCEEDED"), "{text}");
    assert!(text.contains("max_wall_ms"), "{text}");

    let down = read(&state, 7, "c_down").await;
    let text = down.refusal_text().expect("a failing source must refuse");
    assert!(text.contains("SOURCE_UNAVAILABLE"), "{text}");

    let big = read(&state, 8, "d_big").await;
    let text = big.refusal_text().expect("a row breach must refuse");
    assert!(text.contains("SOURCE_QUOTA_EXCEEDED"), "{text}");
    assert!(text.contains("max_rows"), "{text}");

    // B is untouched by the other three and answers within its own quota, repeatedly.
    for id in 9..12 {
        assert_eq!(ids(&read(&state, id, "b_ok").await), ["ok1", "ok2", "ok3"]);
    }
}
