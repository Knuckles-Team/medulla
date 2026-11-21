//! The one executor over a live semantic generation (MEDULLA-FEDERATED-QUERY-R077.5).
//!
//! A column's semantic binding answers a query text with the nearest member identities of the
//! generation the active pointer names. The query text is embedded by the engine-local
//! modality encoder registry at the binding's space tuple; the probe is the generation's own
//! ANN image, so nothing here builds, writes or activates an index.

use medulla_ann_codes::semantic_index_service::SemanticIndexService;
use medulla_exec::exec::TextEmbedder;
use medulla_index_types::semantic_index::SemanticBinding;
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
    let live = live_image(service, binding)?;
    let encoder = medulla_plan::embed_registry::modality_encoders().lookup_by_space(
        &binding.model_id,
        live.dimension,
        &binding.model_revision,
    )?;
    let vector = encoder.embed(text)?;
    live.probe(&vector, k)
}

/// Probe the live generation of `binding` for the `k` members nearest to `vector`.
///
/// The bookmark search (MEDULLA-MEDIA-VOCAB-R001.102.2) passes the caller's embedding; the
/// refusals are those of [`search_live`] without the encoder lookup.
pub fn probe_vector(
    service: &SemanticIndexService,
    binding: &SemanticBinding,
    vector: &[f32],
    k: usize,
) -> Result<LiveSearch, String> {
    live_image(service, binding)?.probe(vector, k)
}

struct LiveImage {
    generation: u64,
    dimension: usize,
    image: medulla_semantic_index::semantic::SemanticGenerationImage,
}

impl LiveImage {
    fn probe(&self, vector: &[f32], k: usize) -> Result<LiveSearch, String> {
        if vector.len() != self.dimension {
            return Err(format!(
                "{}: the encoder returned {} dimensions, the binding holds {}",
                ServerErrorCode::SearchProbeModelMismatch,
                vector.len(),
                self.dimension
            ));
        }
        let hits = self
            .image
            .search(vector, k)
            .map_err(|error| format!("{}: {error}", ServerErrorCode::SearchProbeModelMismatch))?
            .into_iter()
            .map(|(member, similarity)| (member, 1.0 - similarity))
            .collect();
        Ok(LiveSearch {
            generation: self.generation,
            hits,
        })
    }
}

fn live_image(
    service: &SemanticIndexService,
    binding: &SemanticBinding,
) -> Result<LiveImage, String> {
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
    Ok(LiveImage {
        generation,
        dimension,
        image,
    })
}
