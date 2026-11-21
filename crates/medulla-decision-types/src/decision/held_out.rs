//! The subjects a revision revises and the held-out evidence that qualifies a
//! challenger (MEDULLA-DECISION-ENGINE-R805.7, R805.10, R813.1). They live
//! below the outcome types so the governed pointer and the learning write can
//! name a subject and return evidence; `medulla-decision-outcome-types`
//! re-exports them as `decision::promotion`.

use serde::{Deserialize, Serialize};

/// The schema version of [`HeldOutEvidence`].
pub const HELD_OUT_EVIDENCE_SCHEMA_VERSION: u16 = 1;

/// What kind of definition a `Definition` subject names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DefinitionKind {
    Agent,
    Workflow,
    Team,
    HarnessComponent,
    HarnessBundle,
}

impl DefinitionKind {
    /// The snake_case text used in a pointer key.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Workflow => "workflow",
            Self::Team => "team",
            Self::HarnessComponent => "harness_component",
            Self::HarnessBundle => "harness_bundle",
        }
    }
}

/// The thing a revision revises; each subject has one champion pointer.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RevisionSubject {
    DecisionHead {
        component_id: String,
    },
    Program {
        program_ref: String,
    },
    KnobSet {
        policy_id: String,
    },
    ModelPolicy {
        family_ref: String,
    },
    Definition {
        definition_kind: DefinitionKind,
        definition_ref: String,
    },
    GovernedSet {
        scope_ref: String,
    },
    MeasuredFact {
        model_ref: String,
        property: String,
    },
    /// The rerank calibration curve of one scorer version (R822).
    RerankCalibration {
        scorer_version: String,
    },
    /// A retrieval strategy, adopted by a paired held-out run (R821).
    RetrievalStrategy {
        strategy_ref: String,
    },
}

impl RevisionSubject {
    /// The pointer key of this subject's champion: `champion:<kind>:<ref>`.
    pub fn pointer_key(&self) -> String {
        match self {
            Self::DecisionHead { component_id } => format!("champion:decision_head:{component_id}"),
            Self::Program { program_ref } => format!("champion:program:{program_ref}"),
            Self::KnobSet { policy_id } => format!("champion:knob_set:{policy_id}"),
            Self::ModelPolicy { family_ref } => format!("champion:model_policy:{family_ref}"),
            Self::Definition {
                definition_kind,
                definition_ref,
            } => {
                format!(
                    "champion:definition:{}:{definition_ref}",
                    definition_kind.as_str()
                )
            }
            Self::GovernedSet { scope_ref } => format!("champion:governed_set:{scope_ref}"),
            Self::MeasuredFact {
                model_ref,
                property,
            } => {
                format!("champion:measured_fact:{model_ref}:{property}")
            }
            Self::RerankCalibration { scorer_version } => {
                format!("champion:rerank_calibration:{scorer_version}")
            }
            Self::RetrievalStrategy { strategy_ref } => {
                format!("champion:retrieval_strategy:{strategy_ref}")
            }
        }
    }
}

/// The kind of receipt a held-out evidence record was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReceiptKind {
    DecisionEval,
    ProgramEvaluation,
    PolicyEvaluation,
    RolloutSet,
    MeasurementRun,
    ReviewApproval,
}

/// Who graded a score. Only a model judge never decides a promotion (R813.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GraderKind {
    Annotation,
    Human,
    ModelJudge,
    TraceMetric,
    Api,
}

impl GraderKind {
    /// Whether a score of this grader may decide a promotion.
    pub const fn decides_promotion(self) -> bool {
        !matches!(self, Self::ModelJudge)
    }
}

/// When a measured fact's run and its champion's run were observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MeasurementWindow {
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_observed_at_ms: Option<u64>,
}

/// An operator's review of a governed-set revision: two principal strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReviewApproval {
    pub author: String,
    pub reviewer: String,
}

/// The measured evidence that qualifies a challenger against its champion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HeldOutEvidence {
    pub schema_version: u16,
    pub subject: RevisionSubject,
    pub challenger_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_ref: Option<String>,
    pub receipt_kind: ReceiptKind,
    pub receipt_digest: String,
    pub evaluation_set_digest: String,
    pub challenger_score_ppm: i64,
    /// Absent when the subject has no champion to compare with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_score_ppm: Option<i64>,
    pub delta_ppm: i64,
    pub case_count: u64,
    pub independent: bool,
    /// Who graded the score.
    pub grader_kind: GraderKind,
    /// Run times of a `MeasuredFact` challenger and champion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<MeasurementWindow>,
    /// The review record of a `GovernedSet` challenger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewApproval>,
}

