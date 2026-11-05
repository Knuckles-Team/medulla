//! Query understanding model (MEDULLA-FEDERATED-QUERY-R651.1).
//!
//! The typed filters, relational intent and the one expansion port's request
//! and result shapes that served `Discover` mode `deep` hands to `understand`.
//! Stopping is `StopVerdict` (MEDULLA-FEDERATED-QUERY-R082); no stop type
//! lives here.

use medulla_result_types::retrieval_verdict::TermFeedback;
use serde::{Deserialize, Serialize};

/// Largest query text an expansion request accepts, in bytes.
pub const MAX_QUERY_BYTES: usize = 2_048;
/// Smallest and largest number of perspectives one request may ask for.
pub const PERSPECTIVES_MIN: u8 = 1;
pub const PERSPECTIVES_MAX: u8 = 8;

/// Structured filters lifted out of the query text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryFilters {
    pub types: Vec<String>,
    pub sources: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// A subject-relation-object reading of the query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationalIntent {
    pub subject: String,
    pub relation: String,
    pub object: String,
}

/// One request to the expansion port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpansionRequest {
    pub query: String,
    pub perspectives: u8,
    #[serde(default)]
    pub feedback: Vec<TermFeedback>,
}

impl ExpansionRequest {
    /// Refuse an empty or oversized query and a perspective count outside 1 to 8.
    pub fn validate(&self) -> Result<(), ExpansionError> {
        if self.query.trim().is_empty() {
            return Err(ExpansionError::EmptyQuery);
        }
        if self.query.len() > MAX_QUERY_BYTES {
            return Err(ExpansionError::QueryTooLong {
                bytes: self.query.len(),
            });
        }
        if !(PERSPECTIVES_MIN..=PERSPECTIVES_MAX).contains(&self.perspectives) {
            return Err(ExpansionError::PerspectivesOutOfRange {
                perspectives: self.perspectives,
            });
        }
        Ok(())
    }
}

/// The closed set of expansion kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpansionKind {
    Hypothetical,
    SubQuery,
    Perspective,
}

/// One expansion the port produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expansion {
    pub text: String,
    pub kind: ExpansionKind,
    pub digest: String,
}

/// Why an expansion request was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ExpansionError {
    EmptyQuery,
    QueryTooLong { bytes: usize },
    PerspectivesOutOfRange { perspectives: u8 },
}

impl std::fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyQuery => write!(f, "expansion query is empty"),
            Self::QueryTooLong { bytes } => {
                write!(
                    f,
                    "expansion query is {bytes} bytes, over {MAX_QUERY_BYTES}"
                )
            }
            Self::PerspectivesOutOfRange { perspectives } => write!(
                f,
                "perspectives {perspectives} outside {PERSPECTIVES_MIN} to {PERSPECTIVES_MAX}"
            ),
        }
    }
}

impl std::error::Error for ExpansionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_result_types::retrieval_verdict::FeedbackTier;

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value);
    }

    #[test]
    fn types_round_trip_and_reject_unknown_kind() {
        round_trip(&QueryFilters {
            types: vec!["Document".into()],
            sources: vec!["wiki".into()],
            tags: vec!["ops".into()],
            after: Some("2026-01-01".into()),
            before: None,
        });
        round_trip(&RelationalIntent {
            subject: "a".into(),
            relation: "founded".into(),
            object: "b".into(),
        });
        let request = ExpansionRequest {
            query: "who founded b".into(),
            perspectives: 3,
            feedback: vec![TermFeedback {
                term: "founder".into(),
                tier: FeedbackTier::Reinforce,
                weight: 0.5,
                evidence_document_ids: vec!["d1".into()],
            }],
        };
        round_trip(&request);
        assert_eq!(request.validate(), Ok(()));
        for kind in [
            ExpansionKind::Hypothetical,
            ExpansionKind::SubQuery,
            ExpansionKind::Perspective,
        ] {
            round_trip(&Expansion {
                text: "x".into(),
                kind,
                digest: "0".repeat(64),
            });
        }
        assert_eq!(
            serde_json::to_value(ExpansionKind::SubQuery).unwrap(),
            serde_json::json!("sub_query")
        );
        round_trip(&ExpansionError::QueryTooLong { bytes: 3_000 });

        let unknown = r#"{"text":"x","kind":"paraphrase","digest":"d"}"#;
        assert!(serde_json::from_str::<Expansion>(unknown).is_err());

        for bad in [0, 9] {
            let r = ExpansionRequest {
                perspectives: bad,
                ..request.clone()
            };
            assert_eq!(
                r.validate(),
                Err(ExpansionError::PerspectivesOutOfRange { perspectives: bad })
            );
        }
        let long = ExpansionRequest {
            query: "q".repeat(MAX_QUERY_BYTES + 1),
            ..request.clone()
        };
        assert_eq!(
            long.validate(),
            Err(ExpansionError::QueryTooLong {
                bytes: MAX_QUERY_BYTES + 1
            })
        );
        let edge = ExpansionRequest {
            query: "q".repeat(MAX_QUERY_BYTES),
            perspectives: 8,
            ..request
        };
        assert_eq!(edge.validate(), Ok(()));
    }
}
