//! The search-index capability of an operation-bound foreign source
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.5.1).
//!
//! A registration declares what the bound search operation can do. The planner reads the
//! declaration through `SourceCapabilities::search_push`; a mapping binds a document id to an
//! entity IRI, and the remote score lands in the score channel `remote`.

use serde::{Deserialize, Serialize};

use crate::contract::ServerErrorCode;

/// Smallest and largest `max_k` a search operation may declare.
pub const MAX_K_RANGE: (u32, u32) = (1, 10_000);

/// What the remote score means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScoreKind {
    /// A BM25 relevance score.
    Bm25,
    /// A cosine similarity.
    Cosine,
    /// A score whose scale the source does not state.
    Opaque,
}

/// The search modes, filter support and result cap of one bound search operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SearchIndexCapability {
    /// The operation answers a full-text query.
    pub full_text: bool,
    /// The operation answers a vector query.
    pub vector: bool,
    /// The operation accepts structured filters.
    pub filters: bool,
    /// The most results one request returns, in `[1, 10000]`.
    pub max_k: u32,
    /// What the returned score means.
    pub score_kind: ScoreKind,
}

/// A refused search-capability declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchCapabilityRefusal {
    /// Always `SEARCH_CAPABILITY_INVALID`.
    pub code: ServerErrorCode,
    /// Which rule the declaration broke.
    pub detail: &'static str,
}

impl std::fmt::Display for SearchCapabilityRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for SearchCapabilityRefusal {}

impl SearchIndexCapability {
    /// `max_k` must lie in `[1, 10000]` and at least one search mode must be declared.
    pub fn validate(&self) -> Result<(), SearchCapabilityRefusal> {
        let refuse = |detail| SearchCapabilityRefusal {
            code: ServerErrorCode::SearchCapabilityInvalid,
            detail,
        };
        if !(MAX_K_RANGE.0..=MAX_K_RANGE.1).contains(&self.max_k) {
            return Err(refuse("max_k is outside [1, 10000]"));
        }
        if !self.full_text && !self.vector {
            return Err(refuse("no search mode is declared"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared() -> SearchIndexCapability {
        SearchIndexCapability {
            full_text: true,
            vector: false,
            filters: true,
            max_k: 250,
            score_kind: ScoreKind::Bm25,
        }
    }

    /// Verifies MEDULLA-UNIFIED-DATA-PLANE-R038.5.1: Model: `SearchIndexCapability` on the operation-bound source
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.5.1
    #[test]
    fn search_capability_round_trips() {
        for kind in [ScoreKind::Bm25, ScoreKind::Cosine, ScoreKind::Opaque] {
            let capability = SearchIndexCapability {
                score_kind: kind,
                vector: kind == ScoreKind::Cosine,
                ..declared()
            };
            let json = serde_json::to_string(&capability).unwrap();
            let back: SearchIndexCapability = serde_json::from_str(&json).unwrap();
            assert_eq!(back, capability);
            capability.validate().unwrap();
        }
        let unknown_kind =
            r#"{"full_text":true,"vector":false,"filters":false,"max_k":10,"score_kind":"tfidf"}"#;
        assert!(serde_json::from_str::<SearchIndexCapability>(unknown_kind).is_err());
        let unknown_field = r#"{"full_text":true,"vector":false,"filters":false,"max_k":10,"score_kind":"bm25","extra":1}"#;
        assert!(serde_json::from_str::<SearchIndexCapability>(unknown_field).is_err());
        for max_k in [0, 10_001] {
            let refused = SearchIndexCapability {
                max_k,
                ..declared()
            }
            .validate()
            .unwrap_err();
            assert_eq!(refused.code, ServerErrorCode::SearchCapabilityInvalid);
        }
        let no_mode = SearchIndexCapability {
            full_text: false,
            vector: false,
            ..declared()
        };
        assert_eq!(
            no_mode.validate().unwrap_err().code,
            ServerErrorCode::SearchCapabilityInvalid
        );
    }
}
