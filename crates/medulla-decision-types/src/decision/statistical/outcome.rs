//! What a statistical decision concluded, and what guarantee it carries.
//!
//! An engine may ACT only when it can state a risk level and the calibration
//! sample behind it. Otherwise it answers advisory scores marked uncalibrated,
//! or it abstains with the same typed reasons the assembly layer uses.

use serde::{Deserialize, Serialize};

use super::super::numeric::{QuantisedValue, UnitRationalWire};
use super::super::record::AbstainReason;
use medulla_types::contract::BoundedVec;

/// Which controlled procedure produced the risk statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RiskMethod {
    LearnThenTest,
    ConformalRiskControl,
}

/// The risk guarantee an acted-on decision carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RiskStatement {
    pub method: RiskMethod,
    /// Risk level the procedure controls.
    pub epsilon: UnitRationalWire,
    /// Failure probability of that control.
    pub delta: UnitRationalWire,
    pub n_calibration: u64,
}

/// How the scores were calibrated, if at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CalibrationMethod {
    /// Split-conformal acceptability sets (and a Learn-then-Test act
    /// threshold) over temperature-scaled probabilities: the calibration rung
    /// whenever its coverage claim is non-trivial (EH-293).
    Conformal,
    /// Temperature scaling alone: the fallback when no conformal claim holds.
    Temperature,
    Vector,
    Dirichlet,
    Isotonic,
    None,
}

/// The calibration behind a decision's probabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CalibrationStatement {
    pub method: CalibrationMethod,
    #[serde(default)]
    pub alpha: Option<UnitRationalWire>,
    #[serde(default)]
    pub coverage_lower: Option<UnitRationalWire>,
    #[serde(default)]
    pub coverage_upper: Option<UnitRationalWire>,
    pub n_calibration: u64,
    /// True when the calibration set was synthetic.
    pub synthetic: bool,
}

/// One scored option of an advisory answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScoredOption {
    pub option_id: String,
    pub score: QuantisedValue,
    /// Present only when the score is calibrated into a probability.
    #[serde(default)]
    pub probability: Option<UnitRationalWire>,
}

/// The conclusion of one statistical decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StatisticalOutcome {
    /// An option was chosen, with the propensity it was chosen under and the
    /// risk level that choice controls.
    Acted {
        option_id: String,
        propensity: UnitRationalWire,
        prediction_set: BoundedVec<String, 64>,
        risk: RiskStatement,
    },
    /// An option chosen by the declared exploration budget, not by a score:
    /// the propensity is the executed policy's exact probability, and no risk
    /// claim is made.
    Explored {
        option_id: String,
        propensity: UnitRationalWire,
    },
    /// Scores only. `calibrated` says whether they mean anything numerically.
    /// `scores` holds at most the top 64 options; `ranked_total` is how many
    /// candidates were ranked to produce them (MEDULLA-DECISION-ENGINE-R829.1),
    /// equal to `scores.len()` unless a rank catalog named more than 64.
    Advisory {
        scores: BoundedVec<ScoredOption, 64>,
        calibrated: bool,
        ranked_total: u32,
    },
    /// Below the policy's floor; the same typed reasons as an assembly.
    Abstained {
        reasons: BoundedVec<AbstainReason, 64>,
    },
}

impl StatisticalOutcome {
    /// An advisory over `scores` alone: every ranked candidate is scored, so
    /// `ranked_total` is the number of scores.
    pub fn advisory(scores: BoundedVec<ScoredOption, 64>, calibrated: bool) -> Self {
        let ranked_total = scores.as_slice().len() as u32;
        Self::Advisory {
            scores,
            calibrated,
            ranked_total,
        }
    }
}

/// The exact per-feature logit contributions of one option under a linear
/// head: `weight x standardised value`, in schema order. Not an explanation of
/// the calibrated probability or of the softmax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LinearExplanation {
    pub option_id: String,
    pub contributions: BoundedVec<QuantisedValue, 32>,
}

/// One audit-sampling draw. An acted decision is sampled into human review
/// with a KNOWN inclusion probability, so review results re-enter calibration
/// weighted by its inverse and the review queue carries no selection bias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AuditDraw {
    pub inclusion_probability: UnitRationalWire,
    pub sampled: bool,
}

/// The one typed answer a question family concludes beyond the shared
/// outcome (MEDULLA-DECISION-ENGINE-R001.101.5). A family adds one variant
/// here; the record carries it as `StatisticalDecisionRecord.answer`, never
/// as a per-family `Option` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FamilyAnswer {
    /// The evaluate-only explicit task DAG of the `task_plan` family.
    TaskPlan(Box<crate::decision::task_plan::TaskPlan>),
    /// The proven swarm topology of the `topology` family (MEDULLA-DECISION-ENGINE-R111.1).
    Topology(Box<medulla_agent_types::agent_topology::TopologyPlan>),
    /// The expansion choice or stop of the `thought_search` family
    /// (MEDULLA-DECISION-ENGINE-R750.754).
    ThoughtSearch(Box<crate::decision::thought_search::ThoughtSearchAnswer>),
    /// The budgeted plan of the `comparative_analysis_plan` family
    /// (MEDULLA-DECISION-ENGINE-R139.3).
    ComparativeAnalysis(Box<crate::decision::comparative_analysis::ComparativeAnalysisPlan>),
    /// The designated agents, tools and skills of the `capability_designation`
    /// family (MEDULLA-DECISION-ENGINE-R172.7).
    CapabilityDesignation(Box<crate::decision::capability_designation::CapabilityDesignation>),
    /// The bounded tool, skill and agent set of the `load_set` family
    /// (MEDULLA-DECISION-ENGINE-R176.3).
    LoadSet(Box<crate::decision::load_set::LoadSetAnswer>),
    /// The ordered steps, prompts and cautions of the `usage_guidance` family
    /// (MEDULLA-DECISION-ENGINE-R177.3).
    UsageGuidance(Box<crate::decision::usage_guidance::UsageGuidance>),
    /// The receipt of an admitted `skill_upgrade` (MEDULLA-DECISION-ENGINE-R167.3).
    SkillUpgrade(Box<crate::decision::skill_upgrade_request::SkillUpgradeReceipt>),
    /// An admitted plan of the `planning_admission` family (MEDULLA-DECISION-ENGINE-R171.3).
    PlanningAdmission(Box<crate::decision::planning_admission::PlanningAdmissionAnswer>),
    /// The why-not of every non-chosen target of a `placement` question
    /// (MEDULLA-PLACEMENT-R001.9).
    Placement(Box<crate::decision::statistical::placement::PlacementAnswer>),
    /// The sealed service-to-host plan of a `placement_plan` question
    /// (MEDULLA-PLACEMENT-R700.3).
    PlacementPlan(Box<crate::decision::statistical::placement_plan::PlacementPlan>),
    /// The per-step split of a trajectory reward (MEDULLA-DECISION-ENGINE-R751.755).
    RewardDecomposition(Box<crate::decision::reward::TrajectoryReward>),
    /// The ordered reads and stop reason of a research walk (MEDULLA-DECISION-ENGINE-R705.3).
    Research(Box<crate::decision::research_voi::ResearchAnswer>),
    /// The matches, merge clusters and gap plans of a feature comparison (MEDULLA-DECISION-ENGINE-R707.3).
    FeatureMatch(Box<crate::decision::feature_match::FeatureMatchAnswer>),
    /// Whether a curriculum stage is promoted (MEDULLA-DECISION-ENGINE-R751.754).
    CurriculumPromotion(Box<crate::decision::curriculum::PromotionReading>),
    /// The corrigibility verdict of an action class (MEDULLA-DECISION-ENGINE-R752.755).
    Autonomy(Box<crate::decision::autonomy::AutonomyReading>),
    /// The drift reading between two windows (MEDULLA-DECISION-ENGINE-R752.754).
    PopulationDrift(Box<crate::decision::population::DriftReading>),
    /// The replica width a fan-out should use (MEDULLA-DECISION-ENGINE-R752.754).
    FanoutWidth { width: u32 },
    /// The winning trajectory of a join (MEDULLA-DECISION-ENGINE-R753.754).
    TrajectoryJoin(Box<crate::decision::trajectory_verdict::DistilledTrajectory>),
    /// A step verdict and its recovery (MEDULLA-DECISION-ENGINE-R753.755).
    StepVerdict(Box<crate::decision::trajectory_verdict::StepVerification>),
    /// Whether a candidate is admitted (MEDULLA-DECISION-ENGINE-R753.755).
    Admission(crate::decision::trajectory_verdict::AdmissionVerdict),
    /// The typed instruction-disclosure event of the `instruction_disclosure`
    /// family's event mode (MEDULLA-DECISION-ENGINE-R143.3). The prior-event
    /// read of the next request finds it here.
    DisclosureEvent(Box<crate::decision::instruction_disclosure::DisclosureEvent>),
    /// The three-part harness configuration with the evidence it rests on, for
    /// the `harness_config` family (MEDULLA-DECISION-ENGINE-R128.12). The record
    /// seals it as `inputs.harness_evaluation`, not as `answer`.
    HarnessConfig(Box<super::harness_config::HarnessConfigEvaluationEvidence>),
    /// The triage of one defect, or its abstention (MEDULLA-SDLC-R004.3).
    DefectTriage(Box<crate::decision::defect_triage::TriageDecision>),
    /// The program a skill document's typed draft compiles to (MEDULLA-DECISION-ENGINE-R156.3).
    /// Committing the record stores it under `skill:<document digest>`.
    SkillWorkflow(Box<crate::decision::program_store::CompiledProgram>),
}

impl FamilyAnswer {
    /// The task plan this answer carries.
    pub fn task_plan(&self) -> Option<&crate::decision::task_plan::TaskPlan> {
        match self {
            Self::TaskPlan(plan) => Some(plan),
            _ => None,
        }
    }

    /// The designation this answer carries.
    pub fn capability_designation(
        &self,
    ) -> Option<&crate::decision::capability_designation::CapabilityDesignation> {
        match self {
            Self::CapabilityDesignation(designation) => Some(designation),
            _ => None,
        }
    }

    /// The disclosure event this answer carries.
    pub fn disclosure_event(
        &self,
    ) -> Option<&crate::decision::instruction_disclosure::DisclosureEvent> {
        match self {
            Self::DisclosureEvent(event) => Some(event),
            _ => None,
        }
    }

    /// The harness-config evidence this answer carries.
    pub fn harness_config(
        &self,
    ) -> Option<&super::harness_config::HarnessConfigEvaluationEvidence> {
        match self {
            Self::HarnessConfig(evidence) => Some(evidence),
            _ => None,
        }
    }
    /// The load set this answer carries.
    pub fn load_set(&self) -> Option<&crate::decision::load_set::LoadSetAnswer> {
        match self {
            Self::LoadSet(answer) => Some(answer),
            _ => None,
        }
    }

    /// The topology plan this answer carries.
    pub fn topology_plan(&self) -> Option<&medulla_agent_types::agent_topology::TopologyPlan> {
        match self {
            Self::Topology(plan) => Some(plan),
            _ => None,
        }
    }

    /// The thought-search answer this answer carries.
    pub fn thought_search(&self) -> Option<&crate::decision::thought_search::ThoughtSearchAnswer> {
        match self {
            Self::ThoughtSearch(answer) => Some(answer),
            _ => None,
        }
    }

    /// The comparative-analysis plan this answer carries.
    pub fn comparative_analysis_plan(
        &self,
    ) -> Option<&crate::decision::comparative_analysis::ComparativeAnalysisPlan> {
        match self {
            Self::ComparativeAnalysis(plan) => Some(plan),
            _ => None,
        }
    }

    /// The usage guidance this answer carries.
    pub fn usage_guidance(&self) -> Option<&crate::decision::usage_guidance::UsageGuidance> {
        match self {
            Self::UsageGuidance(guidance) => Some(guidance),
            _ => None,
        }
    }

    /// The skill upgrade receipt this answer carries.
    pub fn skill_upgrade_receipt(
        &self,
    ) -> Option<&crate::decision::skill_upgrade_request::SkillUpgradeReceipt> {
        match self {
            Self::SkillUpgrade(receipt) => Some(receipt),
            _ => None,
        }
    }

    /// The planning admission answer this answer carries.
    pub fn planning_admission(
        &self,
    ) -> Option<&crate::decision::planning_admission::PlanningAdmissionAnswer> {
        match self {
            Self::PlanningAdmission(value) => Some(value),
            _ => None,
        }
    }

    /// The placement why-not this answer carries.
    pub fn placement(&self) -> Option<&crate::decision::statistical::placement::PlacementAnswer> {
        match self {
            Self::Placement(value) => Some(value),
            _ => None,
        }
    }

    /// The placement plan this answer carries.
    pub fn placement_plan(
        &self,
    ) -> Option<&crate::decision::statistical::placement_plan::PlacementPlan> {
        match self {
            Self::PlacementPlan(value) => Some(value),
            _ => None,
        }
    }

    /// The research walk this answer carries.
    pub fn research(&self) -> Option<&crate::decision::research_voi::ResearchAnswer> {
        match self {
            Self::Research(answer) => Some(answer),
            _ => None,
        }
    }

    /// The feature comparison this answer carries.
    pub fn feature_match(&self) -> Option<&crate::decision::feature_match::FeatureMatchAnswer> {
        match self {
            Self::FeatureMatch(answer) => Some(answer),
            _ => None,
        }
    }

    /// The promotion reading this answer carries.
    pub fn curriculum_promotion(&self) -> Option<&crate::decision::curriculum::PromotionReading> {
        match self {
            Self::CurriculumPromotion(reading) => Some(reading),
            _ => None,
        }
    }

    /// The autonomy reading this answer carries.
    pub fn autonomy(&self) -> Option<&crate::decision::autonomy::AutonomyReading> {
        match self {
            Self::Autonomy(reading) => Some(reading),
            _ => None,
        }
    }

    /// The trajectory reward this answer carries.
    pub fn reward_decomposition(&self) -> Option<&crate::decision::reward::TrajectoryReward> {
        match self {
            Self::RewardDecomposition(reward) => Some(reward),
            _ => None,
        }
    }

    /// The drift reading this answer carries.
    pub fn population_drift(&self) -> Option<&crate::decision::population::DriftReading> {
        match self {
            Self::PopulationDrift(reading) => Some(reading),
            _ => None,
        }
    }

    /// The fan-out width this answer carries.
    pub fn fanout_width(&self) -> Option<u32> {
        match self {
            Self::FanoutWidth { width } => Some(*width),
            _ => None,
        }
    }

    /// The distilled trajectory this answer carries.
    pub fn trajectory_join(
        &self,
    ) -> Option<&crate::decision::trajectory_verdict::DistilledTrajectory> {
        match self {
            Self::TrajectoryJoin(distilled) => Some(distilled),
            _ => None,
        }
    }

    /// The step verification this answer carries.
    pub fn step_verdict(&self) -> Option<&crate::decision::trajectory_verdict::StepVerification> {
        match self {
            Self::StepVerdict(verification) => Some(verification),
            _ => None,
        }
    }

    /// The admission verdict this answer carries.
    pub fn admission(&self) -> Option<crate::decision::trajectory_verdict::AdmissionVerdict> {
        match self {
            Self::Admission(verdict) => Some(*verdict),
            _ => None,
        }
    }
}
