//! MEDULLA-CQ-XSRC-R009.3: a served `DECAY HALF_LIFE` stage measures a row's age
//! against the engine clock (`authoritative_now_ms`), never the process wall clock.
//! The test moves the engine clock forward by exactly one half-life with
//! `with_engine_clock_offset`, so no test sleeps and no bracket of wall readings is
//! needed: the decayed weight is `0.5`.
// spec: MEDULLA-CQ-XSRC-R009.3
#![cfg(all(feature = "query", feature = "redb", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_kernel::kernel::dispatch_support::{authoritative_now_ms, with_engine_clock_offset};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::wire::{UqlResult, UqlRow};

const SECRET: &str = "uql-decay-engine-clock-secret";
/// Ten years, so the milliseconds the wall clock moves while the test runs weigh far
/// less than the f32 resolution of a score.
const HALF_LIFE_SECS: u64 = 315_360_000;
const HALF_LIFE_MS: u64 = HALF_LIFE_SECS * 1000;

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn uql_rows(state: &test_support::SharedState, id: u64, offset_ms: u64) -> Vec<UqlRow> {
    let request = test_support::commons_request(
        SECRET,
        id,
        Method::Uql {
            text: format!("MATCH (:Evidence) |> DECAY HALF_LIFE {HALF_LIFE_SECS}"),
            params: Default::default(),
            prepare: false,
            foreign_executions: Default::default(),
        },
    );
    let response: Response =
        with_engine_clock_offset(offset_ms, test_support::dispatch(state, request)).await;
    assert!(response.error.is_none(), "UQL failed: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Raw(bytes)) => match rmp_serde::from_slice(&bytes).unwrap() {
            UqlResult::Rows { rows, .. } => rows,
            other => panic!("expected rows, got {other:?}"),
        },
        other => panic!("expected a raw result, got {other:?}"),
    }
}

#[tokio::test]
async fn decay_weight_follows_the_engine_clock() {
    let state = state();
    let valid_time_ms = authoritative_now_ms();
    let response = test_support::dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            1,
            Method::AddNode {
                node_id: "contradiction-1".to_string(),
                properties_msgpack: test_support::json_bytes(serde_json::json!({
                    "type": "Evidence",
                    "valid_time_ms": valid_time_ms,
                })),
            },
        ),
    )
    .await;
    assert!(response.error.is_none(), "AddNode failed: {:?}", response.error);

    // The engine clock at the node's own time: the weight is one.
    let fresh = uql_rows(&state, 2, 0).await;
    let score = fresh[0].score.expect("a decayed row carries a score");
    assert!((f64::from(score) - 1.0).abs() < 1e-6, "fresh weight {score}");

    // The engine clock one half-life later: the weight is one half. The wall clock did
    // not move, so only the engine clock can explain it.
    let aged = uql_rows(&state, 3, HALF_LIFE_MS).await;
    assert_eq!(aged.len(), 1, "{aged:?}");
    let score = aged[0].score.expect("a decayed row carries a score");
    assert!(
        (f64::from(score) - 0.5).abs() < 1e-6,
        "a row one half-life old weighs one half on the engine clock, got {score}"
    );
}
