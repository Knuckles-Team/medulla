//! The value of a media derivation feature (MEDULLA-MEDIA-VOCAB-R650.3).
//!
//! A `SourceIngest` batch carries one `media_derivation` feature per transcript or caption; its
//! `value_msgpack` decodes to [`MediaDerivationValue`]. `medulla-types` stays below
//! `medulla-modality`, so `kind` is the wire string, `transcript` or `caption`.

use serde::{Deserialize, Serialize};

/// One transcript or caption of a media item, with the vector it is recalled by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaDerivationValue {
    pub kind: String,
    pub text: String,
    pub embedding: Vec<f32>,
}
