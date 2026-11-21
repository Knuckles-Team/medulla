//! The value of a media tag feature (MEDULLA-MEDIA-VOCAB-R001.100.2).
//!
//! A `SourceIngest` batch carries one `media_tag` feature per tag; its `value_msgpack` decodes
//! to [`MediaTagValue`]. Collected by `medulla_decision_types::decision::codes::module_tables`
//! (MEDULLA-DECISION-ENGINE-R010.100), so no code here repeats a shared or foreign code.

use serde::{Deserialize, Serialize};

use crate::refusal_code::RefusalCode;

/// Whether a tag names an object in the media or the context it was taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagKind {
    Object,
    Context,
}

/// One tag asserted for a media item, with the provenance that justifies it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaTagValue {
    pub label: String,
    pub origin: String,
    pub taxonomy_id: String,
    pub taxonomy_version: String,
    pub tag_kind: TagKind,
    pub confidence: f64,
    pub producer_id: String,
    pub evidence_ref: String,
    pub media_revision: u64,
}

/// Every refusal code of the media tag ingest.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("TAG_PROVENANCE_MISSING"),
    RefusalCode::new("TAG_MEDIA_REVISION_STALE"),
];
