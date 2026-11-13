//! Wire types of the served `SemanticEnrich` method
//! (MEDULLA-DECISION-ENGINE-R614.3).
//!
//! Every op names the committed `EnrichmentSchedule` decision that chose its
//! work id `semantic:<op>:<subject>`; a call with no matching decision refuses
//! `ENRICHMENT_NOT_SCHEDULED`. The behaviours themselves are the pure
//! functions of `medulla-enrich`'s `semantic` module.

use serde::{Deserialize, Serialize};

/// The refusal of a call whose schedule decision is absent or chose other work.
pub const ENRICHMENT_NOT_SCHEDULED: &str = "ENRICHMENT_NOT_SCHEDULED";

/// One research concept with its ranking inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConceptInputWire {
    pub concept: String,
    pub impact_bp: u32,
    pub confidence_bp: u32,
    pub cost_units: u32,
}

/// One declared property-to-edge projection rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionRuleWire {
    pub id: String,
    pub taxonomy: String,
    pub property: String,
    pub relation: String,
}

/// One semantic enrichment operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SemanticEnrichOp {
    /// Store a grounded capability card for `subject`.
    Card {
        schedule_record_id: String,
        subject: String,
        summary: String,
        model: String,
        prompt_digest: String,
        source_fact_ids: Vec<String>,
    },
    /// Rank `concepts` into enhancement proposals under `subject`.
    RankProposals {
        schedule_record_id: String,
        subject: String,
        concepts: Vec<ConceptInputWire>,
    },
    /// Classify the Document `subject` into the topics of `taxonomy`.
    Classify {
        schedule_record_id: String,
        subject: String,
        taxonomy: String,
    },
    /// Project the properties of every node labelled `subject` into edges.
    Project {
        schedule_record_id: String,
        subject: String,
        rules: Vec<ProjectionRuleWire>,
    },
}

impl SemanticEnrichOp {
    /// The wire name of the op.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Card { .. } => "card",
            Self::RankProposals { .. } => "rank_proposals",
            Self::Classify { .. } => "classify",
            Self::Project { .. } => "project",
        }
    }

    /// The decision record that scheduled this call.
    pub fn schedule_record_id(&self) -> &str {
        match self {
            Self::Card {
                schedule_record_id, ..
            }
            | Self::RankProposals {
                schedule_record_id, ..
            }
            | Self::Classify {
                schedule_record_id, ..
            }
            | Self::Project {
                schedule_record_id, ..
            } => schedule_record_id,
        }
    }

    /// What the call works on: a symbol, a concept set, a Document or a label.
    pub fn subject(&self) -> &str {
        match self {
            Self::Card { subject, .. }
            | Self::RankProposals { subject, .. }
            | Self::Classify { subject, .. }
            | Self::Project { subject, .. } => subject,
        }
    }

    /// The work id the schedule decision must have chosen:
    /// `semantic:<op>:<subject>`.
    pub fn work_id(&self) -> String {
        format!("semantic:{}:{}", self.name(), self.subject())
    }
}

/// What one served call wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SemanticEnrichReceipt {
    pub op: String,
    pub subject: String,
    pub schedule_record_id: String,
    /// Ids of the nodes written (card, proposals or assignment).
    pub node_ids: Vec<String>,
    /// Edges written (a projection's edges, an assignment's link).
    pub edge_count: u32,
    /// Property values of a projection that named no node.
    pub unresolved: u32,
    /// The topic a `Classify` chose, with its confidence in basis points.
    pub topic: Option<String>,
    pub confidence_bp: Option<u32>,
    /// True when a `Project` hit the batch knob and more nodes remain.
    pub more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R614.3
    #[test]
    fn work_id_names_op_and_subject_and_unknown_fields_refuse() {
        let op = SemanticEnrichOp::Classify {
            schedule_record_id: "rec-1".into(),
            subject: "doc:1".into(),
            taxonomy: "subjects".into(),
        };
        assert_eq!(op.work_id(), "semantic:classify:doc:1");
        assert_eq!(op.schedule_record_id(), "rec-1");
        let wire = serde_json::to_value(&op).unwrap();
        assert_eq!(
            serde_json::from_value::<SemanticEnrichOp>(wire.clone()).unwrap(),
            op
        );
        let mut extra = wire;
        extra["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<SemanticEnrichOp>(extra).is_err());
    }
}
