// CONCEPT:MEDULLA-KG.sharding.semantic-embedding-store-backed — the pinned embedding-space
// identity + stamped-vector currency shared by BOTH `medulla-core::compute::semantic`
// backends (`semantic_hnsw`/`semantic_store_ann`). Lives at the bottom of the DAG
// (pure serde, no dep) for the same reason `commit_descriptor`/`row_predicate` do:
// it is a plain-data identity type that `medulla-core` needs on both sides of its
// backend `#[cfg]` split, with no behavior beyond structural validation.
//
// Two dimensionality ceilings live here rather than in `medulla-core` because both
// backends (and any future one) must agree on ONE authoritative bound rather than
// N independently-chosen ones — the same rationale as `medulla-core`'s own
// `compute::semantic::MAX_EMBEDDING_DIMENSION` (that constant bounds the *generic*
// `check_embedding_dimension` chokepoint every raw write goes through; the two
// below are the narrower ceilings a *maintained* coordinate space and a
// *maintained ANN artifact* respectively may not exceed).

use medulla_types::contract::ServerErrorCode;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// Upper bound on the dimensionality of a raw/native embedding compared via
/// exact (brute-force) search — the "generic" ceiling every `SemanticStore`
/// backend enforces before a vector is even eligible to establish or join a
/// resident arena. Deliberately narrower than any single model's realistic
/// width headroom while still comfortably exceeding real embedding models in
/// use; wider rows stay on exact search and never cross the maintained-index
/// boundary (see [`MAX_MAINTAINED_ANN_DIMENSIONS`]).
pub const MAX_EMBEDDING_DIMENSIONS: usize = 16_384;

/// Upper bound on the dimensionality a maintained ANN artifact (the persisted
/// IVF-PQ/HNSW index) may index. Stricter than [`MAX_EMBEDDING_DIMENSIONS`]:
/// a coordinate space wider than this remains exact-search-only and
/// intentionally never allocates ANN state.
pub const MAX_MAINTAINED_ANN_DIMENSIONS: usize = 4_096;

/// The supported wire revision for a document chunk encoding recipe.
pub const DOCUMENT_ENCODING_POLICY_REVISION: u16 = 1;

/// Whether each document chunk starts its position ids again or retains the
/// document-wide offset of its first token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DocumentPositionMode {
    LocalReset,
    AbsoluteOffset,
}

/// The deterministic reduction used to turn independently encoded chunks into
/// the document vector used by retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DocumentAggregation {
    MeanChunks,
}

/// The pinned recipe for encoding one document into independently processed
/// chunks. This is data only: the ingestion worker owns execution, while this
/// type prevents a worker and query path from silently selecting different
/// chunk boundaries or model revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DocumentEncodingPolicy {
    /// The closed policy schema revision understood by the worker.
    pub policy_revision: u16,
    /// The tokenizer identity used to produce the token ids.
    pub tokenizer: String,
    /// Maximum number of token ids in one independently encoded chunk.
    pub chunk_size: usize,
    /// Number of token ids shared by adjacent chunks.
    pub overlap: usize,
    /// Position numbering supplied to each chunk encoder.
    pub position_mode: DocumentPositionMode,
    /// Reduction used after every chunk has been encoded.
    pub aggregation: DocumentAggregation,
    /// The model revision that must encode every chunk in this policy.
    pub model_revision: String,
}

impl DocumentEncodingPolicy {
    /// Construct the currently served policy revision and validate its bounds.
    pub fn new(
        tokenizer: impl Into<String>,
        chunk_size: usize,
        overlap: usize,
        position_mode: DocumentPositionMode,
        aggregation: DocumentAggregation,
        model_revision: impl Into<String>,
    ) -> Result<Self, String> {
        let policy = Self {
            policy_revision: DOCUMENT_ENCODING_POLICY_REVISION,
            tokenizer: tokenizer.into(),
            chunk_size,
            overlap,
            position_mode,
            aggregation,
            model_revision: model_revision.into(),
        };
        policy.validate()?;
        Ok(policy)
    }

    /// Validate the closed policy revision and the chunk recipe's structural
    /// bounds before a worker may execute it.
    pub fn validate(&self) -> Result<(), String> {
        if self.policy_revision != DOCUMENT_ENCODING_POLICY_REVISION {
            return Err(format!(
                "document encoding policy revision {} is not served",
                self.policy_revision
            ));
        }
        if self.tokenizer.is_empty() {
            return Err("document encoding tokenizer must not be empty".to_string());
        }
        if self.model_revision.is_empty() {
            return Err("document encoding model revision must not be empty".to_string());
        }
        if self.chunk_size == 0 {
            return Err("document encoding chunk size must not be zero".to_string());
        }
        if self.overlap >= self.chunk_size {
            return Err(format!(
                "document encoding overlap {} must be less than chunk size {}",
                self.overlap, self.chunk_size
            ));
        }
        Ok(())
    }
}

/// A pinned identity for the coordinate space an embedding model produces:
/// which model, which revision, what preprocessing, at what width, and
/// whether vectors are L2-normalized before comparison. Two embedding
/// producers (a `SemanticStore` and a query vector, or two stores) are
/// comparable ONLY when their spaces carry the identical `digest` — mixing
/// vectors from different models/preprocessing/normalization into one
/// index/comparison yields confident nonsense that no downstream numeric
/// check can distinguish from a genuine answer (the same hazard
/// `medulla-query::tables::embedding_binding::ModelRef`/`StampedVector` close for
/// the EmbeddingBinding catalog, one layer up; this is the bottom-of-DAG
/// twin of that concept for `medulla-core`'s own `SemanticStore`, which cannot
/// depend on `medulla-query`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EmbeddingSpaceRef {
    /// Stable model identifier (e.g. a model name/id from the embedding
    /// provider's own catalog).
    pub id: String,
    /// The model revision/version this space is pinned to.
    pub revision: String,
    /// Caller-supplied content identity for the model weights (opaque here —
    /// this type does not recompute or verify it, only carries it as part of
    /// the pinned identity that feeds `digest`).
    pub model_hash: String,
    /// Caller-supplied content identity for the preprocessing pipeline
    /// (tokenization/normalization/etc.) applied before embedding.
    pub preprocessing_hash: String,
    /// Vector width this space's model emits.
    pub dimensions: usize,
    /// Whether vectors in this space are L2-normalized before comparison.
    pub normalize: bool,
    /// Derived identity for the whole pinned tuple above — the value every
    /// space-vs-space and space-vs-vector comparison actually compares.
    pub digest: String,
    /// MEDULLA-FEDERATED-QUERY-R039.1.1: the per-modality bindings of one
    /// multimodal space. Empty for a single-modality space; when present it
    /// holds exactly one binding per [`Modality`], all under this space's
    /// identity tuple `(id, dimensions, revision)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<ModalityBinding>,
}

/// MEDULLA-FEDERATED-QUERY-R039.1.1: a modality one multimodal space embeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,
}

impl Modality {
    /// Every modality, in declaration order.
    pub const ALL: [Modality; 4] = [
        Modality::Text,
        Modality::Image,
        Modality::Audio,
        Modality::Video,
    ];
}

/// MEDULLA-FEDERATED-QUERY-R039.1.1: one modality's binding inside a pinned
/// space. The encoder is named by the registry id that produces vectors for
/// this modality in the space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModalityBinding {
    pub modality: Modality,
    /// Registry id of the encoder that embeds this modality into the space.
    pub encoder: String,
}

impl EmbeddingSpaceRef {
    /// Pin an embedding space. `id`/`revision` name the model; `model_hash`/
    /// `preprocessing_hash` are the caller's own content-addressed identity
    /// for the model weights and preprocessing pipeline; `dimensions` is the
    /// vector width the model emits; `normalize` records whether vectors are
    /// L2-normalized before comparison. `digest` is derived from the other
    /// six fields (std-hashed, NOT a cryptographic digest — nothing here
    /// needs collision resistance against an adversary, only a stable
    /// equality key over the pinned tuple; `medulla-types` stays dependency-free
    /// per the crate's own "depends only on serde" invariant). Returns `Err`
    /// if the resulting space is not well-formed (see [`Self::validate`]).
    pub fn pinned(
        id: impl Into<String>,
        revision: impl Into<String>,
        model_hash: impl Into<String>,
        preprocessing_hash: impl Into<String>,
        dimensions: usize,
        normalize: bool,
    ) -> Result<Self, String> {
        let id = id.into();
        let revision = revision.into();
        let model_hash = model_hash.into();
        let preprocessing_hash = preprocessing_hash.into();
        let digest = pinned_digest(
            &id,
            &revision,
            &model_hash,
            &preprocessing_hash,
            dimensions,
            normalize,
        );
        let space = Self {
            id,
            revision,
            model_hash,
            preprocessing_hash,
            dimensions,
            normalize,
            digest,
            modalities: Vec::new(),
        };
        space.validate()?;
        Ok(space)
    }

    /// MEDULLA-FEDERATED-QUERY-R039.1.1: bind all four modalities into this
    /// one pinned space. The digest folds the bindings in, so two spaces that
    /// differ in any encoder are never comparable. Refuses with
    /// `EMBEDDING_SPACE_INVALID` unless every modality is bound exactly once.
    pub fn with_modalities(mut self, modalities: Vec<ModalityBinding>) -> Result<Self, String> {
        self.modalities = modalities;
        self.validate()?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.digest.hash(&mut hasher);
        for binding in &self.modalities {
            binding.modality.hash(&mut hasher);
            binding.encoder.hash(&mut hasher);
        }
        self.digest = format!("{:016x}", hasher.finish());
        Ok(self)
    }

    /// Structural well-formedness: non-empty identity fields, a non-zero
    /// dimensionality bounded by [`MAX_EMBEDDING_DIMENSIONS`], and a
    /// non-empty digest. Does NOT re-derive `digest` from the other fields —
    /// a value decoded off the wire is trusted to carry the digest it was
    /// constructed with; this only guards against a hostile/corrupt/default
    /// value before it is used to gate a comparison.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("embedding space model id must not be empty".to_string());
        }
        if self.revision.is_empty() {
            return Err("embedding space model revision must not be empty".to_string());
        }
        if self.model_hash.is_empty() {
            return Err("embedding space model hash must not be empty".to_string());
        }
        if self.preprocessing_hash.is_empty() {
            return Err("embedding space preprocessing hash must not be empty".to_string());
        }
        if self.dimensions == 0 {
            return Err("embedding space dimensions must not be zero".to_string());
        }
        if self.dimensions > MAX_EMBEDDING_DIMENSIONS {
            return Err(format!(
                "embedding space dimensions {} exceed the generic maximum of {}",
                self.dimensions, MAX_EMBEDDING_DIMENSIONS
            ));
        }
        if self.digest.is_empty() {
            return Err("embedding space digest must not be empty".to_string());
        }
        self.validate_modalities()
    }

    /// The identity tuple `(id, dimensions, revision)` is checked above; a
    /// multimodal space must additionally bind each of the four modalities
    /// exactly once to a named encoder.
    fn validate_modalities(&self) -> Result<(), String> {
        if self.modalities.is_empty() {
            return Ok(());
        }
        for modality in Modality::ALL {
            let bound = self
                .modalities
                .iter()
                .filter(|binding| binding.modality == modality)
                .count();
            if bound != 1 {
                return Err(format!(
                    "{}: modality {modality:?} is bound {bound} times; \
                     a multimodal space binds each modality exactly once",
                    ServerErrorCode::EmbeddingSpaceInvalid
                ));
            }
        }
        if self.modalities.len() != Modality::ALL.len() {
            return Err(format!(
                "{}: a multimodal space holds exactly {} bindings",
                ServerErrorCode::EmbeddingSpaceInvalid,
                Modality::ALL.len()
            ));
        }
        if self.modalities.iter().any(|b| b.encoder.is_empty()) {
            return Err(format!(
                "{}: a modality binding must name its encoder",
                ServerErrorCode::EmbeddingSpaceInvalid
            ));
        }
        Ok(())
    }
}

fn pinned_digest(
    id: &str,
    revision: &str,
    model_hash: &str,
    preprocessing_hash: &str,
    dimensions: usize,
    normalize: bool,
) -> String {
    // `str`'s std `Hash` impl already writes a length-disambiguating
    // terminator byte after its contents, so four adjacent `&str` fields
    // hash injectively here without any manual separator.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    revision.hash(&mut hasher);
    model_hash.hash(&mut hasher);
    preprocessing_hash.hash(&mut hasher);
    dimensions.hash(&mut hasher);
    normalize.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// A vector paired with the coordinate-space identity of the model that
/// produced it (mirrors [`EmbeddingSpaceRef`]). A bare `Vec<f32>` cannot say
/// which space it belongs to, which is exactly how a mixed-space comparison
/// happens by accident; stamping closes that gap at the type level for
/// callers that want it (`SemanticStore`'s lower-level raw-vector API
/// intentionally has no such requirement — see its module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StampedVector {
    pub space: EmbeddingSpaceRef,
    pub values: Vec<f32>,
}

impl StampedVector {
    /// Stamp `values` with `space`'s identity. The width is checked here so a
    /// short or long vector can never reach a comparison carrying a space
    /// that claims a different shape.
    pub fn new(space: EmbeddingSpaceRef, values: Vec<f32>) -> Result<Self, String> {
        space.validate()?;
        if values.len() != space.dimensions {
            return Err(format!(
                "embedding space `{}` expects {} dimensions but the supplied vector has {}",
                space.digest,
                space.dimensions,
                values.len()
            ));
        }
        Ok(Self { space, values })
    }

    /// Structural validity: the declared space is well-formed, the vector's
    /// width matches it, and every component is finite.
    pub fn validate(&self) -> Result<(), String> {
        self.space.validate()?;
        if self.values.len() != self.space.dimensions {
            return Err(format!(
                "embedding space `{}` expects {} dimensions but the stamped vector has {}",
                self.space.digest,
                self.space.dimensions,
                self.values.len()
            ));
        }
        if let Some(index) = self.values.iter().position(|value| !value.is_finite()) {
            return Err(format!(
                "stamped vector component at index {index} is NaN or infinite (must be finite)"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoding_policy(position_mode: DocumentPositionMode) -> DocumentEncodingPolicy {
        DocumentEncodingPolicy::new(
            "tokenizer-v1",
            128,
            32,
            position_mode,
            DocumentAggregation::MeanChunks,
            "model-v7",
        )
        .unwrap()
    }

    #[test]
    // spec: MEDULLA-FEDERATED-QUERY-R039.2.1
    fn document_encoding_policy_round_trips_position_modes() {
        for mode in [
            DocumentPositionMode::LocalReset,
            DocumentPositionMode::AbsoluteOffset,
        ] {
            let policy = encoding_policy(mode);
            let wire = serde_json::to_string(&policy).unwrap();
            let decoded: DocumentEncodingPolicy = serde_json::from_str(&wire).unwrap();
            assert_eq!(decoded, policy);
            assert_eq!(decoded.validate(), Ok(()));
        }
    }

    #[test]
    // spec: MEDULLA-FEDERATED-QUERY-R039.2.1
    fn document_encoding_policy_rejects_invalid_bounds_and_revision() {
        assert!(DocumentEncodingPolicy::new(
            "tokenizer-v1",
            0,
            0,
            DocumentPositionMode::LocalReset,
            DocumentAggregation::MeanChunks,
            "model-v7",
        )
        .is_err());
        assert!(DocumentEncodingPolicy::new(
            "tokenizer-v1",
            128,
            128,
            DocumentPositionMode::LocalReset,
            DocumentAggregation::MeanChunks,
            "model-v7",
        )
        .is_err());

        let mut unsupported = encoding_policy(DocumentPositionMode::LocalReset);
        unsupported.policy_revision = DOCUMENT_ENCODING_POLICY_REVISION + 1;
        assert!(unsupported.validate().is_err());
    }

    #[test]
    fn document_encoding_policy_requires_tokenizer_and_model_revision() {
        assert!(DocumentEncodingPolicy::new(
            "",
            128,
            32,
            DocumentPositionMode::LocalReset,
            DocumentAggregation::MeanChunks,
            "model-v7",
        )
        .is_err());
        assert!(DocumentEncodingPolicy::new(
            "tokenizer-v1",
            128,
            32,
            DocumentPositionMode::LocalReset,
            DocumentAggregation::MeanChunks,
            "",
        )
        .is_err());
    }

    fn space(dimensions: usize) -> EmbeddingSpaceRef {
        EmbeddingSpaceRef::pinned(
            "model",
            "1",
            "a".repeat(64),
            "b".repeat(64),
            dimensions,
            true,
        )
        .unwrap()
    }

    #[test]
    fn pinned_spaces_with_identical_tuples_share_a_digest() {
        assert_eq!(space(8).digest, space(8).digest);
    }

    #[test]
    fn pinned_spaces_differing_only_in_dimensions_have_distinct_digests() {
        assert_ne!(space(8).digest, space(16).digest);
    }

    #[test]
    fn zero_dimensions_is_rejected() {
        assert!(EmbeddingSpaceRef::pinned("model", "1", "a", "b", 0, true).is_err());
    }

    #[test]
    fn oversized_dimensions_is_rejected() {
        assert!(EmbeddingSpaceRef::pinned(
            "model",
            "1",
            "a",
            "b",
            MAX_EMBEDDING_DIMENSIONS + 1,
            true
        )
        .is_err());
    }

    #[test]
    fn empty_identity_fields_are_rejected() {
        assert!(EmbeddingSpaceRef::pinned("", "1", "a", "b", 8, true).is_err());
        assert!(EmbeddingSpaceRef::pinned("model", "", "a", "b", 8, true).is_err());
        assert!(EmbeddingSpaceRef::pinned("model", "1", "", "b", 8, true).is_err());
        assert!(EmbeddingSpaceRef::pinned("model", "1", "a", "", 8, true).is_err());
    }

    #[test]
    fn stamped_vector_requires_matching_width() {
        assert!(StampedVector::new(space(2), vec![1.0, 0.0]).is_ok());
        assert!(StampedVector::new(space(2), vec![1.0, 0.0, 0.0]).is_err());
    }

    #[test]
    fn stamped_vector_rejects_non_finite_components() {
        let stamped = StampedVector {
            space: space(2),
            values: vec![1.0, f32::NAN],
        };
        assert!(stamped.validate().is_err());
    }

    fn bindings() -> Vec<ModalityBinding> {
        Modality::ALL
            .iter()
            .map(|modality| ModalityBinding {
                modality: *modality,
                encoder: format!("encoder-{modality:?}"),
            })
            .collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.1.1
    #[test]
    fn modality_bindings_round_trip() {
        let multimodal = space(8).with_modalities(bindings()).unwrap();
        assert_eq!(multimodal.modalities.len(), Modality::ALL.len());
        let wire = serde_json::to_string(&multimodal).unwrap();
        let back: EmbeddingSpaceRef = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, multimodal);
        assert!(back.validate().is_ok());
        // The bindings are part of the pinned identity.
        assert_ne!(multimodal.digest, space(8).digest);
        let mut other = bindings();
        other[1].encoder = "another-image-encoder".to_string();
        let different = space(8).with_modalities(other).unwrap();
        assert_ne!(different.digest, multimodal.digest);
        // A single-modality space keeps its wire shape and digest.
        let plain: EmbeddingSpaceRef =
            serde_json::from_str(&serde_json::to_string(&space(8)).unwrap()).unwrap();
        assert!(plain.modalities.is_empty());
        assert_eq!(plain, space(8));
        // Absent identity tuple, a missing or doubled modality: invalid.
        let mut no_identity = multimodal.clone();
        no_identity.revision.clear();
        assert!(no_identity.validate().is_err());
        let mut three = bindings();
        three.pop();
        let err = space(8).with_modalities(three).unwrap_err();
        let typed = ServerErrorCode::EmbeddingSpaceInvalid;
        assert_eq!(typed.as_str(), "EMBEDDING_SPACE_INVALID");
        assert!(err.starts_with(typed.as_str()));
        let mut doubled = bindings();
        doubled[3].modality = Modality::Text;
        assert!(space(8)
            .with_modalities(doubled)
            .unwrap_err()
            .starts_with(typed.as_str()));
        let mut unnamed = bindings();
        unnamed[0].encoder.clear();
        assert!(space(8)
            .with_modalities(unnamed)
            .unwrap_err()
            .starts_with(typed.as_str()));
    }
}
