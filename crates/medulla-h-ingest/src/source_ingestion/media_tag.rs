//! Tag ingest mapping (MEDULLA-MEDIA-VOCAB-R001.100.2).
//!
//! Each `media_tag` feature of a `SourceIngest` batch becomes one `MediaTag` node, one
//! `taggedMedia` edge to the media item and one policy record, all joining the batch's own
//! methods so they commit in the one `SourceIngest` change envelope.

use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::SourceIngestionBatch;
use medulla_types::contract::Digest256;
use medulla_types::media_tag::MediaTagValue;

use super::{policy, source_node_id, source_scoped_id, AdmittedRecords, PrepareContext};

const PROVENANCE_MISSING: &str = "TAG_PROVENANCE_MISSING";
const REVISION_STALE: &str = "TAG_MEDIA_REVISION_STALE";

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| format!("media tag encoding failed: {error}"))
}

/// The current revision of the media item `object_id`: the batch record that carries it, else the
/// stored node; `None` when neither exists.
async fn current_revision(
    ctx: &PrepareContext<'_>,
    batch: &SourceIngestionBatch,
    persistence: &dyn medulla_server_api::api::persistence::PersistenceBackend,
    graph_fname: &str,
    object_id: &str,
) -> Result<Option<u64>, String> {
    for record in &batch.records {
        let id = source_node_id(
            ctx.tenant_scope,
            batch.connector.as_str(),
            record.stream.as_str(),
            &record.record_id,
        )?;
        if id == object_id {
            let payload = record.payload.value()?;
            return Ok(Some(
                payload
                    .get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            ));
        }
    }
    let Some(blob) = persistence.read_node(graph_fname, object_id).await? else {
        return Ok(None);
    };
    let properties = medulla_types::msgpack::decode_property_object(&blob)
        .map_err(|_| format!("object {object_id} stores an undecodable property map"))?;
    Ok(Some(
        properties
            .get("revision")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    ))
}

pub(super) async fn materialize_media_tags(
    ctx: &PrepareContext<'_>,
    batch: &SourceIngestionBatch,
    persistence: &dyn medulla_server_api::api::persistence::PersistenceBackend,
    graph_fname: &str,
    policy_subject: &str,
    entry_revision: u64,
    admitted: &mut AdmittedRecords,
) -> Result<(), String> {
    for feature in &batch.features {
        let value: MediaTagValue = rmp_serde::from_slice(&feature.value_msgpack)
            .map_err(|_| format!("{PROVENANCE_MISSING}: feature {}", feature.feature_id))?;
        if value.producer_id.is_empty() || value.evidence_ref.is_empty() {
            return Err(format!(
                "{PROVENANCE_MISSING}: feature {}",
                feature.feature_id
            ));
        }
        let current =
            current_revision(ctx, batch, persistence, graph_fname, &feature.object_id).await?;
        match current {
            Some(revision) if value.media_revision >= revision => {}
            _ => {
                return Err(format!("{REVISION_STALE}: feature {}", feature.feature_id));
            }
        }
        let tag_id = source_scoped_id(
            "media-tag",
            b"eg/media-tag-identity/v1",
            [
                ctx.tenant_scope,
                batch.connector.as_str(),
                &feature.object_id,
                &feature.feature_id,
            ],
        )?;
        let media_revision = value.media_revision.to_string();
        let provenance_digest = Digest256::framed(
            b"eg/media-tag-provenance/v1",
            &[
                value.producer_id.as_bytes(),
                feature.model_version.as_bytes(),
                value.evidence_ref.as_bytes(),
                media_revision.as_bytes(),
                feature.object_id.as_bytes(),
            ],
        )?
        .to_hex();
        let tag_kind = serde_json::to_value(value.tag_kind).map_err(|error| error.to_string())?;
        admitted.methods.push(Method::AddNode {
            node_id: tag_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": "MediaTag",
                "label": value.label,
                "origin": value.origin,
                "taxonomyId": value.taxonomy_id,
                "taxonomyVersion": value.taxonomy_version,
                "tagKind": tag_kind,
                "tagConfidence": value.confidence,
                "producerId": value.producer_id,
                "producerRevision": feature.model_version,
                "evidenceRef": value.evidence_ref,
                "mediaRevision": media_revision,
                "provenanceDigest": provenance_digest,
            }))?,
        });
        admitted.methods.push(Method::AddEdge {
            source_id: tag_id.clone(),
            target_id: feature.object_id.clone(),
            properties_msgpack: encode(&serde_json::json!({ "relationship": "taggedMedia" }))?,
        });
        admitted.policies.push(policy(
            &tag_id,
            ctx.tenant_scope,
            policy_subject,
            entry_revision,
        ));
    }
    Ok(())
}
