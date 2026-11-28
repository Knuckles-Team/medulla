//! `Method::Decide`: a statistical decision over library or graph candidates,
//! and the record version 2 it produces.
//!
//! This surface is EVALUATE-ONLY in 2.27.x. It reads candidates, scores them
//! under a pinned feature schema and head, and returns a batch of records; it
//! commits nothing. Whether a record is durable, and where, is a separate
//! decision with its own method.

pub mod feedback;
pub mod harness_config;
pub mod head;
pub mod log;
pub mod nl;
pub mod outcome;
pub mod placement;
pub mod placement_plan;
pub mod retrieval;
pub mod retrieval_adapter;
pub mod retrieval_calibration;
pub mod retrieval_generation;
pub mod retrieval_plan;
pub mod run_credit;
pub mod scorer;
pub mod tool_subset;

use serde::{Deserialize, Serialize};

use super::request::LibraryCandidateScope;
use medulla_agent_types::agent_component::ComponentDependency;
use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_statistical_types::decision::errors::DecisionErrorCode;
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;
use {
    crate::decision::record::CandidateSourceRecord, crate::decision::record::ResolutionKind,
    crate::decision::record::TraceFidelity,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
};
use {
    medulla_decision_statistical_types::decision::numeric::QuantScaleTag,
    medulla_decision_statistical_types::decision::numeric::UnitRationalWire,
};

pub use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
pub use medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind;
pub use outcome::{
    AuditDraw, CalibrationMethod, CalibrationStatement, LinearExplanation, RiskMethod,
    RiskStatement, ScoredOption, StatisticalOutcome,
};

/// The largest record batch one `Decide` may return.
pub const MAX_DECIDE_RECORDS: u16 = 256;

/// What is at stake. Exploration is permitted only for
/// [`QuestionSafety::Ordinary`]; everything else must be decided on evidence
/// already in hand or abstained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum QuestionSafety {
    Ordinary,
    Security,
    Policy,
    WriteBack,
    Irreversible,
}

/// One named, classified question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StatisticalQuestion {
    pub question_id: String,
    pub kind: QuestionKind,
    pub safety: QuestionSafety,
}

/// Where the options come from.
/// Not `Eq`: the cross-modal plan carries floats, so equality on a graph-sourced
/// candidate scope is partial, exactly as it is on `Method` itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CandidateSource {
    /// A tenant-bound agent library scope.
    AgentLibrary { scope: LibraryCandidateScope },
    /// An RLS-filtered cross-modal plan over one named graph -- the same
    /// public plan type `Method::UnifiedQuery` accepts, so there is one plan
    /// vocabulary. The plan only names the candidate id set; it may not rank
    /// or truncate before visibility, because a top-k taken over invisible
    /// rows would move visible ones.
    #[cfg(feature = "query")]
    Graph {
        graph: String,
        plan: Box<medulla_query_types::wire::Plan>,
    },
    /// The caller's own options, each fact a claim the caller makes
    /// ([`declared`]). Visible to the declaring principal only.
    Declared {
        options: BoundedVec<
            medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
            64,
        >,
    },
    /// A capability catalog sent whole for ranking (MEDULLA-DECISION-ENGINE-R829.1):
    /// one declared option per candidate, texts only. A `Vec`, not a
    /// `BoundedVec`, so a set above
    /// [`declared::MAX_RANK_CATALOG_CANDIDATES`] decodes and is refused by value.
    RankCatalog {
        options: Vec<
            medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
        >,
    },
}

/// A typed question parameter. Scalar values are never interpolated into queries.
/// An explicit task proposal carries prompts as data for the host executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TypedValue {
    Bool(bool),
    Int(i64),
    Text(String),
    Iri(String),
    IriList(BoundedVec<String, 64>),
    Rational(UnitRationalWire),
    /// Explicit task claims, never policy or authorization.
    TaskPlan(Box<medulla_decision_program_types::decision::task_plan::TaskPlanProposal>),
    /// Subtasks and specialist scores the harness produced; medulla derives
    /// the plan (MEDULLA-DECISION-ENGINE-R134.3).
    TaskDecomposition(Box<medulla_decision_program_types::decision::task_plan::DecomposeFacts>),
    /// Atomic subtasks to cover with library skills (MEDULLA-DECISION-ENGINE-R134.4).
    SkillPlanQuery(Box<medulla_decision_statistical_types::decision::task_plan::skill_plan::SkillPlanQuery>),
    /// A context-package request (MEDULLA-DECISION-ENGINE-R137.7).
    ContextPackages(Box<super::context_package::ContextPackageRequest>),
    /// A standalone topology question (MEDULLA-DECISION-ENGINE-R111.1).
    Topology(Box<medulla_decision_statistical_types::decision::statistical::topology::TopologyQuestion>),
    /// A placement question (MEDULLA-PLACEMENT-R001.7).
    Placement(Box<placement::PlacementQuestion>),
    /// A placement-plan question (MEDULLA-PLACEMENT-R700.3).
    PlacementPlan(Box<placement_plan::PlacementPlanQuestion>),
    /// One trajectory's reward and step signals (MEDULLA-DECISION-ENGINE-R751.755).
    RewardDecomposition(Box<medulla_decision_statistical_types::decision::reward::RewardDecompositionRequest>),
    /// A stage and its promotion policy (MEDULLA-DECISION-ENGINE-R751.754).
    CurriculumPromotion(Box<medulla_decision_policy_types::decision::curriculum::CurriculumRequest>),
    /// An action class's reversibility (MEDULLA-DECISION-ENGINE-R752.755).
    AutonomyVerdict(Box<medulla_decision_statistical_types::decision::autonomy::AutonomyRequest>),
    /// A reference and a current score window (MEDULLA-DECISION-ENGINE-R752.754).
    PopulationDrift(Box<medulla_decision_policy_types::decision::population::DriftRequest>),
    /// A replica spread, width and budget (MEDULLA-DECISION-ENGINE-R752.754).
    FanoutWidth(Box<medulla_decision_policy_types::decision::population::FanoutRequest>),
    /// Competing trajectories (MEDULLA-DECISION-ENGINE-R753.754).
    TrajectoryJoin(Box<medulla_decision_statistical_types::decision::trajectory_verdict::JoinRequest>),
    /// A trace and its checks (MEDULLA-DECISION-ENGINE-R753.755).
    StepVerification(Box<medulla_decision_statistical_types::decision::trajectory_verdict::VerifyRequest>),
    /// A candidate's probe results (MEDULLA-DECISION-ENGINE-R753.755).
    AdversarialAdmission(Box<medulla_decision_statistical_types::decision::trajectory_verdict::AdmissionRequest>),
    /// The bounded frontier of a thought search (MEDULLA-DECISION-ENGINE-R750.754).
    ThoughtSearch(Box<medulla_decision_statistical_types::decision::thought_search::ReasoningState>),
    /// Retrieved memory or knowledge entries offered as premises, each with its
    /// trust tier (MEDULLA-DECISION-ENGINE-R132.3). Read from the param named
    /// `retrieved_entries`.
    RetrievedEntries(BoundedVec<medulla_decision_statistical_types::decision::entry_types::RetrievedEntry, 64>),
    /// A skill workflow draft to compile (MEDULLA-DECISION-ENGINE-R156.1).
    SkillWorkflowDraft(Box<medulla_decision_program_types::decision::skill_workflow::SkillWorkflowDraft>),
    /// A skill upgrade compatibility question (MEDULLA-DECISION-ENGINE-R167.1).
    SkillUpgrade(Box<medulla_decision_program_types::decision::skill_upgrade_request::SkillUpgradeRequest>),
    /// What a caller observed about one channel (MEDULLA-DECISION-ENGINE-R170.1).
    ChannelSignal(medulla_decision_program_types::decision::team_coordination::ChannelSignal),
    /// A paired held-out evaluation of a raw and a candidate context
    /// (MEDULLA-DECISION-ENGINE-R137.14).
    ContextReplacement(Box<super::context_replacement::PairedContextEvaluation>),
    /// A time-windowed query with its three-clock context (MEDULLA-DECISION-ENGINE-R146.1).
    TemporalQuery(medulla_decision_program_types::decision::temporal_query::TemporalQueryInput),
    /// A stored distilled scorer to rank with (MEDULLA-DECISION-ENGINE-R147.1).
    DistilledScorer {
        scorer_id: String,
        scorer_revision: u64,
        top_k: u32,
    },
    /// A trace, fix, test and receipts to admit (MEDULLA-DECISION-ENGINE-R161.1).
    CorrectionBinding(Box<super::trace_fix_binding::CorrectionBinding>),
    /// A field-shielding request (MEDULLA-DECISION-ENGINE-R162.1).
    FieldShielding(Box<medulla_decision_statistical_types::decision::field_shielding::FieldShieldingRequest>),
    /// Mixed filesystem and graph intake sources (MEDULLA-DECISION-ENGINE-R163.1).
    IntakeSources(BoundedVec<medulla_decision_policy_types::decision::graph_intake::IntakeSource, 64>),
    /// One round of a checker feedback loop (MEDULLA-DECISION-ENGINE-R169.1).
    Feedback(Box<medulla_decision_program_types::decision::checker_feedback::FeedbackRecord>),
    /// A tool-assignment request (MEDULLA-DECISION-ENGINE-R134.110.1).
    ToolAssignment(Box<super::tool_relevance::ToolAssignmentRequest>),
    /// The complete receipt set of one fan-out (MEDULLA-DECISION-ENGINE-R134.100).
    FanoutResults(Box<medulla_decision_program_types::decision::task_plan::fanout::FanoutAggregateRequest>),
    /// A delegation admission request (MEDULLA-DECISION-ENGINE-R134.102).
    DelegationAdmission(medulla_decision_statistical_types::decision::delegation::DelegationAdmissionRequest),
    /// Pairwise caller claims over answer candidates (MEDULLA-DECISION-ENGINE-R134.106).
    AnswerRanking(Box<medulla_decision_program_types::decision::answer_ranking_request::AnswerRankingRequest>),
    /// The ranked candidates of one unified query to budget a comparative
    /// analysis over (MEDULLA-DECISION-ENGINE-R139.3).
    ComparativeAnalysis(Box<medulla_decision_statistical_types::decision::comparative_analysis::ComparativeAnalysisRequest>),
    /// A capability designation question (MEDULLA-DECISION-ENGINE-R172.7).
    CapabilityDesignation(Box<medulla_decision_policy_types::decision::capability_designation::DesignationAsk>),
    /// A job-scoped load set question (MEDULLA-DECISION-ENGINE-R176.3).
    LoadSet(Box<medulla_decision_policy_types::decision::load_set::LoadSetQuestion>),
    /// A plan to admit against the architecture view (MEDULLA-DECISION-ENGINE-R171.3).
    PlanningAdmission(Box<medulla_decision_program_types::decision::planning_admission::PlanningAdmissionRequest>),
    /// The caller's measurements of the policy's safety bounds, read from the
    /// param `safety.measurements` (MEDULLA-DECISION-ENGINE-R001.100.2).
    SafetyMeasurements(BoundedVec<medulla_decision_statistical_types::decision::safety::SafetyMeasurement, 32>),
    /// One trajectory decision of a task plan (MEDULLA-DECISION-ENGINE-R140.104).
    Trajectory(Box<medulla_decision_program_types::decision::task_plan::trajectory::TrajectoryQuery>),
    /// The stored skill binding a task plan was built from, read from the param
    /// `skill_binding` (MEDULLA-REPO-INGEST-R002.1.3).
    SkillBinding(Box<medulla_decision_policy_types::decision::task_plan::skill_binding::SkillBindingPin>),
    /// The proposed action to check against the policy's limits, read from the
    /// param `guarded_action` (MEDULLA-DECISION-ENGINE-R131.3).
    GuardedAction(Box<medulla_decision_statistical_types::decision::limit_types::GuardedAction>),
    /// The two candidates of one branch-merge conflict record
    /// (MEDULLA-DECISION-ENGINE-R828).
    ResolveConflict(Box<medulla_decision_statistical_types::decision::statistical::declared::ConflictCandidates>),
}

/// One named parameter of a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TypedParam {
    pub name: String,
    pub value: TypedValue,
}

/// One statistical decision request.
/// Not `Eq`, for the same reason [`CandidateSource`] is not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DecideRequest {
    pub tenant_id: String,
    pub question: StatisticalQuestion,
    pub candidates: CandidateSource,
    pub feature_schema: ComponentDependency,
    /// Absent asks for the deterministic ladder only.
    #[serde(default)]
    pub head: Option<ComponentDependency>,
    pub policy: DecisionPolicyRef,
    /// Sorted by name and unique.
    #[serde(default)]
    pub params: BoundedVec<TypedParam, 64>,
    /// At most [`MAX_DECIDE_RECORDS`].
    #[serde(default)]
    pub max_records: Option<u16>,
    /// Times (strictly increasing, not after the decision's clock) at which
    /// to report the head's belief over the same options (EH-297). Omitted
    /// when empty.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub belief_as_of: BoundedVec<u64, 8>,
    /// Evidence streams the decision rests on, referenced and never read
    /// (MEDULLA-DECISION-ENGINE-R019.100.1). Omitted when empty.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub evidence_streams:
        BoundedVec<medulla_decision_policy_types::decision::evidence_stream::EvidenceStreamRef, 16>,
}

/// How the feature matrix the decision read is pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "matrix", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FeatureMatrixRef {
    /// Small matrices travel inline, exactly, on a named fixed-point scale.
    Inline {
        candidate_ids: BoundedVec<String, 64>,
        feature_names: BoundedVec<String, 32>,
        scale: QuantScaleTag,
        values: BoundedVec<i64, 2048>,
    },
    /// Larger ones are pinned by content digest.
    Blob {
        sha256: String,
        length: u64,
        rows: u32,
        columns: u32,
    },
}

/// How the shortlist the decision scored was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ShortlistProvenance {
    /// The approximate-nearest-neighbour recall mode, when one was used.
    #[serde(default)]
    pub ann_recall_mode: Option<String>,
    /// The clock the shortlist's recency filters read, recorded so the
    /// shortlist can be reproduced.
    pub now_ms: u64,
    #[serde(default)]
    pub embedder_model_digest: Option<String>,
}

/// A committed exploration draw: the seed is committed before the draw and
/// revealed after, so the draw cannot be chosen after the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExplorationRecord {
    pub seed_commitment: String,
    #[serde(default)]
    pub revealed_seed: Option<String>,
    pub budget_digest: String,
    /// The context-matched candidate pool the draw was gated by
    /// (MEDULLA-DECISION-ENGINE-R020.1.2); omitted when the record predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<CandidatePool>,
}

/// The visible options split by what earlier decisions in the same context
/// executed: `exploitation` holds the options executed in a matched record,
/// `exploration` every other visible option. A non-empty exploitation pool
/// forces exploration off and restricts ranking to that pool
/// (MEDULLA-DECISION-ENGINE-R020.1.2). Both lists are sorted by option id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CandidatePool {
    pub context_digest: String,
    pub exploitation: Vec<String>,
    pub exploration: Vec<String>,
    /// The matched record ids, ascending.
    pub matched_records: Vec<String>,
}

/// The input set of one statistical decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StatisticalInputs {
    /// Optional exact harness evidence; omission preserves existing record bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_evaluation: Option<harness_config::HarnessConfigEvaluationEvidence>,
    /// The contrastive harness composition decision, present exactly when the
    /// question kind is `HarnessContrast` (MEDULLA-DECISION-ENGINE-R803).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_contrast: Option<super::harness_contrast::HarnessContrastDecision>,
    /// Digest of the question id and the name-sorted typed parameters
    /// (MEDULLA-DECISION-ENGINE-R020.1.1): two decisions share a context
    /// exactly when this text is equal. Omitted when no context was sealed, so
    /// older record bytes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_digest: Option<String>,
    pub feature_schema: ComponentDependency,
    #[serde(default)]
    pub head: Option<ComponentDependency>,
    /// The policy the decision resolved, so a replay reads the same body.
    pub policy: DecisionPolicyRef,
    pub policy_digest: String,
    /// The typed parameters the features and the keyed seed read.
    #[serde(default)]
    pub params: BoundedVec<TypedParam, 64>,
    /// Identity of the rules (names, thresholds, ontology digest) the
    /// candidates' derived classes came from (EH-200); a record replays only
    /// under the same identity. `None` when no class was derived.
    #[serde(default)]
    pub classification_rules: Option<String>,
    pub feature_matrix: FeatureMatrixRef,
    pub shortlist: ShortlistProvenance,
    #[serde(default)]
    pub exploration: Option<ExplorationRecord>,
    /// The belief slices' stored inputs (EH-297); omitted when none.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub belief_slices: BoundedVec<
        medulla_decision_program_types::decision::statistical::belief::BeliefSliceInputs,
        8,
    >,
    /// The evidence streams the request referenced; omitted when none, so
    /// older record bytes and digests are unchanged.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub evidence_streams:
        BoundedVec<medulla_decision_policy_types::decision::evidence_stream::EvidenceStreamRef, 16>,
    /// The graph rows a graph-sourced decision read, with the class each row's
    /// typed label gives it (MEDULLA-DECISION-ENGINE-R121.2); omitted when none.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub graph_rows: BoundedVec<
        medulla_decision_program_types::decision::statistical::graph_rows::GraphRowProvenance,
        64,
    >,
}

/// One statistical decision, recorded. Record version 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StatisticalDecisionRecord {
    pub schema_version: u16,
    pub record_id: String,
    pub tenant_id: String,
    pub caller_principal: String,
    pub created_at_ms: u64,
    pub question: StatisticalQuestion,
    pub candidate_source: CandidateSourceRecord,
    pub inputs: StatisticalInputs,
    pub inputs_digest: String,
    pub resolution_kind: ResolutionKind,
    pub evidence_class: EvidenceClass,
    pub trace_fidelity: TraceFidelity,
    pub premises: BoundedVec<PremiseRef, 1024>,
    pub outcome: StatisticalOutcome,
    /// The one typed answer the question's family concluded, when it has
    /// one (the evaluate-only task DAG of `task_plan`). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<outcome::FamilyAnswer>,
    #[serde(default)]
    pub calibration: Option<CalibrationStatement>,
    /// Per-feature logit contributions of the chosen or top option; exact for
    /// a linear head's logit only.
    #[serde(default)]
    pub explanation: Option<LinearExplanation>,
    /// The audit-sampling draw of an acted decision (EH-026).
    #[serde(default)]
    pub audit: Option<AuditDraw>,
    /// The executed policy's full distribution over the options, aligned with
    /// the feature matrix's candidate order and conditional on an option being
    /// executed; empty when nothing was (advisory or abstained). Off-policy
    /// evaluation over the decision log reads it.
    #[serde(default)]
    pub logging_propensities: BoundedVec<UnitRationalWire, 64>,
    /// The routed template and slot values of a `TemplateChoice` question.
    #[serde(default)]
    pub nl_binding: Option<nl::NlBinding>,
    /// The head's belief at each requested slice (EH-297); omitted when none.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub belief:
        BoundedVec<medulla_decision_program_types::decision::statistical::belief::BeliefPoint, 8>,
    /// Synthetic evidence is labelled as such, always.
    pub synthetic_evidence: bool,
    /// The provider a retrieval-plan decision committed to
    /// (MEDULLA-DECISION-ENGINE-R029.1); omitted when none was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_binding: Option<retrieval_plan::RetrievalProviderBinding>,
    /// The retrieved entries the policy refused, with the tier and the check
    /// (MEDULLA-DECISION-ENGINE-R132.5); omitted when none was refused.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub refused_entries:
        BoundedVec<medulla_decision_statistical_types::decision::entry_types::RefusedEntry, 64>,
    pub record_digest: String,
}

impl StatisticalDecisionRecord {
    /// The explicit task plan the `task_plan` family answered, if any.
    pub fn task_plan(
        &self,
    ) -> Option<&medulla_decision_program_types::decision::task_plan::TaskPlan> {
        self.answer
            .as_ref()
            .and_then(outcome::FamilyAnswer::task_plan)
    }

    /// The comparative-analysis plan the `comparative_analysis_plan` family
    /// answered, if any.
    pub fn comparative_analysis_plan(
        &self,
    ) -> Option<&medulla_decision_statistical_types::decision::comparative_analysis::ComparativeAnalysisPlan>{
        self.answer
            .as_ref()
            .and_then(outcome::FamilyAnswer::comparative_analysis_plan)
    }

    /// The validating constructor; see [`super::DecisionRecord::checked`].
    pub fn checked(self) -> Result<Self, DecisionErrorCode> {
        if self.schema_version != medulla_decision_statistical_types::decision::STATISTICAL_DECISION_RECORD_SCHEMA_VERSION {
            return Err(DecisionErrorCode::DecisionRecordVersionUnsupported);
        }
        Ok(self)
    }
}

/// One evaluate-only answer: the records the decision produced, and the digest
/// of the inputs they share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DecisionBatch {
    pub schema_version: u16,
    pub inputs_digest: String,
    pub records: BoundedVec<StatisticalDecisionRecord, 256>,
}

#[cfg(test)]
mod question_kind_declared_tests {
    use medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind;

    // spec: MEDULLA-DECISION-ENGINE-R128.1
    #[test]
    fn harness_config_kind_serialises_snake_case() {
        let wire = serde_json::to_string(&QuestionKind::HarnessConfig).unwrap();
        assert_eq!(wire, "\"harness_config\"");
        assert_eq!(
            serde_json::from_str::<QuestionKind>(&wire).unwrap(),
            QuestionKind::HarnessConfig
        );
        assert!(serde_json::from_str::<QuestionKind>("\"HarnessConfig\"").is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R032.1
    #[test]
    fn resolve_entity_is_declared() {
        assert_eq!(QuestionKind::ResolveEntity, QuestionKind::ResolveEntity);
        assert_ne!(QuestionKind::ResolveEntity, QuestionKind::Route);
    }

    // spec: MEDULLA-SDLC-R004.3
    #[test]
    fn defect_triage_kind_serialises_snake_case() {
        let wire = serde_json::to_string(&QuestionKind::DefectTriage).unwrap();
        assert_eq!(wire, "\"defect_triage\"");
        assert_eq!(
            serde_json::from_str::<QuestionKind>(&wire).unwrap(),
            QuestionKind::DefectTriage
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R033.1
    #[test]
    fn schema_mapping_is_declared() {
        assert_eq!(QuestionKind::SchemaMapping, QuestionKind::SchemaMapping);
        assert_ne!(QuestionKind::SchemaMapping, QuestionKind::Route);
    }
}

#[cfg(test)]
mod tests {
    use super::outcome::FamilyAnswer;
    use super::*;
    use {
        medulla_decision_program_types::decision::task_plan::plan_explicit_tasks,
        medulla_decision_program_types::decision::task_plan::TaskPlanProposal,
        medulla_decision_program_types::decision::task_plan::TaskPlanTask,
    };

    fn task(id: &str, deps: &[&str]) -> TaskPlanTask {
        TaskPlanTask {
            task_id: id.into(),
            prompt: format!("Analyze {id} with cited evidence"),
            depends_on: BoundedVec::new(deps.iter().map(|s| s.to_string()).collect()).unwrap(),
            call: None,
            aggregation: None,
            assignee: None,
            branch: None,
            join: None,
            decision: None,
            approval: None,
            duration_ms: None,
            priority_weight: None,
            foreach: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R001.101.5
    #[test]
    fn family_body_and_answer_round_trip() {
        let proposal = TaskPlanProposal {
            tasks: BoundedVec::new(vec![task("a", &[]), task("b", &["a"])]).unwrap(),
            output_task_id: "b".into(),
            max_concurrency: 1,
            max_steps: 2,
            final_artifact: None,
        };
        // The request body is one `TypedValue` variant.
        let body = TypedValue::TaskPlan(Box::new(proposal.clone()));
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["type"], "task_plan");
        assert_eq!(serde_json::from_value::<TypedValue>(json).unwrap(), body);
        // The answer is one `FamilyAnswer` variant, tagged the same way.
        let plan = plan_explicit_tasks(&proposal).unwrap();
        let answer = FamilyAnswer::TaskPlan(Box::new(plan.clone()));
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json["type"], "task_plan");
        assert_eq!(
            serde_json::from_value::<FamilyAnswer>(json).unwrap(),
            answer
        );
        assert_eq!(answer.task_plan(), Some(&plan));
        // An unknown variant is refused, not defaulted.
        assert!(serde_json::from_value::<FamilyAnswer>(
            serde_json::json!({"type": "invented", "value": {}})
        )
        .is_err());
    }
}
