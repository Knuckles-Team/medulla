//! The one executor over a live semantic generation (MEDULLA-FEDERATED-QUERY-R077.5).
//!
//! A column's semantic binding answers a query text with the nearest member identities of the
//! generation the active pointer names. The query text is embedded by the engine-local
//! modality encoder registry at the binding's space tuple; the probe is the generation's own
//! ANN image, so nothing here builds, writes or activates an index.

use medulla_ann_codes::semantic_index_service::SemanticIndexService;
use medulla_exec::exec::TextEmbedder;
use medulla_index_types::semantic_index::SemanticBinding;
use medulla_semantic_index::semantic_ann::AnnIndex;
use medulla_types::contract::ServerErrorCode;

/// What one live probe answered.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSearch {
    /// The live generation the hits come from.
    pub generation: u64,
    /// `(member id, distance)`, nearest first. The member id is the row's
    /// `SemanticSqlSourceIdentity::source_entity_id`; the distance is `1 - cosine similarity`.
    pub hits: Vec<(String, f32)>,
}

/// Probe the live generation of `binding` for the `k` members nearest to `text`.
///
/// `COLUMN_NOT_BOUND` when the binding has no live generation; `QUERY_ENCODER_UNAVAILABLE`
/// when the registry has no encoder for the binding's space tuple
/// `(model id, dimension, model revision)`; `SEARCH_PROBE_MODEL_MISMATCH` when the live
/// image's identity is not `(binding.dimension, Some(binding.model_digest))` or the encoder
/// returns a vector of another width.
pub fn search_live(
    service: &SemanticIndexService,
    binding: &SemanticBinding,
    text: &str,
    k: usize,
) -> Result<LiveSearch, String> {
    let dimension = usize::try_from(binding.dimension)
        .map_err(|_| format!("{}: binding dimension", ServerErrorCode::SearchProbeModelMismatch))?;
    let (generation, image) = service
        .read_live()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "{}: the binding has no live generation",
                ServerErrorCode::ColumnNotBound
            )
        })?;
    let identity = image
        .identity()
        .map_err(|error| format!("{}: {error}", ServerErrorCode::SearchProbeModelMismatch))?;
    if identity != (dimension, Some(binding.model_digest.clone())) {
        return Err(format!(
            "{}: the live generation was built for another model or width",
            ServerErrorCode::SearchProbeModelMismatch
        ));
    }
    let encoder = medulla_plan::embed_registry::modality_encoders().lookup_by_space(
        &binding.model_id,
        dimension,
        &binding.model_revision,
    )?;
    let vector = encoder.embed(text)?;
    if vector.len() != dimension {
        return Err(format!(
            "{}: the encoder returned {} dimensions, the binding holds {dimension}",
            ServerErrorCode::SearchProbeModelMismatch,
            vector.len()
        ));
    }
    let index = AnnIndex::from_image(&image.index)
        .map_err(|error| format!("{}: {error}", ServerErrorCode::SearchProbeModelMismatch))?;
    let hits = index
        .search(&vector, k)
        .into_iter()
        .map(|(member, similarity)| (member, 1.0 - similarity))
        .collect();
    Ok(LiveSearch { generation, hits })
}
