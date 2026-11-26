//! MEDULLA-FEDERATED-QUERY-R039.1.3: the engine's bound text embedder is registered into
//! the modality encoder registry at server startup, so a served `UnifiedQuery` under a
//! modality pair reaches it; a space whose binding names no registered encoder refuses
//! `QUERY_ENCODER_UNAVAILABLE`.
#![cfg(feature = "query")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_index_types::embedding::{EmbeddingSpaceRef, Modality, ModalityBinding};
use medulla_index_types::retrieval_request::{ModalityPair, RetrievalRequest};
use medulla_protocol_types::protocol::{GraphType, Method, Response};
use medulla_query_types::wire::{Op, Plan};
use medulla_server::query_encoders::register_query_encoders;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::query_txn::query_support::uql_text_encoder_identity;

const SECRET: &str = "query-encoders-served-secret";
const COMMONS: &str = "__commons__";
const UNBOUND: &str = "encoder-unbound";

fn state() -> Arc<RwLock<ServerState>> {
    // The server binds its text embedder from its configuration once per process; the
    // boot step then registers it, exactly as `compose_server_state` does.
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        std::env::set_var("MEDULLA_UQL_TEXT_EMBEDDER", "hash");
        register_query_encoders().expect("the bound encoder registers at startup");
    });
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

fn space(id: &str, dimensions: usize, revision: &str, encoder: &str) -> EmbeddingSpaceRef {
    EmbeddingSpaceRef::pinned(id, revision, "sha256:w", "sha256:p", dimensions, false)
        .unwrap()
        .with_modalities(
            Modality::ALL
                .iter()
                .map(|modality| ModalityBinding {
                    modality: *modality,
                    encoder: encoder.to_string(),
                })
                .collect(),
        )
        .unwrap()
}

async fn seed(state: &Arc<RwLock<ServerState>>, graph: &str, space: EmbeddingSpaceRef) {
    {
        let mut guard = state.write().await;
        if guard.registry.get(graph).is_none() {
            guard
                .registry
                .create_graph(graph, GraphType::Team, None)
                .unwrap();
        }
        guard
            .registry
            .get(graph)
            .unwrap()
            .core
            .semantic_store
            .write()
            .declare_space(space)
            .unwrap();
    }
    let response = test_support::dispatch(
        state,
        test_support::request(
            SECRET,
            1,
            graph,
            Method::AddNode {
                node_id: "doc-0".into(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({ "type": "Doc", "text": "entry" }),
                ),
            },
        ),
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.refusal_text());
}

async fn query(state: &Arc<RwLock<ServerState>>, graph: &str) -> Response {
    let method = Method::UnifiedQuery {
        plan: Plan::new(vec![
            Op::Scan {
                label: "Doc".into(),
            },
            Op::RankEmbed {
                text: "entry".into(),
            },
        ]),
        foreign_execution: None,
        foreign_executions: Default::default(),
        code_map: None,
        retrieval: Some(RetrievalRequest {
            constraints: None,
            topic: None,
            modality: Some(ModalityPair {
                query: Modality::Text,
                target: Modality::Text,
            }),
        }),
        latency_class: Default::default(),
    };
    test_support::dispatch(state, test_support::request(SECRET, 2, graph, method)).await
}

// spec: MEDULLA-FEDERATED-QUERY-R039.1.3
#[tokio::test(flavor = "multi_thread")]
async fn registered_text_encoder_reaches_the_served_query() {
    let state = state();
    let identity = uql_text_encoder_identity().expect("the hash embedder is bound");
    assert_eq!(identity.encoder_id, "builtin:hash");

    // A space whose text binding names the registered encoder at its exact tuple ranks.
    seed(
        &state,
        COMMONS,
        space(
            &identity.space_id,
            identity.dimensions,
            &identity.revision,
            &identity.encoder_id,
        ),
    )
    .await;
    let served = query(&state, COMMONS).await;
    assert!(
        served.error.is_none(),
        "a registered encoder serves: {:?}",
        served.refusal_text()
    );

    // A space whose binding names no registered encoder refuses before any rank.
    seed(
        &state,
        UNBOUND,
        space(
            &identity.space_id,
            identity.dimensions,
            &identity.revision,
            "builtin:unregistered",
        ),
    )
    .await;
    let refused = query(&state, UNBOUND).await;
    assert!(
        refused
            .refusal_text()
            .unwrap_or_default()
            .contains("QUERY_ENCODER_UNAVAILABLE"),
        "{:?}",
        refused.refusal_text()
    );
}
