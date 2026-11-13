//! MEDULLA-FEDERATED-QUERY-R081.11 and R081.3: the served retrieval round through
//! the real `dispatch` shell. A round runs one plan over the caller's snapshot and
//! then the parts the request carries; another graph's caller never sees the rows.
#![cfg(feature = "query")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_exec::algebra::{Op, Plan};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_result_types::retrieval_round::{RoundRequest, RoundResult, SufficiencyPart};
use medulla_result_types::retrieval_verdict::{
    FallbackPolicy, RepairStep, Verdict, RETRIEVAL_ROUND_LIMIT_REQUIRED,
};
use medulla_server_api::state::ServerState;

const SECRET: &str = "retrieval-round-served-secret";

fn state() -> Arc<RwLock<ServerState>> {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn seed(state: &Arc<RwLock<ServerState>>, docs: usize) {
    for i in 0..docs {
        let response = test_support::dispatch(
            state,
            test_support::commons_request(
                SECRET,
                i as u64 + 1,
                Method::AddNode {
                    node_id: format!("doc-{i}"),
                    properties_msgpack: test_support::json_bytes(
                        serde_json::json!({ "type": "Doc", "text": format!("entry {i}") }),
                    ),
                },
            ),
        )
        .await;
        assert!(
            response.error.is_none(),
            "seed failed: {:?}",
            response.error
        );
    }
}

fn request(sufficiency: Option<SufficiencyPart>) -> RoundRequest {
    RoundRequest {
        plan: Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]),
        corpus_revision: "corpus-1".into(),
        model_revision: "model-1".into(),
        sufficiency,
    }
}

fn part(round_limit: Option<u32>) -> SufficiencyPart {
    SufficiencyPart {
        round: 1,
        round_limit,
        k: 4,
        fallback_mode: "extractive".into(),
        fallback: FallbackPolicy::Lenient,
    }
}

async fn round_as(
    state: &Arc<RwLock<ServerState>>,
    graph: Option<&str>,
    request: RoundRequest,
) -> Response {
    let method = Method::RetrievalRound {
        request: Box::new(request),
    };
    let wire = match graph {
        None => test_support::commons_request(SECRET, 90, method),
        Some(graph) => test_support::request(SECRET, 90, graph, method),
    };
    test_support::dispatch(state, wire).await
}

fn decode(response: Response) -> RoundResult {
    assert!(
        response.error.is_none(),
        "round failed: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed result"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R081.11
#[tokio::test(flavor = "multi_thread")]
async fn round_with_no_parts_returns_rows_only() {
    let state = state();
    seed(&state, 3).await;
    let result = decode(round_as(&state, None, request(None)).await);
    let mut ids: Vec<_> = result.rows.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["doc-0", "doc-1", "doc-2"]);
    assert!(result.sufficiency.is_none());

    // Another tenant's graph sees none of those rows.
    let other = round_as(&state, Some("other-tenant"), request(None)).await;
    assert!(
        other.error.is_some() || decode(other).rows.is_empty(),
        "another graph must not see the rows"
    );

    // A malformed request refuses with the typed code and returns nothing.
    let mut blank = request(None);
    blank.corpus_revision = String::new();
    let refused = round_as(&state, None, blank).await;
    assert!(refused
        .error
        .unwrap_or_default()
        .starts_with("RETRIEVAL_CONTRACT_INVALID"));
}

// spec: MEDULLA-FEDERATED-QUERY-R081.3
#[tokio::test(flavor = "multi_thread")]
async fn served_round_returns_verdict_beside_rows() {
    let state = state();
    seed(&state, 2).await;
    let short = decode(round_as(&state, None, request(Some(part(Some(4))))).await);
    assert_eq!(short.rows.len(), 2);
    let verdict = short.sufficiency.expect("verdict beside rows");
    assert_eq!(verdict.verdict, Verdict::Insufficient);
    assert_eq!(verdict.repair, RepairStep::BoostK { k: 8 });

    seed(&state, 3).await;
    let enough = decode(round_as(&state, None, request(Some(part(Some(4))))).await);
    assert_eq!(
        enough.sufficiency.expect("verdict").verdict,
        Verdict::Sufficient
    );

    // Without a round limit the part refuses with its typed code.
    let refused = round_as(&state, None, request(Some(part(None)))).await;
    assert!(refused
        .error
        .unwrap_or_default()
        .starts_with(RETRIEVAL_ROUND_LIMIT_REQUIRED));

    // A request without the part returns rows only.
    assert!(decode(round_as(&state, None, request(None)).await)
        .sufficiency
        .is_none());
}
