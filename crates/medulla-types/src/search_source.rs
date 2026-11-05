//! Declared search-index capability of an operation-bound foreign source
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.5.1). A registration asserts what the bound
//! search operation can rank; the remote score lands in the `remote` score
//! channel of `OpKind::ForeignScan`. All types are `Copy` and refuse unknown
//! fields.

use serde::{Deserialize, Serialize};

/// Refusal code for a capability with no mode declared or `max_k` out of range.
pub const SEARCH_CAPABILITY_INVALID: &str = "SEARCH_CAPABILITY_INVALID";

/// Smallest and largest accepted `max_k`.
pub const SEARCH_MAX_K_RANGE: std::ops::RangeInclusive<u32> = 1..=10_000;

/// The kind of score a remote search operation returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScoreKind {
    Bm25,
    Cosine,
    Opaque,
}

/// What a remote search operation declares it can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SearchIndexCapability {
    pub full_text: bool,
    pub vector: bool,
    pub filters: bool,
    pub max_k: u32,
    pub score_kind: ScoreKind,
}

/// A typed refusal of an invalid capability declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{SEARCH_CAPABILITY_INVALID}: {reason}")]
pub struct SearchCapabilityError {
    pub reason: &'static str,
}

impl SearchIndexCapability {
    /// Refuses `max_k` outside `[1, 10000]` and a capability declaring no mode.
    pub fn validate(&self) -> Result<(), SearchCapabilityError> {
        if !self.full_text && !self.vector {
            return Err(SearchCapabilityError {
                reason: "no search mode declared",
            });
        }
        if !SEARCH_MAX_K_RANGE.contains(&self.max_k) {
            return Err(SearchCapabilityError {
                reason: "max_k outside [1, 10000]",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_capability_round_trips() {
        let cap = SearchIndexCapability {
            full_text: true,
            vector: false,
            filters: true,
            max_k: 50,
            score_kind: ScoreKind::Bm25,
        };
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(
            serde_json::from_str::<SearchIndexCapability>(&json).unwrap(),
            cap
        );
        for kind in [ScoreKind::Bm25, ScoreKind::Cosine, ScoreKind::Opaque] {
            let c = SearchIndexCapability {
                score_kind: kind,
                ..cap
            };
            let back: SearchIndexCapability =
                serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
            assert_eq!(back, c);
        }
        assert!(cap.validate().is_ok());
        let bad_kind =
            r#"{"full_text":true,"vector":false,"filters":false,"max_k":5,"score_kind":"tfidf"}"#;
        assert!(serde_json::from_str::<SearchIndexCapability>(bad_kind).is_err());
        let extra = r#"{"full_text":true,"vector":false,"filters":false,"max_k":5,"score_kind":"bm25","x":1}"#;
        assert!(serde_json::from_str::<SearchIndexCapability>(extra).is_err());
        for max_k in [0, 10_001] {
            let err = SearchIndexCapability { max_k, ..cap }
                .validate()
                .unwrap_err();
            assert!(err.to_string().starts_with(SEARCH_CAPABILITY_INVALID));
        }
        let none = SearchIndexCapability {
            full_text: false,
            vector: false,
            ..cap
        };
        assert!(none.validate().is_err());
    }
}
