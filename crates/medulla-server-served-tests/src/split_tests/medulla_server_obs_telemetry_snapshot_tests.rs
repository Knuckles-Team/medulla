//! MEDULLA-ORCH-VOCAB-R804: the served `GetTelemetrySnapshot` read returns the
//! verified tenant's latest stored metric values by name, refuses another
//! tenant, and writes nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_obs::obs::{ObsState, ObsStateExt};
use medulla_tsdb::point::Point;
use tokio::sync::RwLock;

const TENANT: &str = "tenant-a";
const OTHER_TENANT: &str = "tenant-b";
const SECOND_NS: i64 = 1_000_000_000;

fn now_ns() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch");
    i64::try_from(elapsed.as_nanos()).expect("nanos fit")
}

fn seed(obs: &ObsState, series: &str, samples: &[(i64, f64)]) {
    let points: Vec<Point> = samples
        .iter()
        .map(|(ts, value)| Point::single(*ts, *value))
        .collect();
    obs.series_store()
        .append_batch(
            series,
            1,
            3_600_000_000_000,
            &["value".to_string()],
            &points,
        )
        .expect("seed series");
}

async fn snapshot(
    state: &Arc<RwLock<ServerState>>,
    caller_tenant: &str,
    asked_tenant: &str,
) -> Response {
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            state,
            Request {
                id: 1,
                graph: "__commons__".to_string(),
                auth_token: String::new(),
                agent_id: Some("reader".to_string()),
                method: Method::GetTelemetrySnapshot {
                    tenant: asked_tenant.to_string(),
                },
            },
            VerifiedRequestContext::verified_for_test_with_scopes(
                "reader",
                caller_tenant,
                &["telemetry:read"],
            ),
        ),
    )
    .await
}

fn metrics(response: &Response) -> BTreeMap<String, f64> {
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => {
            serde_json::from_value(value.clone()).expect("metric map")
        }
        other => panic!("expected a JSON metric map, got {other:?}"),
    }
}

// spec: MEDULLA-ORCH-VOCAB-R804
#[tokio::test]
async fn snapshot_is_tenant_scoped_and_read_only() {
    let obs = Arc::new(ObsState::in_memory(1024).expect("in-memory observability store"));
    let now = now_ns();
    seed(
        &obs,
        &format!("requests_total{{__name__=\"requests_total\",medulla_tenant=\"{TENANT}\"}}"),
        &[(now - 40 * SECOND_NS, 3.0), (now - 10 * SECOND_NS, 7.0)],
    );
    // Older than the default 300 s window: not part of the snapshot.
    seed(
        &obs,
        &format!("stale_total{{__name__=\"stale_total\",medulla_tenant=\"{TENANT}\"}}"),
        &[(now - 3_600 * SECOND_NS, 1.0)],
    );
    // Another tenant's series of the same name must stay invisible.
    seed(
        &obs,
        &format!("requests_total{{__name__=\"requests_total\",medulla_tenant=\"{OTHER_TENANT}\"}}"),
        &[(now - 5 * SECOND_NS, 999.0)],
    );
    let series_before = obs.series_store().list_series().expect("series").len();

    let mut server =
        ServerState::new_for_test("snapshot-secret", ServerState::test_isolation("reader"));
    server.set_obs(Some(obs.clone()));
    let state = Arc::new(RwLock::new(server));

    let own = metrics(&snapshot(&state, TENANT, TENANT).await);
    assert_eq!(own, BTreeMap::from([("requests_total".to_string(), 7.0)]));

    let theirs = metrics(&snapshot(&state, OTHER_TENANT, OTHER_TENANT).await);
    assert_eq!(
        theirs,
        BTreeMap::from([("requests_total".to_string(), 999.0)])
    );

    let cross = snapshot(&state, TENANT, OTHER_TENANT).await;
    let refusal = cross.error.as_deref().unwrap_or_default();
    assert!(refusal.starts_with("ACCESS_DENIED"), "{cross:?}");

    assert_eq!(
        obs.series_store().list_series().expect("series").len(),
        series_before,
        "the read wrote nothing"
    );
}
