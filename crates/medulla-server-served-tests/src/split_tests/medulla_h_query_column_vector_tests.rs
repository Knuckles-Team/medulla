//! The served column vector search (MEDULLA-FEDERATED-QUERY-R077.5): a SQL column with a live
//! semantic binding answers `ORDER BY NEAREST(column, 'text')` through the served `Sql` method.
//!
//! The binding is driven to a live generation by
//! [`drive_binding_to_live_generation`], through the same signed dispatch a stage worker uses.
//! The query text embeds through the engine-local modality encoder registry at the binding's
//! space tuple, so the test registers an encoder for that space that answers the vector the
//! live generation holds for the row; the probe's distance is then `1 - cosine similarity` of
//! two equal unit vectors.

use std::sync::Arc;

use medulla_exec::exec::TextEmbedder;
use medulla_protocol_types::protocol::{Method, QueryResult};
use medulla_server_api::test_support::decode_raw_response as ok;

use super::medulla_h_query_dispatch_pipeline_tests::{
    dispatch_state, drive_binding_to_live_generation, refused, signed_method, worker_authority,
    TENANT,
};
use super::medulla_h_query_sql_source_read_tests::{binding_draft, dispatch_table_fixture};
use super::semantic_live_generation::entity_vector;

/// An encoder of the binding's space that answers the vector the live generation holds for one
/// source entity, whatever the query text.
struct EntityEncoder {
    dimension: usize,
    entity: String,
}

impl TextEmbedder for EntityEncoder {
    fn embed(&self, _text: &str) -> Result<Vec<f32>, String> {
        Ok(entity_vector(self.dimension, &self.entity))
    }
}

async fn sql(
    state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    id: u64,
    query: &str,
) -> medulla_protocol_types::protocol::Response {
    medulla_server_router::dispatch::request_boundary::dispatch(
        state,
        signed_method(
            id,
            Method::Sql {
                query: query.to_string(),
                params_msgpack: Vec::new(),
            },
        ),
    )
    .await
}

// spec: MEDULLA-FEDERATED-QUERY-R077.5
#[tokio::test(flavor = "multi_thread")]
async fn bound_column_nearest_neighbour() {
    let worker = worker_authority();
    let fixture = dispatch_table_fixture(&worker, TENANT);
    let draft = binding_draft(&worker, &fixture.snapshot);
    let state = dispatch_state(&fixture.persist_dir);

    // Before the binding exists the column has nothing to probe.
    let unbound = refused(
        "NEAREST over a column with no binding",
        sql(
            &state,
            200_001,
            "SELECT body FROM documents ORDER BY NEAREST(body, 'pipeline') LIMIT 1",
        )
        .await,
    );
    assert!(unbound.starts_with("COLUMN_NOT_BOUND"), "{unbound}");

    let live = drive_binding_to_live_generation(&state, &draft, &fixture).await;
    assert_eq!(
        live.entities.len(),
        1,
        "the committed row is the one entity"
    );
    medulla_plan::embed_registry::modality_encoders()
        .register(
            "r077-5-column-encoder",
            &draft.model.model_id,
            draft.dimension as usize,
            &draft.model.model_revision,
            Box::new(EntityEncoder {
                dimension: draft.dimension as usize,
                entity: live.entities[0].clone(),
            }),
        )
        .expect("the encoder registers once");

    // The nearest row is the one the live generation indexes, and the read returns its text.
    let response = sql(
        &state,
        200_002,
        "SELECT body FROM documents ORDER BY NEAREST(body, 'pipeline') LIMIT 1",
    )
    .await;
    let result: QueryResult = ok("NEAREST read", response);
    assert_eq!(result.columns, vec!["body".to_string()]);
    assert_eq!(result.rows.len(), 1);
    let row: Vec<serde_json::Value> = rmp_serde::from_slice(&result.rows[0]).unwrap();
    assert_eq!(row, vec![serde_json::json!("dispatch pipeline bytes")]);

    // A column of the same table with no binding of its own is still refused.
    let other = refused(
        "NEAREST over the unbound id column",
        sql(
            &state,
            200_003,
            "SELECT id FROM documents ORDER BY NEAREST(id, 'doc') LIMIT 1",
        )
        .await,
    );
    assert!(other.starts_with("COLUMN_NOT_BOUND"), "{other}");
}
