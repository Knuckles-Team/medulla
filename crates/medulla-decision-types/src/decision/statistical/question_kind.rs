//! The class of a statistical question.

use serde::{Deserialize, Serialize};

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
    /// Evaluate-only placement of services onto hosts (MEDULLA-PLACEMENT-R700.3):
    /// `Decide` reads the host budgets from the graph, solves the bounded 0-1
    /// program and seals the plan; it acquires nothing and moves nothing. See
    /// `statistical::placement_plan`.
    PlacementPlan,
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
    /// The outputs of one stored DMN decision table over typed inputs (MEDULLA-DECISION-ENGINE-R180). See `decision::process_decision`.
    DecisionTableEval,
    /// The arms one plan branch takes over typed inputs (MEDULLA-DECISION-ENGINE-R180). See `decision::process_decision`.
    BranchSelect,
    /// Whether a checker feedback loop continues (MEDULLA-DECISION-ENGINE-R169). See `decision::checker_feedback`.
    FeedbackLoop,
    /// Whether a plan's features are wired in the architecture (MEDULLA-DECISION-ENGINE-R171). See `decision::planning_admission`.
    PlanningAdmission,
    /// Whether a tool action that follows content of a recorded trust mark is admitted (MEDULLA-DECISION-ENGINE-R038.2). See `decision::tool_action`.
    ToolActionAdmission,
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
    /// Whether an agent may reuse an owner's stored credential binding for one connector,
    /// provider, scope set and purpose: `admit`, `deny:<code>` or an abstention
    /// (MEDULLA-DECISION-ENGINE-R832). Evaluate-only. See `family::credential_reuse`.
    CredentialReuse,
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
    /// Which reasoning paradigm (rules, owl, causal, statistical) a task's capability tags make legal and best (MEDULLA-DECISION-ENGINE-R823). See `statistical::reasoning_paradigm`.
    ReasoningParadigm,
    /// The concurrency limit one model endpoint's recorded observations allow (MEDULLA-DECISION-ENGINE-R825). See `decision::model_capacity`.
    ModelCapacity,
    /// Whether a run-bound process is a zombie, hung or progressing (MEDULLA-DECISION-ENGINE-R820). See `decision::process_remediation`.
    ProcessRemediation,
    /// The task plan of one stored canonical workflow: each step bound from the graph, else ranked (MEDULLA-CQ-ORCH-R704.5). Evaluate-only. See `decision::process_plan`.
    ProcessPlan,
    /// Which goals the remaining ingest budget funds and which items may be purged (MEDULLA-DECISION-ENGINE-R826). See `decision::ingest_shortfall`.
    IngestShortfall,
    /// Whether a proposed capacity change is allowed against the ledger (MEDULLA-DECISION-ENGINE-R824). See `decision::statistical::capacity`.
    CapacityProposal,
    /// Whether a workload's replica-count intent is allowed against the stored scale authority: `allow` or `refuse:<CODE>` (MEDULLA-DECISION-ENGINE-R833). Evaluate-only. See `statistical::scale`.
    ScaleIntent,
    /// Which red-team attacks and transforms to run for a stated goal (MEDULLA-DECISION-ENGINE-R818). See `family::probe_selection`.
    ProbeSelection,
    /// Splits one trajectory reward over its steps and hierarchy
    /// (MEDULLA-DECISION-ENGINE-R751.750). See `decision::reward`.
    RewardDecomposition,
    /// The sources a research run reads next, by expected information value
    /// under a spend budget (MEDULLA-DECISION-ENGINE-R705.3). See `decision::research_voi`.
    ResearchNext,
    /// Which features of studied projects match local capabilities, and the
    /// plans that close the gaps (MEDULLA-DECISION-ENGINE-R707.3). See
    /// `decision::feature_match`.
    FeatureMatch,
    /// Whether a curriculum stage's recent outcomes clear its promotion
    /// threshold (MEDULLA-DECISION-ENGINE-R751.750). See `decision::curriculum`.
    CurriculumPromotion,
    /// Drift between a reference and a current score window
    /// (MEDULLA-DECISION-ENGINE-R752.750). See `decision::population`.
    PopulationDrift,
    /// How many replicas to fan out from their diversity
    /// (MEDULLA-DECISION-ENGINE-R752.750). See `decision::population`.
    FanoutWidth,
    /// Whether an action class may run without confirmation
    /// (MEDULLA-DECISION-ENGINE-R752.750). See `decision::autonomy`.
    AutonomyVerdict,
    /// Joins competing trajectories into the strongest one
    /// (MEDULLA-DECISION-ENGINE-R753.750). See `decision::trajectory_verdict`.
    TrajectoryJoin,
    /// Verifies a trace step by step and picks a recovery
    /// (MEDULLA-DECISION-ENGINE-R753.750). See `decision::trajectory_verdict`.
    StepVerification,
    /// Admits a candidate against a bounded set of red-team probes
    /// (MEDULLA-DECISION-ENGINE-R753.750). See `decision::trajectory_verdict`.
    AdversarialAdmission,
    /// Skill gaps, neologisms and merge candidates from run traces and the
    /// visible skill library (MEDULLA-DECISION-ENGINE-R703.3). Advisory only:
    /// publishing a change uses the guarded promotion. See
    /// `decision::skill_evolution`.
    SkillEvolution,
    /// Which playbook steps an inbound event burst calls for, proposed and
    /// never run (MEDULLA-DECISION-ENGINE-R704.3). Advisory only. See
    /// `decision::playbook`.
    PlaybookTriage,
    /// Width and depth rung choice for one task (MEDULLA-DECISION-ENGINE-R134.12).
    /// Evaluate-only. See `decision::trajectory_ladder`.
    TrajectoryLadder,
    /// Cited synthesis of analyst findings, abstaining on any gap in the
    /// accounting, a conflict or a bad citation (MEDULLA-DECISION-ENGINE-R134.13).
    /// Evaluate-only. See `decision::analyst_synthesis`.
    AnalystSynthesis,
    /// The employees best covering one role, and the top legal market offer
    /// to allocate, by the deterministic market selection; a twin replays
    /// against its pinned revisions (MEDULLA-ORCH-VOCAB-R750.751).
    /// Evaluate-only. See `decision::recruit`.
    Recruit,
    /// The winner of one branch-merge conflict, by `reliability x confidence`
    /// over a knob margin, or an abstention (MEDULLA-DECISION-ENGINE-R828).
    /// Evaluate-only: `BranchMerge` writes the resolution. See
    /// `decision::resolve_conflict`.
    ResolveConflict,
    /// Council positions coordinated into one evidence-bound decision: the
    /// option a quorum selects, or an abstention (MEDULLA-DECISION-ENGINE-R106.1).
    /// Evaluate-only. See `decision::council`.
    CouncilCoordination,
    /// Whether a delivery's proposal may be armed for the owner's merge, disarmed or held
    /// (MEDULLA-SDLC-R019.10). Evaluate-only: the engine never merges. See
    /// `decision::merge_arming`.
    MergeArming,
    /// How to remove a culprit changeset the bisect named: `revert` or `fix_forward`, ranked over
    /// the legal set (MEDULLA-DECISION-ENGINE-R827). Evaluate-only. See
    /// `decision::revert_or_fix_forward`.
    RevertOrFixForward,
    /// The triage of one defect: its severity, suspected symbols, owner and duplicate link
    /// (MEDULLA-SDLC-R004.3). Evaluate-only: `TriageDefect` commits the record and writes the
    /// `affects` edges. See `decision::defect_triage`.
    DefectTriage,
    /// Whether a challenger revision wins its subject's champion pointer, or
    /// the champion regressed and rolls back (MEDULLA-DECISION-ENGINE-R805.3).
    /// Advisory: the pointer moves only through `DecisionLog`. See
    /// `decision::held_out`.
    Promotion,
}
