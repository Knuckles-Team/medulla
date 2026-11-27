//! Media derivation ingest mapping (MEDULLA-MEDIA-VOCAB-R650.3).
//!
//! Each `media_derivation` feature of a `SourceIngest` batch becomes one `MediaDerivation`
//! node, its embedding, one `wasDerivedFrom` edge to the media item and one policy record, all
//! joining the batch's own methods so they commit in the one `SourceIngest` change envelope.

use medulla_modality::media_memory::{
    plan_media_write, DerivationInput, DerivationKind, MediaBlobRef, MediaRefusal,
};
use medulla_protocol_types::change_envelope::BlobReference;
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::{SourceIngestionBatch, MEDIA_DERIVATION_FEATURE_KIND};
use medulla_types::media_derivation::MediaDerivationValue;

use super::{policy, AdmittedRecords, PrepareContext};

const DERIVATION_EMPTY: &str = "MEDIA_DERIVATION_EMPTY";
const DERIVATION_KIND_INVALID: &str = "MEDIA_DERIVATION_KIND_INVALID";

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("media derivation encoding failed: {error}"))
}

fn kind_of(value: &str) -> Option<DerivationKind> {
    match value {
        "transcript" => Some(DerivationKind::Transcript),
        "caption" => Some(DerivationKind::Caption),
        _ => None,
    }
}

/// The planned node of one feature: the id is the SHA-256 of the item's real content digest (the
/// raw blob the blob ledger holds for the item's record), the kind and the text, so a repeated feature is the same node.
fn plan_feature(
    feature: &medulla_source_types::source_ingestion::SourceFeature,
    blob: &BlobReference,
) -> Result<medulla_modality::media_memory::PlannedDerivation, String> {
    let value: MediaDerivationValue = rmp_serde::from_slice(&feature.value_msgpack)
        .map_err(|_| format!("{DERIVATION_KIND_INVALID}: feature {}", feature.feature_id))?;
    let kind = kind_of(&value.kind)
        .ok_or_else(|| format!("{DERIVATION_KIND_INVALID}: feature {}", feature.feature_id))?;
    let item = MediaBlobRef {
        digest: blob.digest.clone(),
        mime: blob.media_type.clone(),
        byte_len: blob.length,
    };
    let input = DerivationInput {
        kind,
        text: value.text,
        embedding: value.embedding,
    };
    let plan =
        plan_media_write(&item, true, std::slice::from_ref(&input)).map_err(
            |refusal| match refusal {
                MediaRefusal::DerivationTextEmpty => {
                    format!("{DERIVATION_EMPTY}: feature {}", feature.feature_id)
                }
                MediaRefusal::DigestMalformed => {
                    format!("PARAMETER_INVALID: feature {}", feature.feature_id)
                }
            },
        )?;
    let mut planned = plan
        .derivations
        .into_iter()
        .next()
        .ok_or_else(|| format!("{DERIVATION_EMPTY}: feature {}", feature.feature_id))?;
    planned.derived_from = feature.object_id.clone();
    Ok(planned)
}

pub(super) fn materialize_media_derivations(
    ctx: &PrepareContext<'_>,
    batch: &SourceIngestionBatch,
    policy_subject: &str,
    entry_revision: u64,
    admitted: &mut AdmittedRecords,
) -> Result<(), String> {
    for feature in batch
        .features
        .iter()
        .filter(|feature| feature.kind == MEDIA_DERIVATION_FEATURE_KIND)
    {
        let blob = admitted
            .record_blobs
            .get(&feature.object_id)
            .ok_or_else(|| {
                format!(
                    "PARAMETER_INVALID: feature {} names {}, which is not a record of this batch",
                    feature.feature_id, feature.object_id
                )
            })?;
        let planned = plan_feature(feature, blob)?;
        let node_id = planned.id.clone();
        admitted.methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": "MediaDerivation",
                "derivationKind": planned.kind.as_str(),
                "derivationText": planned.text,
                "producerRevision": feature.model_version,
            }))?,
        });
        if !planned.embedding.is_empty() {
            admitted.methods.push(Method::AddEmbedding {
                node_id: node_id.clone(),
                embedding: planned.embedding.clone(),
            });
        }
        admitted.methods.push(Method::AddEdge {
            source_id: node_id.clone(),
            target_id: planned.derived_from.clone(),
            properties_msgpack: encode(&serde_json::json!({ "relationship": "wasDerivedFrom" }))?,
        });
        admitted.policies.push(policy(
            &node_id,
            ctx.tenant_scope,
            policy_subject,
            entry_revision,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_protocol_types::change_envelope::MaterialOperation;
    use medulla_source_types::source_ingestion::SourceFeature;

    fn blob(digest: &str) -> BlobReference {
        BlobReference {
            blob_id: format!("raw:{digest}"),
            operation: MaterialOperation::Upsert,
            digest_algorithm: "sha256".into(),
            digest: digest.into(),
            media_type: "image/png".into(),
            length: 12,
        }
    }

    fn feature(text: &str, kind: &str) -> SourceFeature {
        SourceFeature {
            feature_id: "f1".to_string(),
            object_id: "source:item".to_string(),
            kind: MEDIA_DERIVATION_FEATURE_KIND.to_string(),
            value_msgpack: rmp_serde::to_vec_named(&MediaDerivationValue {
                kind: kind.to_string(),
                text: text.to_string(),
                embedding: vec![1.0, 0.0],
            })
            .unwrap(),
            model_version: "captioner-1".to_string(),
        }
    }

    // spec: MEDULLA-MEDIA-VOCAB-R650.3
    #[test]
    fn derivation_feature_materializes() {
        let first =
            plan_feature(&feature("a red bicycle", "caption"), &blob(&"a".repeat(64))).unwrap();
        let again =
            plan_feature(&feature("a red bicycle", "caption"), &blob(&"a".repeat(64))).unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(first.derived_from, "source:item");
        assert_eq!(first.embedding, vec![1.0, 0.0]);
        let other = plan_feature(
            &feature("a red bicycle", "transcript"),
            &blob(&"a".repeat(64)),
        )
        .unwrap();
        assert_ne!(first.id, other.id);
        assert!(
            plan_feature(&feature("", "caption"), &blob(&"a".repeat(64)))
                .unwrap_err()
                .starts_with(DERIVATION_EMPTY)
        );
        assert!(plan_feature(&feature("x", "poem"), &blob(&"a".repeat(64)))
            .unwrap_err()
            .starts_with(DERIVATION_KIND_INVALID));
    }

    // spec: MEDULLA-MEDIA-VOCAB-R650.3
    #[test]
    fn derivation_id_binds_the_ledger_digest_not_the_object_id() {
        let one = plan_feature(&feature("a cat", "caption"), &blob(&"a".repeat(64))).unwrap();
        let two = plan_feature(&feature("a cat", "caption"), &blob(&"b".repeat(64))).unwrap();
        assert_ne!(one.id, two.id, "same object id, different content digest");
        let expected = medulla_modality::media_memory::plan_media_write(
            &MediaBlobRef {
                digest: "a".repeat(64),
                mime: "image/png".into(),
                byte_len: 12,
            },
            true,
            &[DerivationInput {
                kind: DerivationKind::Caption,
                text: "a cat".into(),
                embedding: vec![1.0, 0.0],
            }],
        )
        .unwrap();
        assert_eq!(one.id, expected.derivations[0].id);
    }
}
