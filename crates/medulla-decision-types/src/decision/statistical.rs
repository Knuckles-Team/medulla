//! `Method::Decide`: a statistical decision over library or graph candidates,
//! and the record version 2 it produces.
//!
//! This surface is EVALUATE-ONLY in 2.27.x. It reads candidates, scores them
//! under a pinned feature schema and head, and returns a batch of records; it
//! commits nothing. Whether a record is durable, and where, is a separate
//! decision with its own method.

pub mod belief;
pub mod body;
pub mod dataset;
pub mod declared;
pub mod distilled;
pub mod enrichment_schedule;
pub mod errors;
pub mod execution_routing;
pub mod features;
pub mod grounded_eval;
pub mod harness_config;
pub mod head;
pub mod ingestion_lane;
pub mod keyed;
pub mod log;
pub mod nl;
pub mod outcome;
pub mod placement;
pub mod retrieval;
pub mod retrieval_adapter;
pub mod retrieval_drift;
pub mod retrieval_generation;
pub mod retrieval_plan;
pub mod retrieval_pointer;
pub mod retrieval_refresh;
pub mod run_credit;
pub mod scorer;
pub mod tool_subset;
pub mod topology;
pub mod writeback;

use serde::{Deserialize, Serialize};

use super::errors::DecisionErrorCode;
use super::numeric::{QuantScaleTag, UnitRationalWire};
use super::record::{
    CandidateSourceRecord, EvidenceClass, PremiseRef, ResolutionKind, TraceFidelity,
};
use super::request::{DecisionPolicyRef, LibraryCandidateScope};
use medulla_agent_types::agent_component::ComponentDependency;
use medulla_types::contract::BoundedVec;

pub use errors::StatisticalErrorCode;
pub use outcome::{
    AuditDraw, CalibrationMethod, CalibrationStatement, LinearExplanation, RiskMethod,
    RiskStatement, ScoredOption, StatisticalOutcome,
};

/// The largest record batch one `Decide` may return.
pub const MAX_DECIDE_RECORDS: u16 = 256;

/// What class of question this is. The kind selects the executor; it never
/// selects the guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum QuestionKind {
    Route,
    Rank,
    Classify,
    ResolveEntity,
    SchemaMapping,
    TemplateChoice,
    RetrievalPlan,
    IngestionLane,
    EnrichmentSchedule,
    /// An advisory risk score for a candidate tool invocation, read before
    /// the call (MEDULLA-DECISION-ENGINE-R038). `Decide` is evaluate-only for
    /// every kind (see this module's own doc), so the score it returns can
    /// only ever inform a caller's own authorization step -- it carries no
    /// mechanism to itself gate or block the invocation.
    PreToolRisk,
    /// Inbound connector-event classification, evaluate-only
    /// (MEDULLA-DECISION-ENGINE-R041): `Decide` returns a proposal and its
    /// evidence class and takes no action on the connector's behalf -- the
    /// same evaluate-only guarantee this module's own doc states for every
    /// kind, named here for the connector-triage use case specifically.
    ConnectorEventTriage,
    /// Which internal tool a connector should use for a task, advisory only
    /// (MEDULLA-DECISION-ENGINE-R042): `Decide` returns a proposal with evidence;
    /// the connector's own deterministic authorization path, never `Decide`,
    /// governs whether a resulting write-back actually occurs.
    ConnectorToolChoice,
    /// A write-back proposal for a connector action, with supporting
    /// evidence (MEDULLA-DECISION-ENGINE-R043): the proposal never authorizes the
    /// write itself. Any exploration or statistical head behind it stays
    /// outside the connector's own deterministic authorization step, which
    /// alone approves the write.
    ConnectorWriteBack,
    /// The smallest tool subset covering every required capability within a
    /// context-token budget (MEDULLA-DECISION-ENGINE-R045). Candidates are
    /// declared (`CandidateSource::Declared`); required capabilities and the
    /// budget are typed request params (`required_capabilities`,
    /// `context_budget_tokens`). See `stat_tool_subset`.
    ToolSubset,
    /// Evaluate-only harness configuration advice (MEDULLA-DECISION-ENGINE-R128).
    /// Returns proposed context, scaffold and action-space choices; never runs them.
    HarnessConfig,
    /// Bounded explicit dependency planning; admission belongs to the host.
    TaskPlan,
    /// The contrastive harness composition decision (MEDULLA-DECISION-ENGINE-R803):
    /// two to eight candidate component versions probed against the same
    /// baseline under identical conditions; the engine adopts a candidate only
    /// when its measured `eval_score_delta` is positive and unbeaten, with
    /// cost as the tie-break, and abstains keeping the current harness when no
    /// candidate improves. See `decision::harness_contrast`.
    HarnessContrast,
    /// Evaluate-only deployment placement over the fixed target registry
    /// (MEDULLA-PLACEMENT-R001.2): `Decide` names the cheapest feasible target and
    /// why every other target was not chosen; it acquires no capacity and
    /// deploys nothing. See `statistical::placement`.
    Placement,
    /// Which context sources fit a token budget, most relevant first
    /// (MEDULLA-DECISION-ENGINE-R029.2.2). Evaluate-only. See
    /// `decision::context_source`.
    ContextBudget,
    /// Whether a prompt passes its window share and the bounded recursive
    /// split that makes it fit (MEDULLA-DECISION-ENGINE-R137). Evaluate-only. See
    /// `decision::context_budget`.
    OverWindowPlan,
    /// Which versions of the named instruction sets a run discloses now: the
    /// initial ones before execution, a runtime refinement only once its
    /// trigger has occurred (MEDULLA-DECISION-ENGINE-R137.100). Evaluate-only.
    /// See `decision::instruction_disclosure`.
    InstructionDisclosure,
    /// Whether a changeset review's findings block the change: findings in the
    /// five flaw classes, each bound to the lines it cites, with the low-impact
    /// doubted ones filtered (MEDULLA-DECISION-ENGINE-R138.101.2). Evaluate-only.
    /// See `decision::changeset_review`.
    ChangesetReview,
    /// Which of the three clocks (event, narrative, storage) a time-windowed
    /// query should retry on, and the widened window to retry with
    /// (MEDULLA-DECISION-ENGINE-R146.3). Evaluate-only: the caller runs the
    /// fallback. See `decision::temporal_query`.
    TemporalBasis,
    /// Whether an evolution round's observed flips verify or falsify its
    /// declaration (MEDULLA-DECISION-ENGINE-R136). See `decision::evolution_round`.
    RoundVerdict,
    /// Which model each evolution stage runs on (MEDULLA-DECISION-ENGINE-R136).
    StageAllocation,
    /// The verdict of one red-team probe (MEDULLA-DECISION-ENGINE-R138). See
    /// `decision::red_team`.
    ProbeVerdict,
    /// Catalog items for a query and a history (MEDULLA-DECISION-ENGINE-R175). See
    /// `decision::recommend`.
    ItemRecommendation,
    /// The tools, skills and agents one job needs (MEDULLA-DECISION-ENGINE-R176).
    /// See `decision::load_set`.
    LoadSet,
    /// How and when to use one connector for one job (MEDULLA-DECISION-ENGINE-R177).
    /// See `decision::usage_guidance`.
    UsageGuidance,
    /// Branch outcome aggregation for planning decisions (MEDULLA-DECISION-ENGINE-R164).
    BranchOutcome,
    /// Edit concentration checks over a declared rolling round window (MEDULLA-DECISION-ENGINE-R173).
    EditConcentration,
    /// Bounded multi-step reasoning search (MEDULLA-DECISION-ENGINE-R750.750):
    /// which frontier leaves to expand next, or a stop with the best-so-far node.
    ThoughtSearch,
    /// A budgeted comparative-analysis plan (MEDULLA-DECISION-ENGINE-R139.1).
    /// See `decision::comparative_analysis`.
    ComparativeAnalysisPlan,
    /// A context package under token, money and latency ceilings
    /// (MEDULLA-DECISION-ENGINE-R137.7). See `decision::context_package`.
    ContextPackage,
    /// The swarm topology of one published graph template, asked on its own
    /// (MEDULLA-DECISION-ENGINE-R111.1). Evaluate-only: it takes no capacity lease.
    /// See `statistical::topology`.
    Topology,
    /// The stored compiled program for one task signature
    /// (MEDULLA-DECISION-ENGINE-R135). Evaluate-only: a lookup writes nothing.
    /// See `decision::program_store`.
    ProgramLookup,
    /// Compile a skill document's typed draft into a stored program
    /// (MEDULLA-DECISION-ENGINE-R156). See `decision::skill_workflow`.
    SkillWorkflowCompile,
    /// Whether a new consumer contract still serves the previous one
    /// (MEDULLA-DECISION-ENGINE-R167). See `decision::skill_upgrade_request`.
    SkillUpgrade,
    /// Which team member or channel takes a task when a channel is unavailable
    /// (MEDULLA-DECISION-ENGINE-R170). See `decision::team_coordination`.
    TeamCoordination,
    /// The agents, tools or skills best able to do a job
    /// (MEDULLA-DECISION-ENGINE-R172). See `decision::capability_designation`.
    CapabilityDesignation,
    /// Whether a candidate context may replace the raw one after a paired
    /// held-out comparison (MEDULLA-DECISION-ENGINE-R137.14). See
    /// `decision::context_replacement`.
    ContextReplacement,
    /// Whether a run works on its tool output directly or recursively, from
    /// measured output size and graph size (MEDULLA-DECISION-ENGINE-R137.13).
    /// See `decision::context_budget` in the outcome types.
    RecursionMode,
    /// Whether a sub-call may be dispatched, from its stored signature
    /// (MEDULLA-DECISION-ENGINE-R137.15). See `decision::sub_call`.
    SubCallAdmission,
    /// Command mode and settings choice (MEDULLA-DECISION-ENGINE-R148). See `decision::command_config`.
    CommandConfig,
    /// Model tier and width adjustment from a run error window (MEDULLA-DECISION-ENGINE-R158). See `decision::run_adjustment`.
    RunAdjustment,
    /// Which routing path to prefer, from per-path success rates (MEDULLA-DECISION-ENGINE-R159). See `decision::path_success`.
    PathPreference,
    /// Whether a correction is admitted, from its trace, fix and test receipts (MEDULLA-DECISION-ENGINE-R161). See `decision::trace_fix_binding`.
    CorrectionAdmission,
    /// Which fields a step may read (MEDULLA-DECISION-ENGINE-R162). See `decision::field_shielding`.
    FieldShielding,
    /// Resolution of mixed filesystem and graph intake sources (MEDULLA-DECISION-ENGINE-R163). See `decision::graph_intake`.
    GraphIntake,
    /// Measured tool necessity against a trace (MEDULLA-DECISION-ENGINE-R165). See `decision::tool_necessity`.
    ToolNecessity,
    /// Whether a checker feedback loop continues (MEDULLA-DECISION-ENGINE-R169). See `decision::checker_feedback`.
    FeedbackLoop,
    /// Whether a plan's features are wired in the architecture (MEDULLA-DECISION-ENGINE-R171). See `decision::planning_admission`.
    PlanningAdmission,
    /// The tenant's agent graph an inbound A2A task routes to (MEDULLA-DECISION-ENGINE-R044.2.2). See `decision::a2a_route`.
    A2aRoute,
    /// Staged papers admitted by calibrated relevance to platform goals and graph content (MEDULLA-DECISION-ENGINE-R031.100). See `decision::paper_admission`.
    PaperAdmission,
    /// Whether a held run program or run output proposal is admitted (MEDULLA-DECISION-ENGINE-R038.3). See `decision::held_item`.
    HeldItemAdmission,
    /// Whether a revision keeps every criterion the prior grading satisfied
    /// (MEDULLA-DECISION-ENGINE-R136.101). See `decision::evolution_round`.
    CriterionReview,
    /// Whether generated code is admitted, from its adversarial verifier receipt
    /// (MEDULLA-DECISION-ENGINE-R138.100). See `decision::verifier_receipt`.
    GeneratedCodeAdmission,
    /// The runtime quota of one run: knob defaults, task-class overrides and a
    /// request that only tightens, under fixed ceilings (MEDULLA-DECISION-ENGINE-R133).
    /// Evaluate-only. See `decision::run_governance`.
    RuntimeQuota,
    /// Which run-derived insights apply to one context signature
    /// (MEDULLA-DECISION-ENGINE-R135.10). Evaluate-only: a selection writes
    /// nothing. See `decision::program_store::insight`.
    InsightSelection,
    /// Which tools a role should see for one task, ranked by lexical and
    /// embedding relevance (MEDULLA-DECISION-ENGINE-R134.110). Evaluate-only.
    /// See `decision::tool_relevance`.
    ToolRelevance,
    /// Aggregate a fan-out's complete receipt set with the plan's already-selected
    /// operator (MEDULLA-DECISION-ENGINE-R134.100). Evaluate-only. See
    /// `decision::task_plan::fanout`.
    FanoutAggregate,
    /// Admit delegation over a stable outcome window (MEDULLA-DECISION-ENGINE-R134.102).
    /// Evaluate-only. See `decision::delegation`.
    DelegationAdmission,
    /// Rank answer candidates from pairwise caller claims, with no verifier
    /// (MEDULLA-DECISION-ENGINE-R134.106). Evaluate-only. See
    /// `decision::answer_ranking_request`.
    AnswerRanking,
    /// Whether a serving technique is activated for a model, from benchmark receipts (MEDULLA-DECISION-ENGINE-R819). See `decision::serving_technique`.
    ServingTechniqueActivation,
    /// Whether a run-bound process is a zombie, hung or progressing (MEDULLA-DECISION-ENGINE-R820). See `decision::process_remediation`.
    ProcessRemediation,
    /// Which goals the remaining ingest budget funds and which items may be purged (MEDULLA-DECISION-ENGINE-R826). See `decision::ingest_shortfall`.
    IngestShortfall,
}

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
        plan: Box<medulla_types::wire::Plan>,
    },
    /// The caller's own options, each fact a claim the caller makes
    /// ([`declared`]). Visible to the declaring principal only.
    Declared {
        options: BoundedVec<declared::DeclaredOption, 64>,
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
    TaskPlan(Box<super::task_plan::TaskPlanProposal>),
    /// Subtasks and specialist scores the harness produced; medulla derives
    /// the plan (MEDULLA-DECISION-ENGINE-R134.3).
    TaskDecomposition(Box<super::task_plan::DecomposeFacts>),
    /// Atomic subtasks to cover with library skills (MEDULLA-DECISION-ENGINE-R134.4).
    SkillPlanQuery(Box<super::task_plan::skill_plan::SkillPlanQuery>),
    /// A context-package request (MEDULLA-DECISION-ENGINE-R137.7).
    ContextPackages(Box<super::context_package::ContextPackageRequest>),
    /// A standalone topology question (MEDULLA-DECISION-ENGINE-R111.1).
    Topology(Box<topology::TopologyQuestion>),
    /// A placement question (MEDULLA-PLACEMENT-R001.7).
    Placement(Box<placement::PlacementQuestion>),
    /// The bounded frontier of a thought search (MEDULLA-DECISION-ENGINE-R750.754).
    ThoughtSearch(Box<super::thought_search::ReasoningState>),
    /// A skill workflow draft to compile (MEDULLA-DECISION-ENGINE-R156.1).
    SkillWorkflowDraft(Box<super::skill_workflow::SkillWorkflowDraft>),
    /// A skill upgrade compatibility question (MEDULLA-DECISION-ENGINE-R167.1).
    SkillUpgrade(Box<super::skill_upgrade_request::SkillUpgradeRequest>),
    /// What a caller observed about one channel (MEDULLA-DECISION-ENGINE-R170.1).
    ChannelSignal(super::team_coordination::ChannelSignal),
    /// A paired held-out evaluation of a raw and a candidate context
    /// (MEDULLA-DECISION-ENGINE-R137.14).
    ContextReplacement(Box<super::context_replacement::PairedContextEvaluation>),
    /// A time-windowed query with its three-clock context (MEDULLA-DECISION-ENGINE-R146.1).
    TemporalQuery(super::temporal_query::TemporalQueryInput),
    /// A stored distilled scorer to rank with (MEDULLA-DECISION-ENGINE-R147.1).
    DistilledScorer {
        scorer_id: String,
        scorer_revision: u64,
        top_k: u32,
    },
    /// A trace, fix, test and receipts to admit (MEDULLA-DECISION-ENGINE-R161.1).
    CorrectionBinding(Box<super::trace_fix_binding::CorrectionBinding>),
    /// A field-shielding request (MEDULLA-DECISION-ENGINE-R162.1).
    FieldShielding(Box<super::field_shielding::FieldShieldingRequest>),
    /// Mixed filesystem and graph intake sources (MEDULLA-DECISION-ENGINE-R163.1).
    IntakeSources(BoundedVec<super::graph_intake::IntakeSource, 64>),
    /// One round of a checker feedback loop (MEDULLA-DECISION-ENGINE-R169.1).
    Feedback(Box<super::checker_feedback::FeedbackRecord>),
    /// A tool-assignment request (MEDULLA-DECISION-ENGINE-R134.110.1).
    ToolAssignment(Box<super::tool_relevance::ToolAssignmentRequest>),
    /// The complete receipt set of one fan-out (MEDULLA-DECISION-ENGINE-R134.100).
    FanoutResults(Box<super::task_plan::fanout::FanoutAggregateRequest>),
    /// A delegation admission request (MEDULLA-DECISION-ENGINE-R134.102).
    DelegationAdmission(super::delegation::DelegationAdmissionRequest),
    /// Pairwise caller claims over answer candidates (MEDULLA-DECISION-ENGINE-R134.106).
    AnswerRanking(Box<super::answer_ranking_request::AnswerRankingRequest>),
    /// The ranked candidates of one unified query to budget a comparative
    /// analysis over (MEDULLA-DECISION-ENGINE-R139.3).
    ComparativeAnalysis(Box<super::comparative_analysis::ComparativeAnalysisRequest>),
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
    pub belief_slices: BoundedVec<belief::BeliefSliceInputs, 8>,
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
    pub belief: BoundedVec<belief::BeliefPoint, 8>,
    /// Synthetic evidence is labelled as such, always.
    pub synthetic_evidence: bool,
    pub record_digest: String,
}

impl StatisticalDecisionRecord {
    /// The explicit task plan the `task_plan` family answered, if any.
    pub fn task_plan(&self) -> Option<&super::task_plan::TaskPlan> {
        self.answer
            .as_ref()
            .and_then(outcome::FamilyAnswer::task_plan)
    }

    /// The comparative-analysis plan the `comparative_analysis_plan` family
    /// answered, if any.
    pub fn comparative_analysis_plan(
        &self,
    ) -> Option<&super::comparative_analysis::ComparativeAnalysisPlan> {
        self.answer
            .as_ref()
            .and_then(outcome::FamilyAnswer::comparative_analysis_plan)
    }

    /// The validating constructor; see [`super::DecisionRecord::checked`].
    pub fn checked(self) -> Result<Self, DecisionErrorCode> {
        if self.schema_version != super::STATISTICAL_DECISION_RECORD_SCHEMA_VERSION {
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
    use super::QuestionKind;

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
    use crate::decision::task_plan::{plan_explicit_tasks, TaskPlanProposal, TaskPlanTask};

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
