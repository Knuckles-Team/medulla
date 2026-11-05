//! Served scheduler configuration reads (MEDULLA-DURABLE-KERNEL-R077).
//!
//! `GetShardWriterWidth` is driven through the real served `dispatch` surface
//! over a redb backend opened with a configured multi-shard layout: the reply
//! carries exactly the configured writer count `k`, and a caller whose verified
//! context lacks `scheduler:read` is refused before the handler runs.
#![cfg(all(feature = "server", feature = "security", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, ResultPayload};
use medulla_work_types::worker_registry::ShardWriterWidth;

const SECRET: &str = "medulla-durable-kernel-r077-shard-writer-width-secret";
const CONFIGURED_WRITERS: usize = 4;
const GRAPH: &str = "r077-width";

fn width_method() -> Method {
    Method::GetShardWriterWidth {
        tenant: common::test_tenant().to_string(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R077
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn width_for_a_multi_shard_tenant_matches_configured_writers() {
    // The shard count is read once when the backend opens; configure k first.
    std::env::set_var("MEDULLA_REDB_SHARDS", CONFIGURED_WRITERS.to_string());
    common::configure_authority();
    let dir = test_support::fresh_dir("r077-shard-writer-width");
    let (_backend, state) = test_support::redb_state_at(
        SECRET,
        common::current_isolation(),
        dir.to_str().expect("utf-8 temp dir"),
    );

    let created = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            1,
            GRAPH,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
        ),
    )
    .await;
    assert!(created.error.is_none(), "CreateGraph: {:?}", created.error);

    let response = test_support::dispatch(
        &state,
        test_support::request(SECRET, 2, GRAPH, width_method()),
    )
    .await;
    assert!(
        response.error.is_none(),
        "GetShardWriterWidth: {:?} {:?}",
        response.error,
        response.error_detail
    );
    let width: ShardWriterWidth = match &response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack width"),
        other => panic!("unexpected width payload: {other:?}"),
    };
    assert_eq!(width.tenant, common::test_tenant());
    assert_eq!(width.shard_writers as usize, CONFIGURED_WRITERS);

    // A caller without scheduler:read is refused before the handler runs.
    let refused = test_support::dispatch(
        &state,
        common::signed_request_with_scopes(SECRET, 3, GRAPH, &["graph:read"], width_method()),
    )
    .await;
    assert!(refused.result.is_none(), "unscoped caller got a width");
    let error = refused.error.expect("unscoped caller is refused");
    assert!(
        error.contains("ACCESS_DENIED") || error.contains("scheduler:read"),
        "unexpected refusal: {error}"
    );

    // A body tenant other than the verified carrier tenant is refused.
    let foreign = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            4,
            GRAPH,
            Method::GetShardWriterWidth {
                tenant: "another-tenant".to_string(),
            },
        ),
    )
    .await;
    assert!(foreign.result.is_none(), "foreign tenant got a width");
    assert!(foreign
        .error
        .expect("foreign tenant is refused")
        .contains("ACCESS_DENIED"));
    std::env::remove_var("MEDULLA_REDB_SHARDS");
}
