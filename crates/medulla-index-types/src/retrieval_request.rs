//! MEDULLA-FEDERATED-QUERY-R040.1.7 — the served retrieval request: the one carrier
//! of hard constraints, topic fusion and the modality pair on `Method::UnifiedQuery`.
//!
//! The request lives here and not in `medulla-query-types` because the modality pair
//! and the topic space name [`Modality`] and [`EmbeddingSpaceRef`], which this crate
//! owns. Validation runs before any snapshot is taken: a request that fails it runs
//! nothing and answers the failing part's registered code.

use crate::embedding::{EmbeddingSpaceRef, Modality};
use medulla_query_types::wire::RetrievalConstraints;
use medulla_types::contract::ServerErrorCode;
use serde::{Deserialize, Serialize};

/// The retrieval options one served `UnifiedQuery` carries. Every part is optional;
/// a request with no part set binds nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalRequest {
    /// Hard entity and relationship constraints applied before ranking.
    #[serde(default)]
    pub constraints: Option<RetrievalConstraints>,
    /// Topic fusion bounds, corpus revision and embedding space.
    #[serde(default)]
    pub topic: Option<TopicRequest>,
    /// The query and target modality the rank step compares.
    #[serde(default)]
    pub modality: Option<ModalityPair>,
}

/// The topic-fusion part of a [`RetrievalRequest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TopicRequest {
    /// The most topics kept, heaviest first; must be positive.
    pub top_topics: u32,
    /// Topics lighter than this weight are dropped; finite, within `[0, 1]`.
    pub min_weight: f32,
    /// The corpus revision the stored centroids must carry.
    pub corpus_revision: u64,
    /// The embedding space the centroids must live in.
    pub space: EmbeddingSpaceRef,
}

/// The modality of the query and of the ranked targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModalityPair {
    /// The modality the query is encoded in.
    pub query: Modality,
    /// The modality of the items ranked against it.
    pub target: Modality,
}

/// A refused [`RetrievalRequest`]: the registered code and the human message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetrievalRequestError {
    /// A malformed or unsupported constraint predicate (`CONSTRAINT_INVALID`).
    ConstraintInvalid(String),
    /// A topic bound out of range (`TOPIC_WEIGHT_INVALID`).
    TopicWeightInvalid(String),
    /// An embedding space without a valid identity (`EMBEDDING_SPACE_INVALID`).
    EmbeddingSpaceInvalid(String),
}

impl RetrievalRequestError {
    /// The registered refusal code of this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConstraintInvalid(_) => ServerErrorCode::ConstraintInvalid.as_str(),
            Self::TopicWeightInvalid(_) => ServerErrorCode::TopicWeightInvalid.as_str(),
            Self::EmbeddingSpaceInvalid(_) => ServerErrorCode::EmbeddingSpaceInvalid.as_str(),
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::ConstraintInvalid(message)
            | Self::TopicWeightInvalid(message)
            | Self::EmbeddingSpaceInvalid(message) => message,
        }
    }
}

impl std::fmt::Display for RetrievalRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for RetrievalRequestError {}

impl RetrievalRequest {
    /// Check every part that is set. The first failing part decides the error.
    pub fn validate(&self) -> Result<(), RetrievalRequestError> {
        if let Some(constraints) = &self.constraints {
            constraints
                .validate()
                .map_err(RetrievalRequestError::ConstraintInvalid)?;
        }
        if let Some(topic) = &self.topic {
            if topic.top_topics == 0 {
                return Err(RetrievalRequestError::TopicWeightInvalid(
                    "topic top_topics must be positive".to_string(),
                ));
            }
            if !topic.min_weight.is_finite() || !(0.0..=1.0).contains(&topic.min_weight) {
                return Err(RetrievalRequestError::TopicWeightInvalid(
                    "topic min_weight must be finite and within [0, 1]".to_string(),
                ));
            }
            topic
                .space
                .validate()
                .map_err(RetrievalRequestError::EmbeddingSpaceInvalid)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::ModalityBinding;
    use medulla_query_types::wire::{EntityPredicate, PredLiteral};
    use medulla_types::contract::BoundedVec;

    fn space() -> EmbeddingSpaceRef {
        EmbeddingSpaceRef::pinned("model", "1", "hash", "pre", 8, true)
            .unwrap()
            .with_modalities(
                Modality::ALL
                    .iter()
                    .map(|modality| ModalityBinding {
                        modality: *modality,
                        encoder: format!("encoder-{modality:?}"),
                    })
                    .collect(),
            )
            .unwrap()
    }

    fn constraints() -> RetrievalConstraints {
        RetrievalConstraints {
            graph_revision: 7,
            entity_predicates: BoundedVec::new(vec![EntityPredicate::Eq {
                property: "kind".into(),
                value: PredLiteral::Str("person".into()),
            }])
            .unwrap(),
            relationship_predicates: Default::default(),
        }
    }

    fn full() -> RetrievalRequest {
        RetrievalRequest {
            constraints: Some(constraints()),
            topic: Some(TopicRequest {
                top_topics: 3,
                min_weight: 0.25,
                corpus_revision: 11,
                space: space(),
            }),
            modality: Some(ModalityPair {
                query: Modality::Text,
                target: Modality::Image,
            }),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.1.7
    #[test]
    fn request_round_trips_and_refuses_unknown_fields() {
        let request = full();
        request.validate().unwrap();
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<RetrievalRequest>(&json).unwrap(),
            request
        );
        let packed = rmp_serde::to_vec_named(&request).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<RetrievalRequest>(&packed).unwrap(),
            request
        );
        // A request with no part set decodes from an empty object.
        let empty: RetrievalRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(
            empty,
            RetrievalRequest {
                constraints: None,
                topic: None,
                modality: None
            }
        );
        empty.validate().unwrap();

        // An unknown field is refused at every level.
        assert!(serde_json::from_str::<RetrievalRequest>(r#"{"backend":"x"}"#).is_err());
        assert!(serde_json::from_str::<ModalityPair>(
            r#"{"query":"text","target":"image","extra":1}"#
        )
        .is_err());

        let topic_with = |top_topics: u32, min_weight: f32, space: EmbeddingSpaceRef| {
            let mut request = full();
            request.topic = Some(TopicRequest {
                top_topics,
                min_weight,
                corpus_revision: 11,
                space,
            });
            request.validate().unwrap_err()
        };
        let zero = topic_with(0, 0.5, space());
        assert_eq!(zero.code(), "TOPIC_WEIGHT_INVALID");
        let high = topic_with(3, 1.5, space());
        assert_eq!(high.code(), "TOPIC_WEIGHT_INVALID");
        let nan = topic_with(3, f32::NAN, space());
        assert_eq!(nan.code(), "TOPIC_WEIGHT_INVALID");
        let mut bad_space = space();
        bad_space.revision.clear();
        let invalid = topic_with(3, 0.5, bad_space);
        assert_eq!(invalid.code(), "EMBEDDING_SPACE_INVALID");
        assert!(invalid.to_string().starts_with("EMBEDDING_SPACE_INVALID"));

        let mut bad_constraint = full();
        bad_constraint.constraints = Some(RetrievalConstraints {
            graph_revision: 7,
            entity_predicates: BoundedVec::new(vec![EntityPredicate::Eq {
                property: " ".into(),
                value: PredLiteral::Bool(true),
            }])
            .unwrap(),
            relationship_predicates: Default::default(),
        });
        assert_eq!(
            bad_constraint.validate().unwrap_err().code(),
            "CONSTRAINT_INVALID"
        );
    }
}
