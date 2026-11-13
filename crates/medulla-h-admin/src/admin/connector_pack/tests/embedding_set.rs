//! `source_ingestion::prepare` builds the explicit embedding set from the
//! columns that survive admission (MEDULLA-DECISION-ENGINE-R079.6). The test
//! drives `prepare` itself, not a served call, so no receipt is involved. It
//! lives in this crate because only here can a connector pack be imported for
//! the ingest to resolve.

use medulla_h_ingest::source_ingestion::{prepare, PrepareContext};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_source_types::source_ingestion::column_profile::{
    ColumnExclusion, ColumnExclusionReason, EmbeddingSetEntry,
};
use medulla_source_types::source_ingestion::SourceIngestionRequest;
use medulla_types::contract::ResourceId;

use super::scale::{first_ingest, pack_graph};
use super::sql_columns::profiled_request;
use super::{Served, ADMIN, TENANT};

/// Run `prepare` on the pack graph `pack_graph` created, as the ingest route does.
async fn prepared_embedding_set(
    served: &Served,
    request: SourceIngestionRequest,
) -> medulla_h_ingest::ingestion_admission::EmbeddingSet {
    let verified = VerifiedRequestContext::verified_for_test_with_scopes(ADMIN, TENANT, &["*"]);
    let tenant_scope = CarrierAuthority::from_verified(&verified)
        .expect("verified carrier authority")
        .tenant_scope()
        .to_string();
    let prepared = prepare(
        PrepareContext {
            state: &served.state,
            request_id: 1,
            graph_name: "packingest",
            tenant_scope: &tenant_scope,
            verified: &verified,
            graph_version: 0,
            placement_epoch: 0,
            fencing_token: None,
            schema_sources: Default::default(),
        },
        request,
    )
    .await
    .expect("prepare admits the batch");
    prepared.embedding_set().clone()
}

// spec: MEDULLA-DECISION-ENGINE-R079.6
#[tokio::test]
async fn prepare_builds_the_embedding_set_from_admitted_columns() {
    let (served, _guard) = pack_graph("embedding-set").await;
    let set = prepared_embedding_set(&served, profiled_request()).await;
    assert_eq!(
        set.entries,
        vec![EmbeddingSetEntry {
            stream: ResourceId::new("items").unwrap(),
            record_id: "item-1".into(),
            column: "notes".into(),
        }],
        "only the column that survives admission joins the embedding set"
    );
    let mut excluded: Vec<_> = [
        ("owner_id", ColumnExclusionReason::ForeignKey),
        ("active", ColumnExclusionReason::Boolean),
        ("created_at", ColumnExclusionReason::Timestamp),
        ("price", ColumnExclusionReason::Numeric),
        ("status", ColumnExclusionReason::Enumeration),
        ("short_notes", ColumnExclusionReason::InsufficientSamples),
    ]
    .into_iter()
    .map(|(column, reason)| ColumnExclusion {
        stream: ResourceId::new("items").unwrap(),
        column: column.into(),
        reason,
    })
    .collect();
    excluded.sort();
    assert_eq!(set.exclusions, excluded);

    let (plain, _plain_guard) = pack_graph("embedding-set-plain").await;
    let empty = prepared_embedding_set(&plain, first_ingest()).await;
    assert!(
        empty.entries.is_empty() && empty.exclusions.is_empty(),
        "a record with no column profile contributes nothing to the set"
    );
}
