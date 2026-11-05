//! Corpus retrieval request and result types (MEDULLA-FEDERATED-QUERY-R653.1).
//!
//! One served `CorpusRetrieve` request selects a mode (grep, entity, session or
//! lineage) and returns typed rows. The types are serde-stable; every request
//! runs inside the caller's tenant, so no row carries a tenant of its own.

use serde::{Deserialize, Serialize};

/// Default row cap of a corpus request.
pub const DEFAULT_LIMIT: u32 = 50;
/// Largest accepted row cap.
pub const MAX_LIMIT: u32 = 500;
/// Largest accepted grep pattern, in bytes.
pub const MAX_PATTERN_BYTES: usize = 1_024;

/// The retrieval mode of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusMode {
    Grep,
    Entity,
    Session,
    Lineage,
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

/// A served `CorpusRetrieve` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusRequest {
    pub mode: CorpusMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_ref: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// Why a [`CorpusRequest`] is not well formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusRequestError {
    /// `limit` is outside `[1, MAX_LIMIT]`.
    LimitOutOfRange(u32),
    /// `pattern` is longer than [`MAX_PATTERN_BYTES`].
    PatternTooLong(usize),
}

impl CorpusRequest {
    /// Check the request bounds before any mode runs.
    pub fn validate(&self) -> Result<(), CorpusRequestError> {
        if !(1..=MAX_LIMIT).contains(&self.limit) {
            return Err(CorpusRequestError::LimitOutOfRange(self.limit));
        }
        match &self.pattern {
            Some(pattern) if pattern.len() > MAX_PATTERN_BYTES => {
                Err(CorpusRequestError::PatternTooLong(pattern.len()))
            }
            _ => Ok(()),
        }
    }
}

/// One grep hit in a text field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepMatch {
    pub node_id: String,
    pub field: String,
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub snippet: String,
}

/// One neighbour of an entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Neighbour {
    pub relationship: String,
    pub node_id: String,
}

/// An entity with its neighbourhood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityContext {
    pub entity: String,
    pub hops: u32,
    pub neighbours: Vec<Neighbour>,
}

/// One turn of a session transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHit {
    pub session_id: String,
    pub ordinal: u32,
    pub role: String,
    pub snippet: String,
}

/// One hop of an answer lineage path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageHop {
    pub node_id: String,
    pub relationship: String,
}

/// The path from an answer back to one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineagePath {
    pub answer_ref: String,
    pub hops: Vec<LineageHop>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R653.1
    #[test]
    fn types_round_trip_and_reject_oversize_pattern() {
        let request: CorpusRequest =
            serde_json::from_str(r#"{"mode":"grep","pattern":"fn main","label":"File"}"#).unwrap();
        assert_eq!(request.limit, DEFAULT_LIMIT);
        assert_eq!(request.validate(), Ok(()));
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(
            text,
            r#"{"mode":"grep","pattern":"fn main","label":"File","limit":50}"#
        );
        assert_eq!(
            serde_json::from_str::<CorpusRequest>(&text).unwrap(),
            request
        );
        assert!(serde_json::from_str::<CorpusRequest>(r#"{"mode":"guess"}"#).is_err());
        assert!(serde_json::from_str::<CorpusRequest>(r#"{"mode":"grep","tenant":"t"}"#).is_err());

        let edge = CorpusRequest {
            pattern: Some("a".repeat(MAX_PATTERN_BYTES)),
            limit: MAX_LIMIT,
            ..request.clone()
        };
        assert_eq!(edge.validate(), Ok(()));
        let oversize = CorpusRequest {
            pattern: Some("a".repeat(MAX_PATTERN_BYTES + 1)),
            ..request.clone()
        };
        assert_eq!(
            oversize.validate(),
            Err(CorpusRequestError::PatternTooLong(MAX_PATTERN_BYTES + 1))
        );
        for limit in [0, MAX_LIMIT + 1] {
            let bad = CorpusRequest {
                limit,
                ..request.clone()
            };
            assert_eq!(
                bad.validate(),
                Err(CorpusRequestError::LimitOutOfRange(limit))
            );
        }

        let grep = GrepMatch {
            node_id: "n1".into(),
            field: "body".into(),
            line: 3,
            start: 4,
            end: 11,
            snippet: "fn main()".into(),
        };
        let entity = EntityContext {
            entity: "e1".into(),
            hops: 1,
            neighbours: vec![Neighbour {
                relationship: "MENTIONS".into(),
                node_id: "n2".into(),
            }],
        };
        let session = SessionHit {
            session_id: "s1".into(),
            ordinal: 2,
            role: "user".into(),
            snippet: "hello".into(),
        };
        let lineage = LineagePath {
            answer_ref: "a1".into(),
            hops: vec![LineageHop {
                node_id: "doc-1".into(),
                relationship: "CITES".into(),
            }],
        };
        let back: GrepMatch = serde_json::from_str(&serde_json::to_string(&grep).unwrap()).unwrap();
        assert_eq!(back, grep);
        let back: EntityContext =
            serde_json::from_str(&serde_json::to_string(&entity).unwrap()).unwrap();
        assert_eq!(back, entity);
        let back: SessionHit =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(back, session);
        let back: LineagePath =
            serde_json::from_str(&serde_json::to_string(&lineage).unwrap()).unwrap();
        assert_eq!(back, lineage);
    }
}
