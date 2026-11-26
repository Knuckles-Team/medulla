//! Decision budgets: one Criterion group `decide/<family>` per registered
//! question family (MEDULLA-DECISION-ENGINE-R178.1).
//!
//! Every group builds its `Decide` request through the served-test kit
//! fixtures (`medulla_h_decide::decide::stat_test_support`) and drives it
//! through the verified dispatch boundary, so a group times the served path
//! (admission, the family hooks, the executor and the sealed record), and a
//! family that refuses the fixture request times its refusal path.
//!
//! Besides the Criterion report, the run writes the budget report that
//! `cargo xtask decide-budgets` compares to `benches/baselines/decide.json`
//! (R178.2) when `DECIDE_BUDGETS_REPORT` names a path: per group `p50_ms`,
//! `p99_ms` and `peak_mib` (resident high-water growth during the group).
//!
//! Run: `DECIDE_BUDGETS_REPORT=target/decide-budgets.json cargo bench --features full --bench decide_budgets`
//! The nightly workflow `.github/workflows/nightly-decide-budgets.yml` runs it (R178.3).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// The served dispatch kit of the other served benches: the agent-loop group needs a durable
// graph behind the real dispatch entry (MEDULLA-DURABLE-KERNEL-R871).
#[allow(dead_code)]
#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;
// The program fixtures of the served program job tests: the `decide/program` group compiles
// the same 200-example corpus (MEDULLA-DECISION-ENGINE-R810.6).
#[allow(dead_code, unused_imports)]
#[path = "../tests/common/program_job_support.rs"]
mod program_job_support;

use criterion::{criterion_group, Criterion};
use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_agent_types::agent_component::{
    MeasuredFact, ObservationRef, ShellProficiency, StoppingBehaviour,
};
use medulla_agent_types::agent_topology::{
    LeasePlan, SlotPlan, StopRule, TopologyPlan, TopologyPolicy,
};
use medulla_compute::assemble::admit_topology;
use medulla_decision_outcome_types::decision::concentration::{
    concentration_verdict, ConcentrationPolicy, EditFact, EditStatus,
};
use medulla_decision_stats::decision::population::wasserstein1;
use medulla_decision_stats::decision::reward::decompose;
use medulla_decision_stats::decision::thought_search::choose_expansion;
use medulla_decision_stats::decision::trajectory_join::join;
use medulla_decision_types::belief_revision::BeliefReviseRequest;
use medulla_decision_types::claims::{ClaimOp, EvidenceChannel};
use medulla_decision_types::decision::autonomy::{AutonomyRequest, IrreversibilityClass};
use medulla_decision_types::decision::curriculum::{
    CurriculumRequest, CurriculumStage, PromotionPolicy,
};
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::population::{DriftRequest, DriftWindow};
use medulla_decision_types::decision::program_store::{
    compile_program, program_key, ProgramKnobs, MAX_PROGRAM_STEPS,
};
use medulla_decision_types::decision::request::LibraryCandidateScope;
use medulla_decision_types::decision::reward::{
    RewardDecompositionRequest, RewardKnobs, StepSignal, MAX_REWARD_STEPS,
};
use medulla_decision_types::decision::run_governance::{
    RunOutcome, RunTraceRecord, StepKind, StepOutcome, TraceArgument, TraceScalar, TraceStep,
};
use medulla_decision_types::decision::skill_evolution::{PARAM_REQUESTED_SKILLS, PARAM_USED_TERMS};
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::statistical::distilled::{DistilledUpload, TeacherJudgment};
use medulla_decision_types::decision::statistical::features::{
    FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::harness_config::{
    HarnessConfigEvaluationEvidence, HarnessEvidenceLimits,
};
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::retrieval::LearningWrite;
use medulla_decision_types::decision::statistical::writeback::{
    ProposalInput, QueuedProposalInput, RiskTier, WritebackOp,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, DecisionBatch, QuestionKind, QuestionSafety,
    StatisticalDecisionRecord, StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::task_plan::{TaskPlanProposal, TaskPlanTask};
use medulla_decision_types::decision::thought_search::{
    BudgetLedger, ReasoningState, ReasoningStrategy, ThoughtNode, ThoughtSearchKnobs,
    MAX_THOUGHT_LEAVES,
};
use medulla_decision_types::decision::trajectory_verdict::{
    JoinRequest, StepCheck, TraceStep as VerifyStep, TrajectoryEntry, VerifyRequest,
    MAX_TRAJECTORIES,
};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_decision_types::decision::{DecisionPolicyRef, EvidenceClass};
use medulla_h_decide::decide::stat_test_support::{
    declared_request, decode, publish_route_schema, request, Harness, TENANT,
};
use medulla_mutation_types::capacity_lease::LeasePriority;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use std::hint::black_box;
use tokio::runtime::{Builder, Runtime};

/// Every registered family timed with the generic fixture request. The
/// reward-decomposition, population-drift, curriculum-promotion and autonomy-verdict families have their own served
/// groups (`bench_served_kernels`), because the generic request is not their body.
/// `family_name` matches exhaustively, so a new
/// `QuestionKind` fails to compile until it gains a group name there; add it
/// here in the same change.
const FAMILIES: &[QuestionKind] = &[
    QuestionKind::Route,
    QuestionKind::Rank,
    QuestionKind::Classify,
    QuestionKind::ResolveEntity,
    QuestionKind::SchemaMapping,
    QuestionKind::TemplateChoice,
    QuestionKind::RetrievalPlan,
    QuestionKind::IngestionLane,
    QuestionKind::EnrichmentSchedule,
    QuestionKind::PreToolRisk,
    QuestionKind::ConnectorEventTriage,
    QuestionKind::ConnectorToolChoice,
    QuestionKind::ConnectorWriteBack,
    QuestionKind::ToolSubset,
    QuestionKind::HarnessConfig,
    QuestionKind::TaskPlan,
    QuestionKind::HarnessContrast,
    QuestionKind::Placement,
    QuestionKind::ContextBudget,
    QuestionKind::OverWindowPlan,
    QuestionKind::InstructionDisclosure,
    QuestionKind::ChangesetReview,
    QuestionKind::TemporalBasis,
    QuestionKind::RoundVerdict,
    QuestionKind::StageAllocation,
    QuestionKind::ProbeVerdict,
    QuestionKind::ItemRecommendation,
    QuestionKind::LoadSet,
    QuestionKind::UsageGuidance,
    QuestionKind::BranchOutcome,
    QuestionKind::EditConcentration,
    QuestionKind::ThoughtSearch,
    QuestionKind::ComparativeAnalysisPlan,
    QuestionKind::ContextPackage,
    QuestionKind::ContextReplacement,
    QuestionKind::RecursionMode,
    QuestionKind::SubCallAdmission,
    QuestionKind::CommandConfig,
    QuestionKind::RunAdjustment,
    QuestionKind::PathPreference,
    QuestionKind::CorrectionAdmission,
    QuestionKind::FieldShielding,
    QuestionKind::GraphIntake,
    QuestionKind::ToolNecessity,
    QuestionKind::FeedbackLoop,
    QuestionKind::PlanningAdmission,
    QuestionKind::A2aRoute,
    QuestionKind::PaperAdmission,
    QuestionKind::HeldItemAdmission,
    QuestionKind::CriterionReview,
    QuestionKind::GeneratedCodeAdmission,
    QuestionKind::RuntimeQuota,
    QuestionKind::FanoutAggregate,
    QuestionKind::DelegationAdmission,
    QuestionKind::AnswerRanking,
    QuestionKind::ServingTechniqueActivation,
    QuestionKind::ReasoningParadigm,
    QuestionKind::ModelCapacity,
    QuestionKind::ProcessRemediation,
    QuestionKind::IngestShortfall,
    QuestionKind::CapacityProposal,
    QuestionKind::ProbeSelection,
    QuestionKind::InsightSelection,
    QuestionKind::SkillUpgrade,
    QuestionKind::SkillWorkflowCompile,
    QuestionKind::TeamCoordination,
    QuestionKind::FanoutWidth,
    QuestionKind::AdversarialAdmission,
    QuestionKind::ResearchNext,
    QuestionKind::FeatureMatch,
    QuestionKind::PlaybookTriage,
    QuestionKind::ToolActionAdmission,
];

/// The families with their own bench fixture instead of the generic one:
/// `bench_topology_admit`, `bench_tool_relevance_cap`,
/// `bench_capability_designation` and `bench_skill_evolution` (`ProgramLookup`
/// is listed by `bench_program_lookup`).
const OWN_FIXTURE_FAMILIES: &[QuestionKind] = &[
    QuestionKind::Topology,
    QuestionKind::ToolRelevance,
    QuestionKind::CapabilityDesignation,
    QuestionKind::SkillEvolution,
];

/// The samples the budget report takes per group.
const REPORT_SAMPLES: usize = 200;

/// The group name of one family: its wire (snake_case) name.
fn family_name(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Route => "route",
        QuestionKind::Rank => "rank",
        QuestionKind::Classify => "classify",
        QuestionKind::ResolveEntity => "resolve_entity",
        QuestionKind::SchemaMapping => "schema_mapping",
        QuestionKind::TemplateChoice => "template_choice",
        QuestionKind::RetrievalPlan => "retrieval_plan",
        QuestionKind::IngestionLane => "ingestion_lane",
        QuestionKind::EnrichmentSchedule => "enrichment_schedule",
        QuestionKind::PreToolRisk => "pre_tool_risk",
        QuestionKind::ConnectorEventTriage => "connector_event_triage",
        QuestionKind::ConnectorToolChoice => "connector_tool_choice",
        QuestionKind::ConnectorWriteBack => "connector_write_back",
        QuestionKind::ToolSubset => "tool_subset",
        QuestionKind::HarnessConfig => "harness_config",
        QuestionKind::TaskPlan => "task_plan",
        QuestionKind::HarnessContrast => "harness_contrast",
        QuestionKind::Placement => "placement",
        QuestionKind::PlacementPlan => "placement_plan",
        QuestionKind::ContextBudget => "context_budget",
        QuestionKind::OverWindowPlan => "over_window_plan",
        QuestionKind::InstructionDisclosure => "instruction_disclosure",
        QuestionKind::ChangesetReview => "changeset_review",
        QuestionKind::TemporalBasis => "temporal_basis",
        QuestionKind::RoundVerdict => "round_verdict",
        QuestionKind::StageAllocation => "stage_allocation",
        QuestionKind::ProbeVerdict => "probe_verdict",
        QuestionKind::ItemRecommendation => "item_recommendation",
        QuestionKind::LoadSet => "load_set",
        QuestionKind::UsageGuidance => "usage_guidance",
        QuestionKind::BranchOutcome => "branch_outcome",
        QuestionKind::EditConcentration => "edit_concentration",
        QuestionKind::ThoughtSearch => "thought_search",
        QuestionKind::ComparativeAnalysisPlan => "comparative_analysis_plan",
        QuestionKind::ContextPackage => "context_package",
        QuestionKind::ContextReplacement => "context_replacement",
        QuestionKind::RecursionMode => "recursion_mode",
        QuestionKind::SubCallAdmission => "sub_call_admission",
        QuestionKind::RewardDecomposition => "reward_decomposition",
        QuestionKind::CurriculumPromotion => "curriculum_promotion",
        QuestionKind::PopulationDrift => "population_drift",
        QuestionKind::FanoutWidth => "fanout_width",
        QuestionKind::AutonomyVerdict => "autonomy_verdict",
        QuestionKind::TrajectoryJoin => "trajectory_join",
        QuestionKind::StepVerification => "step_verification",
        QuestionKind::AdversarialAdmission => "adversarial_admission",
        QuestionKind::ResearchNext => "research_next",
        QuestionKind::FeatureMatch => "feature_match",
        // Needs a published template and a typed question, so it has its own
        // bench fixture, not the generic one `FAMILIES` is driven with.
        QuestionKind::Topology => "topology",
        // Needs a stored program, so it has its own bench fixture.
        QuestionKind::ProgramLookup => "program_lookup",
        QuestionKind::SkillWorkflowCompile => "skill_workflow_compile",
        QuestionKind::SkillUpgrade => "skill_upgrade",
        QuestionKind::TeamCoordination => "team_coordination",
        QuestionKind::CapabilityDesignation => "capability_designation",
        QuestionKind::CommandConfig => "command_config",
        QuestionKind::RunAdjustment => "run_adjustment",
        QuestionKind::PathPreference => "path_preference",
        QuestionKind::CorrectionAdmission => "correction_admission",
        QuestionKind::FieldShielding => "field_shielding",
        QuestionKind::GraphIntake => "graph_intake",
        QuestionKind::ToolNecessity => "tool_necessity",
        QuestionKind::DecisionTableEval => "decision_table_eval",
        QuestionKind::BranchSelect => "branch_select",
        QuestionKind::FeedbackLoop => "feedback_loop",
        QuestionKind::PlanningAdmission => "planning_admission",
        QuestionKind::ToolActionAdmission => "tool_action_admission",
        QuestionKind::A2aRoute => "a2a_route",
        QuestionKind::PaperAdmission => "paper_admission",
        QuestionKind::HeldItemAdmission => "held_item_admission",
        QuestionKind::CriterionReview => "criterion_review",
        QuestionKind::GeneratedCodeAdmission => "generated_code_admission",
        QuestionKind::RuntimeQuota => "runtime_quota",
        // Needs a stored program, so it has its own bench fixture.
        // Needs stored insights, so it has its own bench fixture.
        QuestionKind::InsightSelection => "insight_selection",
        // Needs a tool index, so it has its own bench fixture.
        QuestionKind::ToolRelevance => "tool_relevance",
        QuestionKind::FanoutAggregate => "fanout_aggregate",
        QuestionKind::DelegationAdmission => "delegation_admission",
        QuestionKind::AnswerRanking => "answer_ranking",
        QuestionKind::ServingTechniqueActivation => "serving_technique_activation",
        QuestionKind::ReasoningParadigm => "reasoning_paradigm",
        QuestionKind::ModelCapacity => "model_capacity",
        QuestionKind::ProcessRemediation => "process_remediation",
        QuestionKind::ProcessPlan => "process_plan",
        QuestionKind::IngestShortfall => "ingest_shortfall",
        QuestionKind::CapacityProposal => "capacity_proposal",
        QuestionKind::ProbeSelection => "probe_selection",
        QuestionKind::SkillEvolution => "skill_evolution",
        QuestionKind::PlaybookTriage => "playbook_triage",
        // Need a ladder and a graph of cited records, so each has its own fixture.
        QuestionKind::TrajectoryLadder => "trajectory_ladder",
        QuestionKind::AnalystSynthesis => "analyst_synthesis",
        // Needs a role, its candidates and a market, so it has its own fixture.
        QuestionKind::Recruit => "recruit",
        // Needs a pinned policy and a conflict record, so it has its own fixture.
        QuestionKind::ResolveConflict => "resolve_conflict",
        // Needs pinned positions and evidence heads, so it has its own fixture.
        QuestionKind::CouncilCoordination => "council_coordination",
        // Needs a delivery, its gate verdicts and an owner approval, so it has its own fixture.
        QuestionKind::MergeArming => "merge_arming",
        // Needs a stored defect, symbols and blame, so it has its own bench fixture.
        QuestionKind::DefectTriage => "defect_triage",
    }
}

async fn dispatch(h: &Harness, method: Method) -> Response {
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 178,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some("decider".into()),
                method,
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes(
                "decider",
                TENANT,
                &["kg:read", "agent:decision-write"],
            ),
        ),
    )
    .await
}

/// The served-test kit fixture: one published tool and the route feature
/// schema; the request is re-kinded per family.
async fn fixture(kind: QuestionKind) -> (Harness, DecideRequest) {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    h.publish_test_component(
        "tool-search",
        AgentComponentKind::Tool,
        "web search engine",
        None::<&()>,
        None,
    )
    .unwrap();
    let (schema_pin, _) = publish_route_schema(&h);
    let mut asked = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    asked.question.kind = kind;
    asked.question.question_id = format!("bench.{}", family_name(kind));
    (h, asked)
}

fn decide(rt: &Runtime, h: &Harness, asked: &DecideRequest) -> Response {
    rt.block_on(dispatch(
        h,
        Method::Decide {
            request: Box::new(asked.clone()),
        },
    ))
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn percentile_ms(sorted: &[Duration], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

/// Evidence items per claim the `decide/claims` group holds (R652.6).
const CLAIM_EVIDENCE_ITEMS: usize = 50;

/// One claim operation through the served handler.
fn claims(rt: &Runtime, h: &Harness, op: ClaimOp) -> Response {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("bench", TENANT);
    rt.block_on(medulla_h_decide::decide::handle_claims(
        &h.state, 1, &verified, op,
    ))
}

/// The `decide/claims` group (MEDULLA-DECISION-ENGINE-R652.6): a claim holding
/// 50 evidence items is read, its cohort artifact compiled (a replay of the
/// stored seal) and the sealed artifact read back with its seal verified.
fn bench_claims(c: &mut Criterion, rt: &Runtime, report: &mut BTreeMap<String, serde_json::Value>) {
    let h = rt.block_on(Harness::new());
    let ok = |response: Response| {
        assert!(
            response.error.is_none(),
            "claims bench fixture: {response:?}"
        );
        response
    };
    ok(claims(
        rt,
        &h,
        ClaimOp::Propose {
            claim_id: "bench-claim".to_string(),
        },
    ));
    for index in 0..CLAIM_EVIDENCE_ITEMS {
        ok(claims(
            rt,
            &h,
            ClaimOp::RecordEvidence {
                evidence_id: format!("bench-e{index}"),
                claim_id: "bench-claim".to_string(),
                channel: Some(EvidenceChannel::Outcome),
                supports: index % 5 != 0,
                weight_ppm: 20_000,
                source_digest: format!("sha256:{index:064x}"),
                stances: BTreeMap::new(),
                feed: None,
            },
        ));
    }
    let compile = || ClaimOp::CompileArtifact {
        artifact_id: "bench-artifact".to_string(),
        cohort_id: "bench-cohort".to_string(),
        claim_ids: vec!["bench-claim".to_string()],
        code_specs: vec!["bench-spec".to_string()],
        exploration_edges: Vec::new(),
    };
    let journey = || {
        black_box(ok(claims(
            rt,
            &h,
            ClaimOp::Get {
                claim_id: Some("bench-claim".to_string()),
                artifact_id: None,
            },
        )));
        black_box(ok(claims(rt, &h, compile())));
        black_box(ok(claims(
            rt,
            &h,
            ClaimOp::Get {
                claim_id: None,
                artifact_id: Some("bench-artifact".to_string()),
            },
        )));
    };
    let group_name = "decide/claims".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(journey));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            journey();
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// One write-back operation through the served `DecisionLog` handler.
fn writeback(rt: &Runtime, h: &Harness, computation: WritebackOp) -> Response {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("bench", TENANT);
    rt.block_on(medulla_h_decide::decide::handle_decision_log(
        &h.state,
        1,
        &verified,
        DecisionLogOp::Writeback {
            tenant_id: TENANT.to_string(),
            computation,
        },
    ))
}

/// The `decide/writeback_queue` group (MEDULLA-DECISION-ENGINE-R615.6): one
/// high-tier proposal is queued under a fresh queue id and approved, through
/// the served `Writeback` op. Its budget is `group_budgets` in
/// `benches/baselines/decide.json`: p99 at most 15 ms.
fn bench_writeback_queue(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let h = rt.block_on(Harness::new());
    let mut next = 0u64;
    let mut journey = || {
        next += 1;
        let queue_id = format!("bench-queue-{next}");
        let proposal_id = format!("bench-proposal-{next}");
        let queued = writeback(
            rt,
            &h,
            WritebackOp::Queue {
                queue_id: queue_id.clone(),
                proposals: vec![QueuedProposalInput {
                    proposal: ProposalInput {
                        id: proposal_id.clone(),
                        external_id: format!("bench-external-{next}"),
                        system_of_record: "sor:ledger".to_string(),
                        risk_tier: RiskTier::High,
                        as_of_ms: 1_700_000_000_000,
                        connector_operation: None,
                    },
                    source_values: BTreeMap::new(),
                }],
            },
        );
        assert!(queued.error.is_none(), "writeback bench queue: {queued:?}");
        let approved = writeback(
            rt,
            &h,
            WritebackOp::Approve {
                queue_id,
                proposal_id,
            },
        );
        assert!(
            approved.error.is_none(),
            "writeback bench approve: {approved:?}"
        );
        black_box(approved);
    };
    let group_name = "decide/writeback_queue".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(&mut journey));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            journey();
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// The maximum bounded plan of MEDULLA-DECISION-ENGINE-R134.111.6: 64 tasks,
/// eight dependency edges per task and the full 64 KiB of prompt text.
fn maximum_plan_request(asked: &DecideRequest) -> DecideRequest {
    let tasks: Vec<TaskPlanTask> = (0..64usize)
        .map(|i| TaskPlanTask {
            task_id: format!("t{i:02}"),
            prompt: "x".repeat(1024),
            depends_on: BoundedVec::new((0..i).rev().take(8).map(|d| format!("t{d:02}")).collect())
                .unwrap(),
            call: None,
            aggregation: None,
            assignee: None,
            branch: None,
            join: None,
            decision: None,
            approval: None,
            duration_ms: None,
            priority_weight: None,
        })
        .collect();
    let options = tasks
        .iter()
        .map(|t| DeclaredOption {
            option_id: t.task_id.clone(),
            classification: BoundedVec::default(),
            numbers: BoundedVec::default(),
            texts: BoundedVec::default(),
        })
        .collect();
    let mut request = asked.clone();
    request.question.kind = QuestionKind::TaskPlan;
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(options).unwrap(),
    };
    request.params = BoundedVec::new(vec![TypedParam {
        name: "task_plan".into(),
        value: TypedValue::TaskPlan(Box::new(TaskPlanProposal {
            tasks: BoundedVec::new(tasks).unwrap(),
            output_task_id: "t63".into(),
            max_concurrency: 8,
            max_steps: 64,
            final_artifact: None,
        })),
    }])
    .unwrap();
    request
}

/// The `decide/task_plan_bounds` group (MEDULLA-DECISION-ENGINE-R134.111.6):
/// `plan_explicit_tasks` over the maximum plan, through the served path.
fn bench_task_plan_bounds(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let (h, asked) = rt.block_on(fixture(QuestionKind::TaskPlan));
    let asked = maximum_plan_request(&asked);
    let response = decide(rt, &h, &asked);
    assert!(
        response.error.is_none(),
        "task_plan_bounds fixture: {response:?}"
    );
    let group_name = "decide/task_plan_bounds".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(decide(rt, &h, &asked));
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// The `decide/tool_relevance_cap` group (MEDULLA-DECISION-ENGINE-R134.751): a
/// visible catalog of `tool_subset.max_tools` (default 10000) tools with the
/// tool index cache warm, ranked through the served path.
fn bench_tool_relevance_cap(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_agent_types::agent_component::{AgentComponentFacts, ToolEffect};
    use medulla_decision_types::decision::tool_relevance::ToolAssignmentRequest;
    use medulla_persistence_records::agent_component::test_component_draft;

    let (h, mut asked) = rt.block_on(fixture(QuestionKind::ToolRelevance));
    // The cap is the knob default; the catalog holds exactly that many tools.
    let cap = 10_000;
    for index in 0..cap {
        let id = format!("tool-cap-{index:05}");
        let mut draft = test_component_draft(TENANT, &id);
        draft.summary = format!("tool number {index} handles topic{} requests", index % 97);
        draft.facts = AgentComponentFacts::Tool {
            effect: ToolEffect::Read,
            required_scopes: Vec::new(),
            input_schema_digest: None,
            output_schema_digest: None,
            read_only_hint: None,
            destructive_hint: None,
            idempotent_hint: None,
            open_world_hint: None,
            modalities: Default::default(),
            cost: None,
            latency_declared: None,
            side_effects: None,
            declared_by: None,
            schema_tokens: Some(50),
            read_only_unproven: false,
        };
        draft
            .attributes
            .insert("name".to_string(), format!("tool {index}"));
        h.commit_in(TENANT, draft, None).unwrap();
    }
    asked.params = BoundedVec::new(vec![TypedParam {
        name: "tool_assignment".to_string(),
        value: TypedValue::ToolAssignment(Box::new(ToolAssignmentRequest {
            schema_version: 1,
            task_text: "handles topic5 requests".to_string(),
            task_embedding: None,
            agent_role_iri: String::new(),
            top_k: 12,
            context_budget_tokens: 4000,
            required_capabilities: BoundedVec::default(),
        })),
    }])
    .unwrap();
    // The first call builds the index; every timed call then hits the cache.
    let response = decide(rt, &h, &asked);
    assert!(
        response.error.is_none(),
        "tool_relevance_cap fixture: {response:?}"
    );
    let group_name = "decide/tool_relevance_cap".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(decide(rt, &h, &asked));
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// Insights the `decide/insight_selection` group holds.
const INSIGHT_ROWS: usize = 200;

/// The `decide/insight_selection` group (MEDULLA-DECISION-ENGINE-R135.15): the
/// served `Decide` selects among stored insights, each submitted through the
/// served `DecisionLog` run-record write.
fn bench_insight_selection(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_types::decision::program_store::insight::{
        Insight, INSIGHT_SCHEMA_VERSION,
    };
    use medulla_decision_types::decision::program_store::RunRecord;
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;

    let (h, mut asked) = rt.block_on(fixture(QuestionKind::InsightSelection));
    for index in 0..INSIGHT_ROWS {
        let insight = Insight {
            schema_version: INSIGHT_SCHEMA_VERSION,
            insight_id: format!("bench-insight-{index:04}"),
            text: format!("retry step {index} once before escalating"),
            source_run_ids: BoundedVec::new(vec![format!("bench-run-{index:04}")]).unwrap(),
            context_signature: format!("export weekly report variant{}", index % 7),
            success_count: 1 + (index % 5) as u32,
            selection_revision: 1,
        };
        let response = rt.block_on(dispatch(
            &h,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::RunRecord {
                    tenant_id: TENANT.to_string(),
                    record: RunRecord::Insight {
                        insight: serde_json::to_value(&insight).unwrap(),
                    },
                }),
            },
        ));
        assert!(
            response.error.is_none(),
            "insight_selection fixture: {response:?}"
        );
    }
    asked.params = BoundedVec::new(vec![TypedParam {
        name: "context_signature".to_string(),
        value: TypedValue::Text("export weekly report variant3".to_string()),
    }])
    .unwrap();
    let response = decide(rt, &h, &asked);
    assert!(
        response.error.is_none(),
        "insight_selection fixture: {response:?}"
    );
    let group_name = "decide/insight_selection".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(decide(rt, &h, &asked));
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// Candidates the `decide/distilled_rank` group ranks (R147.6).
const DISTILLED_CANDIDATES: usize = 64;

/// The `decide/distilled_rank` group (MEDULLA-DECISION-ENGINE-R147.6): teacher
/// scores for 64 graph candidates are uploaded once through `DecisionLog`
/// `Learn`; the timed work is the served `Rank` from the stored scores.
fn bench_distilled_rank(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    const GRAPH: &str = "kg-bench-distilled";
    let h = rt.block_on(Harness::with_isolation(ServerState::test_isolation(
        "decider",
    )));
    rt.block_on(async {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        for index in 0..DISTILLED_CANDIDATES {
            let props = serde_json::json!({"type": "Tool", "score": 0.5});
            core.add_node(
                format!("n-{index:03}"),
                rmp_serde::to_vec_named(&props).unwrap(),
            );
        }
    });
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "score".to_string(),
            kind: FeatureKind::Number {
                key: "score".to_string(),
            },
            missing: MissingValue::Abstain,
        }])
        .unwrap(),
    };
    let pin = h
        .publish_test_component(
            "schema-distilled",
            AgentComponentKind::FeatureSchema,
            "distilled features",
            Some(&body),
            None,
        )
        .unwrap();
    let scores: Vec<TeacherJudgment> = (0..DISTILLED_CANDIDATES)
        .map(|index| TeacherJudgment {
            document_id: format!("n-{index:03}"),
            score_q16: ((index * 7) % DISTILLED_CANDIDATES) as u32 * 100 + 1,
            fidelity_lo_q16: 40_000,
            fidelity_hi_q16: 65_536,
        })
        .collect();
    let uploaded = rt.block_on(medulla_h_decide::decide::handle_decision_log(
        &h.state,
        9,
        &VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT),
        DecisionLogOp::Learn {
            tenant_id: TENANT.to_string(),
            write: LearningWrite::UploadDistilledScores {
                upload: Box::new(DistilledUpload {
                    scorer_id: "bench-scorer".to_string(),
                    scorer_revision: 1,
                    scores: BoundedVec::new(scores).unwrap(),
                }),
            },
        },
    ));
    assert!(
        uploaded.error.is_none(),
        "distilled_rank upload: {uploaded:?}"
    );
    let mut asked = request(
        &pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    asked.question.kind = QuestionKind::Rank;
    asked.question.question_id = "bench.distilled_rank".to_string();
    asked.candidates = CandidateSource::Graph {
        graph: GRAPH.to_string(),
        plan: Box::new(medulla_query_types::wire::Plan::new(vec![
            medulla_query_types::wire::Op::Scan {
                label: "Tool".to_string(),
            },
        ])),
    };
    asked.params = BoundedVec::new(vec![TypedParam {
        name: "distilled_scorer".to_string(),
        value: TypedValue::DistilledScorer {
            scorer_id: "bench-scorer".to_string(),
            scorer_revision: 1,
            top_k: 10,
        },
    }])
    .unwrap();
    let response = decide(rt, &h, &asked);
    assert!(
        response.error.is_none(),
        "distilled_rank fixture: {response:?}"
    );
    let group_name = "decide/distilled_rank".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(decide(rt, &h, &asked));
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}
/// The widest bounded plan: six slots whose widths total 64 agents, at the
/// round cap, with every slot declaring tokens.
fn widest_plan() -> TopologyPlan {
    let widths = [16u8, 16, 16, 8, 4, 4];
    let slots = widths
        .iter()
        .enumerate()
        .map(|(index, width)| SlotPlan {
            node_id: format!("slot{index}"),
            width: *width,
            rounds: 8,
            tokens: Some(1_000),
        })
        .collect();
    TopologyPlan {
        class_iri: "bench:widest".to_string(),
        slots: BoundedVec::new(slots).unwrap(),
        stop: StopRule::MaxRounds { n: 8 },
        lease: LeasePlan {
            per_cell: BoundedVec::default(),
            priority: LeasePriority::Orchestration,
        },
        allowances: BoundedVec::default(),
        makespan_ms: None,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R046.751
/// The `decide/topology_admit` group (MEDULLA-DECISION-ENGINE-R046.751):
/// `admit_topology` alone over the widest bounded plan, the 64-agent plan
/// held to the engine caps. Its budget is `group_budgets` in
/// `benches/baselines/decide.json`: p50 at most 0.2 ms, p99 at most 0.8 ms.
fn bench_topology_admit(c: &mut Criterion, report: &mut BTreeMap<String, serde_json::Value>) {
    let plan = widest_plan();
    let caps = TopologyPolicy::engine_default().caps;
    let admit = || black_box(admit_topology(black_box(&plan), 1, black_box(&caps)));
    assert!(admit().is_ok(), "topology bench fixture must be admitted");
    let group_name = "decide/topology_admit".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("decide_topology_admit", |b| b.iter(admit));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES * 10)
        .map(|_| {
            let start = Instant::now();
            let _ = admit();
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

// spec: MEDULLA-DECISION-ENGINE-R135.6
/// The task signature the `decide/program_lookup` group looks up.
const PROGRAM_SIGNATURE: &str = "summarize-report";

/// A successful run of `MAX_PROGRAM_STEPS` steps with two literals a step: the
/// widest program the store holds.
fn widest_trace() -> RunTraceRecord {
    let step = |index: usize| TraceStep {
        index: index as u32,
        kind: StepKind::ToolCall,
        action_ref: format!("tool:t{index}"),
        outcome: StepOutcome::Ok,
        arguments: BoundedVec::new(vec![
            TraceArgument {
                name: "title".into(),
                value: TraceScalar::Text(format!("weekly-{index}")),
            },
            TraceArgument {
                name: "limit".into(),
                value: TraceScalar::Int(index as i64),
            },
        ])
        .unwrap(),
    };
    RunTraceRecord {
        schema_version: 1,
        run_id: "bench-run".into(),
        task_signature: PROGRAM_SIGNATURE.into(),
        model_id: "model-1".into(),
        steps: BoundedVec::new((0..MAX_PROGRAM_STEPS).map(step).collect()).unwrap(),
        outcome: RunOutcome::Succeeded,
        started_at_ms: 1,
    }
}

/// The `decide/program_lookup` group (MEDULLA-DECISION-ENGINE-R135.6, R178.1):
/// a stored compiled program is looked up by task signature through the served
/// `Decide` path. The fixture seeds the widest program under its run-record key
/// and asserts the first answer is the stored program, so the group times the
/// hit, not the `NO_PROGRAM` refusal.
fn bench_program_lookup(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let h = rt.block_on(Harness::new());
    let program = compile_program(
        &widest_trace(),
        &ProgramKnobs {
            max_steps: MAX_PROGRAM_STEPS,
            match_threshold_micros: 850_000,
            match_candidates: 8,
        },
    )
    .expect("the widest trace compiles");
    h.store
        .put_decision_artifacts(
            TENANT,
            &[(
                program_key(PROGRAM_SIGNATURE),
                encode_artifact(&program).unwrap(),
            )],
        )
        .expect("seed the stored program");
    let (schema_pin, _) = publish_route_schema(&h);
    let asked = declared_request(
        &schema_pin,
        TENANT,
        QuestionKind::ProgramLookup,
        "program",
        DecisionPolicyRef::Default,
        vec![TypedParam {
            name: "task_signature".into(),
            value: TypedValue::Text(PROGRAM_SIGNATURE.into()),
        }],
    );
    let hit = serde_json::to_string(&decide(rt, &h, &asked).result).unwrap();
    assert!(
        hit.contains("program:{") && !hit.contains("NO_PROGRAM"),
        "program_lookup fixture must answer the stored program: {hit}"
    );
    let group_name = "decide/program_lookup".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(decide(rt, &h, &asked));
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

const AGENT_LOOP_SECRET: &str = "agent-loop-bench-secret";
const AGENT_LOOP_ENCRYPTION_KEY: &str = "agent-loop-bench-encryption-key-0123456789";
const AGENT_LOOP_GRAPH: &str = "agentloop";

fn loop_blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).expect("agent-loop bench blob")
}

/// One mixed batch of 64 operations: creates, compare-and-sets, reinforcements and reads of
/// nodes this same batch wrote.
fn agent_loop_batch(round: u64) -> Method {
    use medulla_query_types::wire::BatchOperation;
    let mut operations = Vec::with_capacity(
        usize::try_from(medulla_server_api::knob_config::knobs().batch_max_operations)
            .expect("the batch cap fits usize"),
    );
    for slot in 0..32u64 {
        operations.push(BatchOperation::CreateNodeIfAbsent {
            node_id: format!("loop-{round}-{slot}"),
            properties_msgpack: loop_blob(serde_json::json!({"label": "step", "n": slot})),
        });
    }
    for slot in 0..16u64 {
        operations.push(BatchOperation::CompareAndSetNodeFields {
            node_id: format!("loop-{round}-{slot}"),
            conditions_msgpack: loop_blob(serde_json::json!({"n": slot})),
            updates_msgpack: loop_blob(serde_json::json!({"done": true})),
        });
    }
    for slot in 0..8u64 {
        operations.push(BatchOperation::Reinforce {
            node_id: format!("loop-{round}-{slot}"),
            now_ms: 1_800_000_000_000 + round,
            weight: 1.0,
        });
    }
    for slot in 0..8u64 {
        operations.push(BatchOperation::GetNodeProperties {
            node_id: format!("loop-{round}-{slot}"),
        });
    }
    Method::Batch { operations }
}

/// The `decide/agent_loop` groups (MEDULLA-DURABLE-KERNEL-R871 and
/// MEDULLA-DECISION-ENGINE-R178): one served read plus one served write per sample
/// (`decide/agent_loop`, p99 at most 2 ms) and one served `Batch` of 64 operations per sample
/// (`decide/agent_loop_batch`, p99 at most 10 ms), through the real dispatch entry over a
/// durable graph. Budgets: `group_budgets` in `benches/baselines/decide.json`.
fn bench_agent_loop(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let (_dir, _backend, state) = rt.block_on(test_support::durable_graph_state(
        AGENT_LOOP_SECRET,
        AGENT_LOOP_ENCRYPTION_KEY,
        "agent-loop-bench",
        common::current_isolation(),
        AGENT_LOOP_GRAPH,
    ));
    let mut next_id = 1_000u64;
    let mut call = |method: Method| {
        next_id += 1;
        let request = test_support::request(AGENT_LOOP_SECRET, next_id, AGENT_LOOP_GRAPH, method);
        let start = Instant::now();
        let response = rt.block_on(test_support::dispatch(&state, request));
        let elapsed = start.elapsed();
        assert!(
            response.error.is_none(),
            "agent_loop bench fixture: {:?}",
            response.error
        );
        elapsed
    };
    let mut single = |round: u64| {
        let write = call(Method::CreateNodeIfAbsent {
            node_id: format!("single-{round}"),
            properties_msgpack: loop_blob(serde_json::json!({"label": "step", "n": round})),
        });
        let read = call(Method::GetNodeProperties {
            node_id: format!("single-{round}"),
        });
        write.max(read)
    };
    let mut measure = |name: &str, run: &mut dyn FnMut(u64) -> Duration| {
        let group_name = format!("decide/{name}");
        let mut group = c.benchmark_group(&group_name);
        let mut round = 0u64;
        group.bench_function("served", |b| {
            b.iter(|| {
                round += 1;
                black_box(run(round))
            })
        });
        group.finish();
        let before = high_water_mib();
        let mut samples: Vec<Duration> = (0..REPORT_SAMPLES as u64)
            .map(|index| run(1_000_000 + index))
            .collect();
        samples.sort();
        report.insert(
            group_name,
            serde_json::json!({
                "p50_ms": percentile_ms(&samples, 0.50),
                "p99_ms": percentile_ms(&samples, 0.99),
                "peak_mib": (high_water_mib() - before).max(0.0),
            }),
        );
    };
    measure("agent_loop", &mut single);
    let mut batch = |round: u64| call(agent_loop_batch(round));
    measure("agent_loop_batch", &mut batch);
}

/// Edit facts the `decide/edit_concentration/kernel` group holds
/// (MEDULLA-DECISION-ENGINE-R173.6).
const CONCENTRATION_FACTS: u32 = 10_000;

/// The `decide/edit_concentration/kernel` group: `concentration_verdict` over
/// 10,000 shipped edit facts on one dimension, the candidate at the last round.
fn bench_concentration_kernel(c: &mut Criterion, report: &mut BTreeMap<String, serde_json::Value>) {
    let dimension = "dimension:tool-selection";
    let fact = |n: u32, status: EditStatus| EditFact {
        edit_id: format!("edit:{n}"),
        dimension_iri: dimension.to_string(),
        round: n % 50 + 1,
        standing: status,
        policy_revision: 1,
    };
    let facts: Vec<EditFact> = (0..CONCENTRATION_FACTS)
        .map(|n| fact(n, EditStatus::Shipped))
        .collect();
    let candidate = EditFact {
        edit_id: "edit:candidate".to_string(),
        dimension_iri: dimension.to_string(),
        round: 50,
        standing: EditStatus::Proposed,
        policy_revision: 1,
    };
    let policy = ConcentrationPolicy {
        policy_iri: "policy:concentration".to_string(),
        min_shipped: 3,
        window_rounds: 5,
        revision: 1,
    };
    let verdict = || {
        black_box(concentration_verdict(
            black_box(&facts),
            black_box(&candidate),
            black_box(Some(&policy)),
        ))
    };
    assert!(
        verdict().is_ok(),
        "concentration bench fixture must conclude"
    );
    let group_name = "decide/edit_concentration/kernel".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("concentration_verdict", |b| b.iter(verdict));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES * 10)
        .map(|_| {
            let start = Instant::now();
            let _ = verdict();
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// Deterministic xorshift generator: the bench fixture needs no entropy source.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A value in `[-1, 1)`.
    fn unit(&mut self) -> f32 {
        ((self.next() >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }

    fn vector(&mut self, dim: usize) -> Vec<f32> {
        let raw: Vec<f32> = (0..dim).map(|_| self.unit()).collect();
        let norm = raw
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(f32::MIN_POSITIVE);
        raw.iter().map(|v| v / norm).collect()
    }
}

const DESIGNATION_VERSION: &str = "bench-emb";
const DESIGNATION_DIM: usize = 1024;
const DESIGNATION_K: usize = 5;
const DESIGNATION_QUERIES: usize = 200;
const RARE_CAPABILITY: &str = "cap:rare";

fn designation_record(
    tenant: &medulla_types::contract::TenantId,
    index: usize,
) -> medulla_decision_types::decision::capability_designation::CapabilityRecord {
    use medulla_decision_types::decision::capability_designation::{CapabilityRecord, EntityKind};
    // Eight capabilities per record; every 25th record (4 %) also holds the
    // rare one, so a filter on it selects at most 5 % of the records.
    let mut capabilities: std::collections::BTreeSet<String> = (0..8)
        .map(|slot| format!("cap:{}", (index * 7 + slot * 13) % 64))
        .collect();
    if index.is_multiple_of(25) {
        capabilities.insert(RARE_CAPABILITY.to_string());
    }
    while capabilities.len() > 8 {
        let drop = capabilities.iter().find(|c| *c != RARE_CAPABILITY).cloned();
        capabilities.remove(&drop.unwrap());
    }
    CapabilityRecord {
        tenant_id: Some(tenant.clone()),
        entity_id: medulla_types::contract::OpaqueId::new(format!("agent-{index}")).unwrap(),
        entity_kind: EntityKind::Agent,
        capabilities,
        ontology_type: None,
        policy_tags: std::collections::BTreeSet::new(),
        swappable_with: std::collections::BTreeSet::new(),
        embedding_version: DESIGNATION_VERSION.to_string(),
        dim: DESIGNATION_DIM as u16,
        record_revision: 1,
        applies_to: std::collections::BTreeSet::new(),
        executes: std::collections::BTreeSet::new(),
        pack: None,
    }
}

/// The `decide/capability_designation` group (MEDULLA-DECISION-ENGINE-R172.10):
/// the resident index at the reference load (`records` records of dim 1024,
/// eight capabilities each, k = 5, one core). The group's `p50_ms`/`p99_ms`
/// are the unfiltered read; the filtered read (one required capability
/// selecting at most 5 %), recall@5 against an exact scan, the incremental
/// upsert p99 and the resident bytes ride beside them for
/// `cargo xtask decide-budgets`.
fn bench_capability_designation(
    c: &mut Criterion,
    group_name: &str,
    records: usize,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_memory::memory::capability_index::{CapabilityIndex, ShardLimits};
    use medulla_decision_types::decision::capability_designation::DesignationKnobs;
    use medulla_decision_types::decision::DecisionPolicy;
    use std::collections::{BTreeMap as Map, BTreeSet as Set};

    let knobs = DesignationKnobs::from_policy(&DecisionPolicy::engine_default());
    let tenant = medulla_types::contract::TenantId::new(TENANT).unwrap();
    let index = CapabilityIndex::new(ShardLimits {
        max_records: records + 1_024,
        compact_ratio_q32: knobs.tombstone_compact_ratio_q32,
    });
    let mut rng = Xorshift(0x0172_1010_5eed_cafe);
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(records);
    for i in 0..records {
        let vector = rng.vector(DESIGNATION_DIM);
        index
            .upsert(designation_record(&tenant, i), vector.clone())
            .expect("bench upsert");
        vectors.push(vector);
    }
    let queries: Vec<Vec<f32>> = (0..DESIGNATION_QUERIES)
        .map(|_| rng.vector(DESIGNATION_DIM))
        .collect();
    let shortlist = (knobs.oversample_factor.max(1) * DESIGNATION_K).max(DESIGNATION_K);
    let none: Set<String> = Set::new();
    let no_subtypes: Map<String, Set<String>> = Map::new();
    let rare: Set<String> = Set::from([RARE_CAPABILITY.to_string()]);
    let rare_subtypes: Map<String, Set<String>> =
        Map::from([(RARE_CAPABILITY.to_string(), Set::new())]);
    let read = |query: &[f32], required: &Set<String>, subtypes: &Map<String, Set<String>>| {
        index
            .read(
                &tenant,
                DESIGNATION_VERSION,
                query,
                required,
                &none,
                subtypes,
                shortlist,
                knobs.flat_threshold,
            )
            .expect("bench read")
    };
    let mut group = c.benchmark_group(group_name);
    group.bench_function("unfiltered", |b| {
        let mut at = 0;
        b.iter(|| {
            at = (at + 1) % DESIGNATION_QUERIES;
            black_box(read(&queries[at], &none, &no_subtypes))
        });
    });
    group.bench_function("filtered", |b| {
        let mut at = 0;
        b.iter(|| {
            at = (at + 1) % DESIGNATION_QUERIES;
            black_box(read(&queries[at], &rare, &rare_subtypes))
        });
    });
    group.finish();

    let time = |mut run: Box<dyn FnMut(usize) + '_>| {
        let mut samples: Vec<Duration> = (0..DESIGNATION_QUERIES)
            .map(|at| {
                let start = Instant::now();
                run(at);
                start.elapsed()
            })
            .collect();
        samples.sort();
        samples
    };
    let unfiltered = time(Box::new(|at| {
        black_box(read(&queries[at], &none, &no_subtypes));
    }));
    let filtered = time(Box::new(|at| {
        black_box(read(&queries[at], &rare, &rare_subtypes));
    }));
    let filtered_share =
        read(&queries[0], &rare, &rare_subtypes).capability_filtered as f64 / records as f64;
    assert!(
        filtered_share <= 0.05,
        "the filter selects {filtered_share}"
    );

    // Recall@5 against an exact scan of every stored vector.
    let mut found = 0usize;
    for query in &queries[..50] {
        let mut exact: Vec<(usize, f32)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i, v.iter().zip(query).map(|(a, b)| a * b).sum()))
            .collect();
        exact.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let truth: Set<String> = exact[..DESIGNATION_K]
            .iter()
            .map(|(i, _)| format!("agent-{i}"))
            .collect();
        let got = read(query, &none, &no_subtypes);
        found += got
            .hits
            .iter()
            .take(DESIGNATION_K)
            .filter(|hit| truth.contains(hit.record.entity_id.as_str()))
            .count();
    }
    let recall = found as f64 / (50 * DESIGNATION_K) as f64;

    // Incremental upsert of a replacement record, without durable commit.
    let upserts = time(Box::new(|at| {
        let i = at % records;
        index
            .upsert(designation_record(&tenant, i), vectors[i].clone())
            .expect("bench replace");
    }));
    let resident = index
        .shard(Some(&tenant))
        .map_or(0, |shard| shard.snapshot().resident_bytes());
    let before = high_water_mib();
    report.insert(
        group_name.to_string(),
        serde_json::json!({
            "p50_ms": percentile_ms(&unfiltered, 0.50),
            "p99_ms": percentile_ms(&unfiltered, 0.99),
            "peak_mib": high_water_mib().max(before),
            "filtered_p50_ms": percentile_ms(&filtered, 0.50),
            "filtered_p99_ms": percentile_ms(&filtered, 0.99),
            "recall_at_5": recall,
            "upsert_p99_ms": percentile_ms(&upserts, 0.99),
            "resident_bytes": resident,
            "records": records,
            "dim": DESIGNATION_DIM,
        }),
    );
}

/// Nodes the nightly `decide/retrieval_learning` cycle run re-embeds.
const CYCLE_NODES: usize = 10_000;
/// Nodes one served batch re-embeds.
const CYCLE_BATCH: usize = 256;
/// Rank and width of the adapter the query-latency budget is held at.
const ADAPTER_RANK: usize = 8;
const ADAPTER_DIM: usize = 1024;

/// The `decide/retrieval_learning` groups (MEDULLA-DECISION-ENGINE-R174.6 and
/// R174.7.6). The group itself times a generation cycle over 10,000 nodes
/// through the served `DecisionLog` `Learn` path, one 256-node batch per
/// submission, and reports the engine overhead of a batch: the served time
/// minus the time the embedder takes for the same texts (`p99_ms`, budget 1 ms).
/// `decide/retrieval_learning/adapter_query` times the query adapter applied to
/// a query at `d = 1024` and rank 8, which is the latency a served `Discover`
/// gains once an adapter is active (`p99_ms`, budget 0.05 ms).
fn bench_retrieval_learning(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_stats::decision::adapter::{to_body, AdapterKernel, Direction};
    use medulla_exec::exec::TextEmbedder;
    use medulla_h_decide::decide::stat_test_support::{
        generation_cycle_fixture, submit_generation_cycle, CYCLE_DIMS,
    };

    let group_name = "decide/retrieval_learning".to_string();
    let h = rt.block_on(Harness::with_isolation(ServerState::test_isolation(
        "decider",
    )));
    let ledger = tempfile::tempdir().expect("bench ledger dir");
    let request = rt.block_on(generation_cycle_fixture(&h, ledger.path(), CYCLE_NODES));
    let hash = medulla_exec::exec::HashEmbedder::new(CYCLE_DIMS);
    let texts: Vec<String> = (0..CYCLE_BATCH).map(|i| format!("doc-{i}")).collect();
    let embed = || {
        for text in &texts {
            black_box(hash.embed(text).expect("bench embed"));
        }
    };
    let embedding = {
        let mut times: Vec<Duration> = (0..20)
            .map(|_| {
                let start = Instant::now();
                embed();
                start.elapsed()
            })
            .collect();
        times.sort();
        times[times.len() / 2]
    };
    let before = high_water_mib();
    let mut overhead: Vec<Duration> = Vec::new();
    loop {
        let start = Instant::now();
        let (state, nodes) = rt
            .block_on(submit_generation_cycle(&h, &request))
            .expect("bench cycle submission");
        let elapsed = start.elapsed();
        if state == "Activated" {
            assert_eq!(
                nodes as usize, CYCLE_NODES,
                "the cycle re-embedded every node"
            );
            break;
        }
        overhead.push(elapsed.saturating_sub(embedding));
    }
    overhead.sort();
    assert!(
        overhead.len() >= CYCLE_NODES / CYCLE_BATCH - 1,
        "a 10,000-node cycle runs in 256-node batches"
    );
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("embedding_floor", |b| b.iter(embed));
    group.finish();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&overhead, 0.50),
            "p99_ms": percentile_ms(&overhead, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
            "batches": overhead.len(),
            "nodes": CYCLE_NODES,
        }),
    );

    let mut rng = Xorshift(0x0174_7600_5eed_cafe);
    let directions: Vec<Direction> = (0..ADAPTER_RANK)
        .map(|_| {
            let unit: Vec<f64> = rng.vector(ADAPTER_DIM).into_iter().map(f64::from).collect();
            Direction { unit, gain: 0.25 }
        })
        .collect();
    let body = to_body(
        &directions,
        "sha256:bench",
        ADAPTER_DIM,
        ("bench".into(), 1),
    )
    .expect("bench adapter body");
    let kernel = AdapterKernel::of(&body).expect("bench adapter kernel");
    let queries: Vec<Vec<f32>> = (0..DESIGNATION_QUERIES)
        .map(|_| rng.vector(ADAPTER_DIM))
        .collect();
    let adapter_group = "decide/retrieval_learning/adapter_query".to_string();
    let mut group = c.benchmark_group(&adapter_group);
    group.bench_function("apply", |b| {
        let mut at = 0;
        b.iter(|| {
            at = (at + 1) % DESIGNATION_QUERIES;
            black_box(kernel.apply(&queries[at]))
        });
    });
    group.finish();
    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..DESIGNATION_QUERIES * 10)
        .map(|at| {
            let start = Instant::now();
            black_box(kernel.apply(&queries[at % DESIGNATION_QUERIES]));
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        adapter_group,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
            "rank": ADAPTER_RANK,
            "dim": ADAPTER_DIM,
        }),
    );
}

/// Packs and tools per pack the `decide/load_set` group seeds (R176.6): 5,000
/// tools across 100 packs.
const LOAD_SET_PACKS: usize = 100;
const LOAD_SET_TOOLS_PER_PACK: usize = 50;
const LOAD_SET_DIM: usize = 16;
const LOAD_SET_VERSION: &str = "bench-load-set";
const LOAD_SET_TOOLS: u16 = 32;

/// A request of `kind` whose one typed parameter is `value` and whose one
/// declared candidate is the question id.
fn typed_request(
    asked: &DecideRequest,
    question_id: &str,
    params: Vec<TypedParam>,
) -> DecideRequest {
    let mut out = asked.clone();
    out.question.question_id = question_id.to_string();
    out.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![DeclaredOption {
            option_id: question_id.to_string(),
            classification: BoundedVec::default(),
            numbers: BoundedVec::default(),
            texts: BoundedVec::default(),
        }])
        .expect("one option fits the bound"),
    };
    out.params = BoundedVec::new(params).expect("params fit the bound");
    out
}

/// The `decide/load_set` group (MEDULLA-DECISION-ENGINE-R176.6): the served
/// `LoadSet` question over a resident capability index of 5,000 tools in 100
/// packs, one core. The budget is p95 <= 50 ms; the report's p99 is held
/// to the same 50 ms, which is the stricter bound.
fn bench_load_set(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_types::decision::capability_designation::{
        CapabilityRecord, EntityKind, PackOrigin,
    };
    use medulla_decision_types::decision::load_set::{LoadSetQuestion, LOAD_SET_PARAM};
    use medulla_h_decide::decide::capability_feed::shared_capability_index;

    let (h, asked) = rt.block_on(fixture(QuestionKind::LoadSet));
    let tenant = medulla_types::contract::TenantId::new(TENANT).unwrap();
    let index = rt.block_on(async { shared_capability_index(&*h.state.read().await) });
    let mut rng = Xorshift(0x0176_0006_5eed_cafe);
    for pack in 0..LOAD_SET_PACKS {
        let connector = format!("bench-pack-{pack}");
        for slot in 0..LOAD_SET_TOOLS_PER_PACK {
            let ordinal = pack * LOAD_SET_TOOLS_PER_PACK + slot;
            let record = CapabilityRecord {
                tenant_id: Some(tenant.clone()),
                entity_id: medulla_types::contract::OpaqueId::new(format!(
                    "mcp:{connector}/tool/t{slot}"
                ))
                .unwrap(),
                entity_kind: EntityKind::Tool,
                capabilities: std::collections::BTreeSet::from([format!("cap:{}", ordinal % 64)]),
                ontology_type: None,
                policy_tags: std::collections::BTreeSet::new(),
                swappable_with: std::collections::BTreeSet::new(),
                embedding_version: LOAD_SET_VERSION.to_string(),
                dim: LOAD_SET_DIM as u16,
                record_revision: 1,
                applies_to: std::collections::BTreeSet::new(),
                executes: std::collections::BTreeSet::new(),
                pack: Some(PackOrigin {
                    connector: connector.clone(),
                    binding_revision: 1,
                    pack_digest: format!("sha256:{}", "ab".repeat(32)),
                    entry_digest: format!("sha256:{}", "cd".repeat(32)),
                }),
            };
            index
                .upsert(record, rng.vector(LOAD_SET_DIM))
                .expect("bench upsert");
        }
    }
    let question = LoadSetQuestion {
        job_text: "look up an order and close the ticket".to_string(),
        job_embedding: rng.vector(LOAD_SET_DIM),
        required_capabilities: Vec::new(),
        max_tools: LOAD_SET_TOOLS,
        max_agents: 4,
        graph: "__commons__".to_string(),
        embedding_version: LOAD_SET_VERSION.to_string(),
    };
    let asked = typed_request(
        &asked,
        "bench.load_set",
        vec![TypedParam {
            name: LOAD_SET_PARAM.to_string(),
            value: TypedValue::LoadSet(Box::new(question)),
        }],
    );
    let served = decide(rt, &h, &asked);
    assert!(
        served.error.is_none(),
        "the load set bench question must be answered: {:?}",
        served.error
    );
    let group_name = "decide/load_set".to_string();
    let mut group = c.benchmark_group(format!("{group_name}/served_5000"));
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();
    report.insert(group_name, sampled_report(rt, &h, &asked));
}

/// Time `REPORT_SAMPLES` served calls of `asked`: p50, p99 and the peak
/// resident growth.
fn sampled_report(rt: &Runtime, h: &Harness, asked: &DecideRequest) -> serde_json::Value {
    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(decide(rt, h, asked));
            start.elapsed()
        })
        .collect();
    samples.sort();
    serde_json::json!({
        "p50_ms": percentile_ms(&samples, 0.50),
        "p99_ms": percentile_ms(&samples, 0.99),
        "peak_mib": (high_water_mib() - before).max(0.0),
    })
}

/// The pack facts the `decide/usage_guidance` group reads: one connector with
/// three tools, a job class with three steps and the outcome-free ordering.
const USAGE_PACK_UPDATE: &str = "INSERT DATA { \
<mcp-server://bench> <https://knuckles-team.github.io/ontology/connector-pack#providesFeature> <urn:feature:bench> . \
<urn:feature:bench> <https://knuckles-team.github.io/ontology/connector-pack#realizedBy> <tool://bench/find> . \
<urn:feature:bench> <https://knuckles-team.github.io/ontology/connector-pack#realizedBy> <tool://bench/read> . \
<urn:feature:bench> <https://knuckles-team.github.io/ontology/connector-pack#realizedBy> <tool://bench/label> . \
<tool://bench/find> <https://knuckles-team.github.io/ontology/connector-pack#providesCapability> <cap:find> . \
<tool://bench/read> <https://knuckles-team.github.io/ontology/connector-pack#providesCapability> <cap:read> . \
<tool://bench/label> <https://knuckles-team.github.io/ontology/connector-pack#providesCapability> <cap:label> . \
<urn:job:bench> <https://knuckles-team.github.io/ontology/core#hasStep> <urn:step:find> . \
<urn:job:bench> <https://knuckles-team.github.io/ontology/core#hasStep> <urn:step:read> . \
<urn:job:bench> <https://knuckles-team.github.io/ontology/core#hasStep> <urn:step:label> . \
<urn:step:find> <https://knuckles-team.github.io/ontology/core#needsSkill> <skill://find> . \
<urn:step:read> <https://knuckles-team.github.io/ontology/core#needsSkill> <skill://read> . \
<urn:step:label> <https://knuckles-team.github.io/ontology/core#needsSkill> <skill://label> . \
<skill://find> <https://knuckles-team.github.io/ontology/connector-pack#providesCapability> <cap:find> . \
<skill://read> <https://knuckles-team.github.io/ontology/connector-pack#providesCapability> <cap:read> . \
<skill://label> <https://knuckles-team.github.io/ontology/connector-pack#providesCapability> <cap:label> . \
<urn:step:find> <https://knuckles-team.github.io/ontology/core#precedes> <urn:step:label> . \
<urn:step:read> <https://knuckles-team.github.io/ontology/core#precedes> <urn:step:label> . }";

/// The `decide/usage_guidance` group (MEDULLA-DECISION-ENGINE-R177.6): the
/// served `UsageGuidance` question over a pack and job class written to the
/// commons graph through the served update path.
fn bench_usage_guidance(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_types::decision::usage_guidance::{
        USAGE_GUIDANCE_CONNECTOR_PARAM, USAGE_GUIDANCE_GRAPH_PARAM, USAGE_GUIDANCE_JOB_CLASS_PARAM,
    };

    let (h, asked) = rt.block_on(fixture(QuestionKind::UsageGuidance));
    let written = rt.block_on(Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 177,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some("decider".into()),
                method: Method::ApplyMutation {
                    event_type: medulla_server_api::api::dispatch_support::SPARQL_HTTP_UPDATE_EVENT
                        .to_string(),
                    query: USAGE_PACK_UPDATE.to_string(),
                },
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes(
                "decider",
                TENANT,
                &["kg:read", "kg:write", "agent:decision-write"],
            ),
        ),
    ));
    assert!(
        written.error.is_none(),
        "the usage guidance pack facts must be written: {:?}",
        written.error
    );
    let asked = typed_request(
        &asked,
        "bench.usage_guidance",
        vec![
            TypedParam {
                name: USAGE_GUIDANCE_CONNECTOR_PARAM.to_string(),
                value: TypedValue::Iri("mcp-server://bench".to_string()),
            },
            TypedParam {
                name: USAGE_GUIDANCE_GRAPH_PARAM.to_string(),
                value: TypedValue::Text("__commons__".to_string()),
            },
            TypedParam {
                name: USAGE_GUIDANCE_JOB_CLASS_PARAM.to_string(),
                value: TypedValue::Iri("urn:job:bench".to_string()),
            },
        ],
    );
    let served = decide(rt, &h, &asked);
    assert!(
        served.error.is_none(),
        "the usage guidance bench question must be answered: {:?}",
        served.error
    );
    let group_name = "decide/usage_guidance".to_string();
    let mut group = c.benchmark_group(format!("{group_name}/served_pack"));
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();
    report.insert(group_name, sampled_report(rt, &h, &asked));
}

/// Calls of `choose_expansion` one sample of `decide/thought_search_kernel`
/// takes: 100000 leaves in all, `MAX_THOUGHT_LEAVES` per call (the bound of
/// one reasoning state).
const KERNEL_LEAVES: usize = 100_000;

/// The `decide/entry_trust` group (MEDULLA-DECISION-ENGINE-R132.6, R178.1): a
/// route question under a policy that knows one source, carrying a verified
/// entry and an unverified entry that contradicts it. The fixture asserts the
/// sealed record holds the verified fact as its entry premise and the
/// unverified entry in `refused_entries`, so the group times the admitted path.
// spec: MEDULLA-DECISION-ENGINE-R132.6
fn bench_entry_trust(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_types::decision::entry_types::{EntryTrust, RetrievedEntry, TrustTier};
    const SOURCE: &str = "sha256:bench-source";
    let (h, mut asked) = rt.block_on(fixture(QuestionKind::Route));
    let mut policy = DecisionPolicy::engine_default();
    policy
        .entry_trust
        .known_source_digests
        .insert(SOURCE.to_string());
    asked.policy = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-bench-entry-trust", &policy),
    };
    let entry = |id: &str, tier: TrustTier, value: &str| RetrievedEntry {
        entry_id: id.to_string(),
        subject: "urn:agent:planner".to_string(),
        fact_key: "max_tools".to_string(),
        value: value.to_string(),
        trust: EntryTrust {
            tier,
            source_digest: SOURCE.to_string(),
            observed_at: 10,
        },
    };
    asked.params = BoundedVec::new(vec![TypedParam {
        name: "retrieved_entries".to_string(),
        value: TypedValue::RetrievedEntries(
            BoundedVec::new(vec![
                entry("e-verified", TrustTier::Verified, "8"),
                entry("e-unverified", TrustTier::Unverified, "99"),
            ])
            .unwrap(),
        ),
    }])
    .unwrap();
    let batch: DecisionBatch =
        decode(decide(rt, &h, &asked)).expect("the entry-trust question is answered");
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.refused_entries.as_slice().len(), 1);
    assert_eq!(record.refused_entries.as_slice()[0].entry_id, "e-unverified");
    let group_name = "decide/entry_trust".to_string();
    let mut group = c.benchmark_group(format!("{group_name}/served"));
    group.bench_function("served", |b| b.iter(|| black_box(decide(rt, &h, &asked))));
    group.finish();
    report.insert(group_name, sampled_report(rt, &h, &asked));
}

/// Failed runs the `decide/failure_patterns` group seeds.
const FAILURE_RUNS: u64 = 64;

/// The `decide/failure_patterns` group (MEDULLA-DECISION-ENGINE-R701.6,
/// R178.1): the served `FailureIngest` journey over `FAILURE_RUNS` failed
/// `RunTrace` nodes of one signature, seeded through served `BatchUpdate`.
/// The fixture ingests once (one gap filed), then each sample is the repeat
/// ingest that plans nothing, so the group times the steady-state read and
/// cluster path. Budgets: `group_budgets` in `benches/baselines/decide.json`.
// spec: MEDULLA-DECISION-ENGINE-R701.6
fn bench_failure_patterns(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_decision_types::failure_patterns::{FailureIngestResult, FailureOp};
    let h = rt.block_on(Harness::with_isolation(ServerState::test_isolation("decider")));
    let call = |method: Method| -> Response {
        rt.block_on(Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &h.state,
                Request {
                    id: 701,
                    graph: "__commons__".into(),
                    auth_token: String::new(),
                    agent_id: Some("decider".into()),
                    method,
                    trace_id: None,
                },
                VerifiedRequestContext::verified_for_test_with_scopes("decider", TENANT, &["*"]),
            ),
        ))
    };
    let mut operations = vec![serde_json::json!({"op": "upsert_node", "id": "search",
                                                  "properties": {"kind": "tool"}})];
    for index in 0..FAILURE_RUNS {
        let run = format!("run-{index}");
        let plan = format!("{run}-plan");
        let step = format!("{run}-call");
        operations.push(serde_json::json!({"op": "upsert_node", "id": run, "properties": {
            "node_type": "RunTrace",
            "run_trace_id": format!("trace-{index}"),
            "terminationReason": "max_steps",
            "endMs": 1_700_000_000_000u64 + index,
        }}));
        operations.push(serde_json::json!({"op": "upsert_node", "id": plan,
                                           "properties": {"kind": "plan"}}));
        operations.push(serde_json::json!({"op": "upsert_node", "id": step,
                                           "properties": {"kind": "tool_call"}}));
        for (source, target, relationship) in [
            (run.as_str(), plan.as_str(), "hasStep"),
            (run.as_str(), step.as_str(), "hasStep"),
            (plan.as_str(), step.as_str(), "precedes"),
            (step.as_str(), "search", "usesTool"),
        ] {
            operations.push(serde_json::json!({"op": "upsert_edge", "source": source,
                "target": target, "properties": {"relationship": relationship}}));
        }
    }
    let seeded = call(Method::BatchUpdate {
        operations_msgpack: rmp_serde::to_vec_named(&operations).expect("failure bench batch"),
    });
    assert!(seeded.refusal_text().is_none(), "{:?}", seeded.refusal_text());
    let ingest = || {
        call(Method::FailureIngest {
            op: Box::new(FailureOp::Ingest {
                graph: "__commons__".into(),
                since_ms: None,
            }),
        })
    };
    let first: FailureIngestResult = decode(ingest()).expect("the first ingest is answered");
    assert_eq!(first.filed.len(), 1);
    let group_name = "decide/failure_patterns".to_string();
    let mut group = c.benchmark_group(format!("{group_name}/served"));
    group.bench_function("served", |b| b.iter(|| black_box(ingest())));
    group.finish();
    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(ingest());
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// A maximal frontier: `MAX_THOUGHT_LEAVES` leaves over four depths.
fn widest_frontier() -> ReasoningState {
    let nodes = (0..MAX_THOUGHT_LEAVES as u32)
        .map(|id| ThoughtNode {
            id,
            parent: (id > 0).then_some(0),
            depth: 1 + id % 4,
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: i64::from(id.wrapping_mul(2_654_435_761) % 1_000) << 22,
            },
            tokens: 16,
            state_digest: format!("sha256:{:064x}", id % 48),
        })
        .collect();
    ReasoningState::new(
        nodes,
        BudgetLedger {
            nodes_used: 100,
            tokens_used: 1_600,
            depth_reached: 4,
        },
    )
    .expect("a frontier of exactly the bound")
}

// spec: MEDULLA-DECISION-ENGINE-R750.753
/// The `decide/thought_search_kernel` group (MEDULLA-DECISION-ENGINE-R750.753):
/// `choose_expansion` alone, over 100000 leaves (the maximal frontier,
/// repeated), for every strategy. Budget: `group_budgets` in
/// `benches/baselines/decide.json`.
fn bench_thought_search_kernel(
    c: &mut Criterion,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let state = widest_frontier();
    let knobs = ThoughtSearchKnobs::default();
    let strategies = [
        ReasoningStrategy::Chain,
        ReasoningStrategy::Tree,
        ReasoningStrategy::Graph,
        ReasoningStrategy::Planning,
    ];
    let sweep = || {
        for _ in 0..(KERNEL_LEAVES / MAX_THOUGHT_LEAVES) {
            for strategy in strategies {
                black_box(choose_expansion(
                    black_box(&state),
                    strategy,
                    black_box(&knobs),
                ));
            }
        }
    };
    let group_name = "decide/thought_search_kernel".to_string();
    let mut group = c.benchmark_group(&group_name);
    group.bench_function("choose_expansion", |b| b.iter(sweep));
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            sweep();
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name,
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// Trajectory steps the reward kernel group sweeps per sample.
const REWARD_KERNEL_STEPS: usize = 10_000;
/// Samples per drift window of the served drift group.
const DRIFT_WINDOW: usize = 2048;

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

fn trajectory(steps: usize) -> RewardDecompositionRequest {
    let one = 1i64 << 32;
    RewardDecompositionRequest::new(
        "bench-trajectory".to_string(),
        q32(one),
        (0..steps)
            .map(|i| StepSignal {
                value_estimate: q32(one / 2 * (i as i64 - i as i64 / 10) / steps as i64 + 1),
                cache_hit: i % 5 == 0,
            })
            .collect(),
        Vec::new(),
        false,
        None,
    )
    .unwrap()
}

fn drift_window(len: usize, offset: i64) -> DriftWindow {
    DriftWindow::new(
        (0..len)
            .map(|i| q32(((i as i64 * 7919) % 4096) * (1 << 20) + offset))
            .collect(),
    )
    .unwrap()
}

/// One sampled group: Criterion times `work`, and the budget report takes
/// `REPORT_SAMPLES` timings of it.
fn bench_group(
    c: &mut Criterion,
    report: &mut BTreeMap<String, serde_json::Value>,
    group_name: &str,
    work: &mut dyn FnMut(),
) {
    let mut group = c.benchmark_group(group_name);
    group.bench_function("run", |b| b.iter(&mut *work));
    group.finish();
    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed()
        })
        .collect();
    samples.sort();
    report.insert(
        group_name.to_string(),
        serde_json::json!({
            "p50_ms": percentile_ms(&samples, 0.50),
            "p99_ms": percentile_ms(&samples, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }),
    );
}

/// Claims the `decide/belief_revision` group holds, each with evidence already
/// revised once.
const BELIEF_CLAIMS: usize = 64;
/// Evidence items each belief-bench claim starts with.
const BELIEF_EVIDENCE_PER_CLAIM: usize = 8;
/// Graph the belief-bench passes run on.
const BELIEF_GRAPH: &str = "bench-belief";

// spec: MEDULLA-DECISION-ENGINE-R700.6
/// The `decide/belief_revision` group (MEDULLA-DECISION-ENGINE-R700.6, R178.1):
/// 64 claims of 8 evidence items each have been revised once; one sample
/// records one new evidence item on the next claim and serves the
/// `BeliefRevise` pass that revises it. The fixture asserts the pass visited
/// exactly that claim, so the group times the revision and not a refusal.
/// Budgets: `group_budgets` in `benches/baselines/decide.json`.
fn bench_belief_revision(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let h = rt.block_on(Harness::new());
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("bench", TENANT);
    let record = |claim: usize, item: usize| {
        let response = claims(
            rt,
            &h,
            ClaimOp::RecordEvidence {
                evidence_id: format!("belief-e{claim}-{item}"),
                claim_id: format!("belief-claim-{claim}"),
                channel: Some(EvidenceChannel::Outcome),
                supports: item % 3 != 0,
                weight_ppm: 50_000,
                source_digest: format!("sha256:{claim:032x}{item:032x}"),
                stances: BTreeMap::new(),
                feed: None,
            },
        );
        assert!(
            response.error.is_none(),
            "belief bench fixture: {response:?}"
        );
    };
    let pass = |ids: Vec<String>| {
        let response = rt.block_on(medulla_h_decide::decide::handle_belief_revise(
            &h.state,
            2,
            &verified,
            BeliefReviseRequest {
                graph: BELIEF_GRAPH.to_string(),
                claim_ids: ids,
                texts: BTreeMap::new(),
                premises: Vec::new(),
                advance: Vec::new(),
            },
        ));
        assert!(
            response.error.is_none(),
            "belief bench fixture: {response:?}"
        );
        response
    };
    for claim in 0..BELIEF_CLAIMS {
        let response = claims(
            rt,
            &h,
            ClaimOp::Propose {
                claim_id: format!("belief-claim-{claim}"),
            },
        );
        assert!(
            response.error.is_none(),
            "belief bench fixture: {response:?}"
        );
        for item in 0..BELIEF_EVIDENCE_PER_CLAIM {
            record(claim, item);
        }
    }
    pass(
        (0..BELIEF_CLAIMS)
            .map(|n| format!("belief-claim-{n}"))
            .collect(),
    );
    let mut round = 0usize;
    bench_group(c, report, "decide/belief_revision", &mut || {
        let claim = round % BELIEF_CLAIMS;
        record(claim, BELIEF_EVIDENCE_PER_CLAIM + round);
        round += 1;
        black_box(pass(vec![format!("belief-claim-{claim}")]));
    });
}

/// Skills the `decide/skill_evolution` group publishes, one library block.
const EVOLUTION_SKILLS: usize = 256;

// spec: MEDULLA-DECISION-ENGINE-R703.6
/// The `decide/skill_evolution` group (MEDULLA-DECISION-ENGINE-R703.6, R178.1):
/// one served `Decide` over a library block of 256 skills with requested
/// skills and used terms, answering gaps and neologisms. The fixture asserts
/// the answer names a gap and a neologism, so the group times the proposals and
/// not a refusal. Budgets: `group_budgets` in `benches/baselines/decide.json`.
fn bench_skill_evolution(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let (h, asked) = rt.block_on(fixture(QuestionKind::SkillEvolution));
    for n in 0..EVOLUTION_SKILLS {
        h.publish_test_component(
            &format!("skill-{n}"),
            AgentComponentKind::Skill,
            "bench skill",
            None::<&()>,
            None,
        )
        .unwrap();
    }
    let list = |values: &[&str]| {
        TypedValue::IriList(
            BoundedVec::new(values.iter().map(|v| (*v).to_string()).collect()).unwrap(),
        )
    };
    let mut ask = asked.clone();
    ask.question.kind = QuestionKind::SkillEvolution;
    ask.candidates = CandidateSource::AgentLibrary {
        scope: LibraryCandidateScope {
            kinds: BoundedVec::new(vec![AgentComponentKind::Skill]).unwrap(),
            classification_under: None,
        },
    };
    ask.params = BoundedVec::new(vec![
        TypedParam {
            name: PARAM_REQUESTED_SKILLS.to_string(),
            value: list(&["skill-0", "skill-missing", "skill-missing"]),
        },
        TypedParam {
            name: PARAM_USED_TERMS.to_string(),
            value: list(&["frobnicate", "frobnicate", "frobnicate"]),
        },
    ])
    .unwrap();
    let answer = serde_json::to_string(&decide(rt, &h, &ask).result).unwrap();
    assert!(
        answer.contains("gap:skill-missing") && answer.contains("neologism:frobnicate"),
        "skill_evolution fixture must answer proposals: {answer}"
    );
    bench_group(c, report, "decide/skill_evolution", &mut || {
        black_box(decide(rt, &h, &ask));
    });
}

// spec: MEDULLA-DECISION-ENGINE-R654.6
/// The `decide/skill_evolution_paired_bootstrap` group (MEDULLA-DECISION-ENGINE-R654.6, R178.1):
/// `paired_rollout_evidence` over 200 pairs per arm, the bootstrap lower bound of a skill
/// revision's rollout set. The default budgets of `benches/baselines/decide.json` apply.
fn bench_paired_rollouts(c: &mut Criterion, report: &mut BTreeMap<String, serde_json::Value>) {
    use medulla_decision_stats::decision::promotion::paired_rollout_evidence;
    use medulla_decision_types::decision::skill_evolution::{
        Rollout, RolloutSet, ROLLOUT_SET_SCHEMA_VERSION,
    };
    let set = RolloutSet {
        schema_version: ROLLOUT_SET_SCHEMA_VERSION,
        program_ref: "skill-program".to_string(),
        challenger_ref: "revision:skill-v1".to_string(),
        champion_ref: Some("revision:skill-v0".to_string()),
        rollouts: BoundedVec::new(
            (0..200u64)
                .map(|index| Rollout {
                    seed: index % 5,
                    task_id: format!("task-{index}"),
                    challenger_score_ppm: Some(600_000 + (index as u32 % 7) * 20_000),
                    champion_score_ppm: Some(600_000 + (index as u32 % 3) * 10_000),
                    trace_ref: format!("trace-{index}"),
                })
                .collect(),
        )
        .unwrap(),
    };
    let policy = DecisionPolicy::engine_default();
    let evidence = || {
        black_box(paired_rollout_evidence(black_box(&set), black_box(&policy)));
    };
    assert_eq!(paired_rollout_evidence(&set, &policy).case_count, 200);
    let mut group = c.benchmark_group("decide/skill_evolution_paired_bootstrap");
    group.bench_function("evidence", |b| b.iter(evidence));
    group.finish();
    report.insert(
        "decide/skill_evolution_paired_bootstrap".to_string(),
        report_entry(REPORT_SAMPLES, evidence),
    );
}

/// Facts the `decide/memory_scaling` group loads at its smallest size (N).
const MEMORY_SCALING_BASE: usize = 100_000;
/// Recalls timed at each corpus size.
const MEMORY_SCALING_RECALLS: usize = 50;

// spec: MEDULLA-DECISION-ENGINE-R154.2
/// The `decide/memory_scaling` group (MEDULLA-DECISION-ENGINE-R154.2, R178.1): N, 2N and 4N
/// facts are stored through the served `MemoryRemember`, and the p95 of the served
/// `MemoryRecall` is measured at each size. The `scaling` samples in the report let
/// `cargo xtask decide-budgets` hold the p95 growth per doubling at `scaling_limits` of
/// `benches/baselines/decide.json` (2.2).
fn bench_memory_scaling(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    use medulla_work_types::memory::{
        MemoryKind, MemoryRecallRequest, MemoryRecord, MemoryRememberRequest, MemoryTier,
        MEMORY_RECORD_SCHEMA_VERSION,
    };
    let h = rt.block_on(Harness::new());
    let remember = |n: usize| {
        let needle = n % 1000 == 0;
        let record = MemoryRecord {
            memory_id: medulla_types::contract::ResourceId::new(format!("m:{n}")).unwrap(),
            tenant: medulla_types::contract::TenantId::new(TENANT).unwrap(),
            kind: MemoryKind::Fact,
            tier: MemoryTier::Semantic,
            text: if needle {
                format!("needle fact {n}")
            } else {
                format!("filler fact {n}")
            },
            provenance_key: None,
            session_id: None,
            timestamp_ms: 10,
            topics: Vec::new(),
            pattern_key: None,
            importance: 1.0,
            access_count: 0,
            last_access_ms: 0,
            last_decay_ms: 0,
            forgotten: false,
            legal_hold: false,
            expires_at_ms: None,
            support_count: 0,
            token_estimate: 10,
            attachments: Vec::new(),
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
        };
        let response = rt.block_on(dispatch(
            &h,
            Method::MemoryRemember {
                request: Box::new(MemoryRememberRequest {
                    record,
                    embedding: Vec::new(),
                }),
            },
        ));
        assert!(
            response.error.is_none(),
            "memory_scaling fixture: {response:?}"
        );
    };
    let recall = || {
        let response = rt.block_on(dispatch(
            &h,
            Method::MemoryRecall {
                request: Box::new(MemoryRecallRequest {
                    query_embedding: Vec::new(),
                    keywords: vec!["needle".to_string()],
                    tiers: Vec::new(),
                    scope: None,
                    as_of_ms: None,
                    budget: 100,
                    prime_session: None,
                    include_plan: false,
                    include_media: false,
                }),
            },
        ));
        assert!(
            response.error.is_none(),
            "memory_scaling fixture: {response:?}"
        );
        response
    };
    let before = high_water_mib();
    let mut loaded = 0usize;
    let mut scaling = Vec::new();
    let mut last: Vec<Duration> = Vec::new();
    for size in [
        MEMORY_SCALING_BASE,
        MEMORY_SCALING_BASE * 2,
        MEMORY_SCALING_BASE * 4,
    ] {
        while loaded < size {
            remember(loaded);
            loaded += 1;
        }
        let mut samples: Vec<Duration> = (0..MEMORY_SCALING_RECALLS)
            .map(|_| {
                let start = Instant::now();
                black_box(recall());
                start.elapsed()
            })
            .collect();
        samples.sort();
        scaling.push(serde_json::json!({
            "size": size,
            "p95_ms": percentile_ms(&samples, 0.95),
        }));
        last = samples;
    }
    let mut group = c.benchmark_group("decide/memory_scaling");
    group.sample_size(10);
    group.bench_function("run", |b| b.iter(|| black_box(recall())));
    group.finish();
    report.insert(
        "decide/memory_scaling".to_string(),
        serde_json::json!({
            "p50_ms": percentile_ms(&last, 0.50),
            "p99_ms": percentile_ms(&last, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
            "scaling": scaling,
        }),
    );
}

/// A kernel family request: one typed body parameter and one declared candidate.
fn kernel_ask(
    asked: &DecideRequest,
    kind: QuestionKind,
    param: &str,
    value: TypedValue,
    candidate: &str,
) -> DecideRequest {
    kernel_ask_many(asked, kind, param, value, vec![candidate.to_string()])
}

/// `asked` re-pointed at `kind` with one typed body parameter and the
/// declared candidates that name its subject.
fn kernel_ask_many(
    asked: &DecideRequest,
    kind: QuestionKind,
    param: &str,
    value: TypedValue,
    candidate_ids: Vec<String>,
) -> DecideRequest {
    let mut ask = asked.clone();
    ask.question.kind = kind;
    ask.question.question_id = format!("bench.{param}");
    ask.candidates = CandidateSource::Declared {
        options: BoundedVec::new(
            candidate_ids
                .into_iter()
                .map(|option_id| DeclaredOption {
                    option_id,
                    classification: BoundedVec::default(),
                    numbers: BoundedVec::default(),
                    texts: BoundedVec::default(),
                })
                .collect(),
        )
        .unwrap(),
    };
    ask.params = BoundedVec::new(vec![TypedParam {
        name: param.to_string(),
        value,
    }])
    .unwrap();
    ask
}

// spec: MEDULLA-DECISION-ENGINE-R751.753
// spec: MEDULLA-DECISION-ENGINE-R752.753
/// The served kernel groups (MEDULLA-DECISION-ENGINE-R751.753, R752.753):
/// `decide/reward_decomposition` serves a 1024-step trajectory through
/// `Decide`; `decide/population_drift` serves two 2048-sample windows. The
/// `_kernel` groups time `decompose` over 10000 steps and `wasserstein1` over
/// two 2048-sample windows. Budgets: `group_budgets` in `benches/baselines/decide.json`.
fn bench_served_kernels(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let (h, asked) = rt.block_on(fixture(QuestionKind::RewardDecomposition));
    let served_reward = kernel_ask(
        &asked,
        QuestionKind::RewardDecomposition,
        "reward_decomposition",
        TypedValue::RewardDecomposition(Box::new(trajectory(1024))),
        "bench-trajectory",
    );
    bench_group(c, report, "decide/reward_decomposition", &mut || {
        let response = black_box(decide(rt, &h, &served_reward));
        assert!(
            response.error.is_none(),
            "the served trajectory must be answered, not refused"
        );
    });
    let knobs = RewardKnobs::default();
    let full = trajectory(MAX_REWARD_STEPS);
    let rest = trajectory(REWARD_KERNEL_STEPS % MAX_REWARD_STEPS);
    bench_group(c, report, "decide/reward_decomposition_kernel", &mut || {
        for _ in 0..(REWARD_KERNEL_STEPS / MAX_REWARD_STEPS) {
            black_box(decompose(black_box(&full), &knobs).unwrap());
        }
        black_box(decompose(black_box(&rest), &knobs).unwrap());
    });

    let (h, asked) = rt.block_on(fixture(QuestionKind::PopulationDrift));
    let reference = drift_window(DRIFT_WINDOW, 0);
    let current = drift_window(DRIFT_WINDOW, 1 << 24);
    let served_drift = kernel_ask(
        &asked,
        QuestionKind::PopulationDrift,
        "population_drift",
        TypedValue::PopulationDrift(Box::new(DriftRequest {
            reference: reference.clone(),
            current: current.clone(),
        })),
        "scores",
    );
    bench_group(c, report, "decide/population_drift", &mut || {
        let response = black_box(decide(rt, &h, &served_drift));
        assert!(
            response.error.is_none(),
            "the served drift windows must be answered, not refused"
        );
    });
    bench_group(c, report, "decide/population_drift_kernel", &mut || {
        black_box(
            wasserstein1(
                black_box(reference.as_slice()),
                black_box(current.as_slice()),
            )
            .unwrap(),
        );
    });

    // spec: MEDULLA-DECISION-ENGINE-R751.754
    let (h, asked) = rt.block_on(fixture(QuestionKind::CurriculumPromotion));
    let served_curriculum = kernel_ask(
        &asked,
        QuestionKind::CurriculumPromotion,
        "curriculum_promotion",
        TypedValue::CurriculumPromotion(Box::new(CurriculumRequest {
            judged_stage: CurriculumStage {
                stage_id: "bench-stage".to_string(),
                horizon_steps: 8,
                window_runs: 20,
            },
            policy: PromotionPolicy {
                promote_threshold: q32(3 << 30),
                next_stage_id: "bench-next".to_string(),
            },
        })),
        "bench-stage",
    );
    bench_group(c, report, "decide/curriculum_promotion", &mut || {
        let response = black_box(decide(rt, &h, &served_curriculum));
        assert!(
            response.error.is_none(),
            "the served stage must be answered, not refused"
        );
    });

    // spec: MEDULLA-DECISION-ENGINE-R752.755
    let (h, asked) = rt.block_on(fixture(QuestionKind::AutonomyVerdict));
    let served_autonomy = kernel_ask(
        &asked,
        QuestionKind::AutonomyVerdict,
        "autonomy_verdict",
        TypedValue::AutonomyVerdict(Box::new(AutonomyRequest {
            irreversibility: IrreversibilityClass::Reversible,
        })),
        "bench.action",
    );
    bench_group(c, report, "decide/autonomy_verdict", &mut || {
        let response = black_box(decide(rt, &h, &served_autonomy));
        assert!(
            response.error.is_none(),
            "the served action class must be answered, not refused"
        );
    });
}

/// Trajectories the served `decide/trajectory_join` group joins
/// (MEDULLA-DECISION-ENGINE-R753.753).
const SERVED_JOIN_TRAJECTORIES: usize = 64;

/// Steps the served `decide/step_verification` group verifies.
const SERVED_VERIFY_STEPS: usize = 256;

/// `entries` competing trajectories: observation sources with spread scores
/// and one proof source, so the join has a single winner.
fn join_entries(entries: usize) -> Vec<TrajectoryEntry> {
    (0..entries)
        .map(|n| {
            let evidence = if n == entries / 2 {
                EvidenceClass::Proof
            } else {
                EvidenceClass::Observation
            };
            TrajectoryEntry::new(
                format!("t{n:04}"),
                evidence,
                QuantisedValue {
                    scale: QuantScaleTag::Q32,
                    value: ((n as i64 * 7919) % 1_000) << 22,
                },
                "cd".repeat(32),
            )
            .expect("a well-formed trajectory entry")
        })
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R753.753
/// The `decide/trajectory_join` and `decide/step_verification` groups
/// (MEDULLA-DECISION-ENGINE-R753.753): a valid 64-trajectory join and a valid
/// 256-step verification through the served `Decide` path, so each group times
/// the family's answer and not its refusal of the generic fixture request.
fn bench_trajectory_served(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let (h, asked) = rt.block_on(fixture(QuestionKind::TrajectoryJoin));
    let entries = join_entries(SERVED_JOIN_TRAJECTORIES);
    let mut ids: Vec<String> = entries.iter().map(|e| e.trajectory_id.clone()).collect();
    ids.sort();
    let joined = kernel_ask_many(
        &asked,
        QuestionKind::TrajectoryJoin,
        "trajectory_join",
        TypedValue::TrajectoryJoin(Box::new(JoinRequest {
            entries: BoundedVec::new(entries).unwrap(),
        })),
        ids,
    );
    let response = decide(rt, &h, &joined);
    assert!(
        response.error.is_none(),
        "trajectory_join fixture: {response:?}"
    );
    bench_group(c, report, "decide/trajectory_join", &mut || {
        black_box(decide(rt, &h, &joined));
    });

    let steps: Vec<VerifyStep> = (0..SERVED_VERIFY_STEPS)
        .map(|n| VerifyStep {
            step_id: format!("s{n:04}"),
            digest: format!("{:02x}", n % 251 + 1).repeat(32),
            evidence_ids: Vec::new(),
        })
        .collect();
    let checks: Vec<StepCheck> = (0..SERVED_VERIFY_STEPS)
        .map(|n| StepCheck {
            step_id: format!("s{n:04}"),
            passed: true,
            check_digest: format!("{:02x}", n % 127 + 128).repeat(32),
            interval: None,
        })
        .collect();
    let step_digests: Vec<&str> = steps.iter().map(|step| step.digest.as_str()).collect();
    let trace_digest = medulla_decision_types::decision::digest::step_trace_digest(&step_digests);
    let verified = kernel_ask(
        &asked,
        QuestionKind::StepVerification,
        "step_verification",
        TypedValue::StepVerification(Box::new(VerifyRequest {
            steps: BoundedVec::new(steps).unwrap(),
            checks: BoundedVec::new(checks).unwrap(),
            attempts: 0,
            trace_digest,
        })),
        "trace-1",
    );
    let response = decide(rt, &h, &verified);
    assert!(
        response.error.is_none(),
        "step_verification fixture: {response:?}"
    );
    bench_group(c, report, "decide/step_verification", &mut || {
        black_box(decide(rt, &h, &verified));
    });
}

// spec: MEDULLA-DECISION-ENGINE-R753.753
/// The `decide/trajectory_join_kernel` group (MEDULLA-DECISION-ENGINE-R753.753):
/// `join` alone over `MAX_TRAJECTORIES` (256) trajectories.
fn bench_trajectory_join_kernel(
    c: &mut Criterion,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let entries = join_entries(MAX_TRAJECTORIES);
    bench_group(c, report, "decide/trajectory_join_kernel", &mut || {
        black_box(join(black_box(&entries)).expect("a valid join"));
    });
}

const BENCH_PROFILE: &str = "bench-profile";
const BENCH_TASK: &str = "urn:task:bench";
const BENCH_SWARM: &str = "https://knuckles-team.github.io/ontology/swarm#";

/// A harness with a model profile and task facts, and one sealed
/// `harness_config` record decided over three declared tools.
async fn harness_config_record() -> (Harness, StatisticalDecisionRecord) {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    fn measured<T>(value: T, run: &str) -> MeasuredFact<T> {
        MeasuredFact {
            value,
            evaluation_run_ref: ObservationRef {
                evaluation_id: run.to_string(),
                digest: format!("sha256:{run}"),
            },
        }
    }
    h.publish_model_profile(
        TENANT,
        BENCH_PROFILE,
        8192,
        Some(measured(ShellProficiency::High, "eval:shell")),
        Some(measured(StoppingBehaviour::Balanced, "eval:stopping")),
    )
    .unwrap();
    let mut task = serde_json::Map::new();
    task.insert(format!("{BENCH_SWARM}shellCentric"), false.into());
    task.insert(
        format!("{BENCH_SWARM}taskTokenUpperBound"),
        10_000u64.into(),
    );
    h.seed_node("__commons__", BENCH_TASK, task.into()).await;
    let (schema_pin, _) = publish_route_schema(&h);
    let option = |id: &str, capability: &str, tokens: i64| DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec![capability.to_string()]).unwrap(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "schema_tokens".to_string(),
            q32: tokens,
        }])
        .unwrap(),
        texts: BoundedVec::default(),
    };
    let param = |name: &str, value: TypedValue| TypedParam {
        name: name.to_string(),
        value,
    };
    let asked = DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "bench.harness_config_replay".to_string(),
            kind: QuestionKind::HarnessConfig,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![
                option("tool-a", "eg:cap/a", 100),
                option("tool-b", "eg:cap/b", 50),
                option("tool-c", "eg:cap/a", 400),
            ])
            .unwrap(),
        },
        feature_schema: schema_pin,
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            param("graph", TypedValue::Text("__commons__".to_string())),
            param(
                "model_profile_ref",
                TypedValue::Iri(BENCH_PROFILE.to_string()),
            ),
            param("priority", TypedValue::Text("interactive".to_string())),
            param(
                "required_capabilities",
                TypedValue::IriList(
                    BoundedVec::new(vec!["eg:cap/a".to_string(), "eg:cap/b".to_string()]).unwrap(),
                ),
            ),
            param("task_class", TypedValue::Iri(BENCH_TASK.to_string())),
            param("usable_context_budget_tokens", TypedValue::Int(1000)),
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    };
    let response = dispatch(
        &h,
        Method::Decide {
            request: Box::new(asked),
        },
    )
    .await;
    let batch: DecisionBatch = decode(response).expect("the harness_config bench decision");
    let record = batch.records.as_slice()[0].clone();
    (h, record)
}

/// A changed record with its ids and digests recomputed.
fn reseal(mut record: StatisticalDecisionRecord) -> StatisticalDecisionRecord {
    record.inputs_digest = medulla_query_types::solve_scalar::digest_text(
        medulla_h_decide::decide::stat_decide::STATISTICAL_INPUTS_DOMAIN,
        &(&record.question, &record.inputs),
    );
    record.record_id = format!(
        "decision:{}",
        record.inputs_digest.trim_start_matches("sha256:")
    );
    record.record_digest =
        medulla_decision_types::decision::digest::statistical_record_digest(&record);
    record
}

/// The widest evidence the default limits admit: 4096 items.
fn widest_evidence() -> HarnessConfigEvaluationEvidence {
    let decided =
        |value: serde_json::Value| serde_json::json!({"decision": "decided", "value": value});
    let q32 = |n: u64| serde_json::json!({"numerator": n, "denominator": 4294967296u64});
    let tools: Vec<String> = (0..256).map(|i| format!("tool-{i:04}")).collect();
    let declared: Vec<serde_json::Value> = (0..64)
        .map(|i| {
            serde_json::json!({
                "option_id": format!("tool-{i:04}"),
                "classification": [format!("eg:cap/{i}")]
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "model_profile": {"component_id": "profile", "kind": "model_profile",
            "definition_digest": "sha256:profile"},
        "request": {"model_profile_ref": "profile", "task_class": "task",
            "usable_context_budget_tokens": 10, "priority": "interactive"},
        "answer": {
            "context_policy": decided(serde_json::json!("none")),
            "soft_threshold": decided(q32(1)),
            "hard_threshold": decided(q32(2)),
            "recent_window_fraction": decided(q32(1)),
            "plan_scaffold": decided(serde_json::json!("on")),
            "action_space": decided(serde_json::json!("tool_set")),
            "tool_subset": decided(serde_json::json!(tools))
        },
        "declared_options": declared
    }))
    .expect("the widest evidence fixture decodes")
}

// spec: MEDULLA-DECISION-ENGINE-R810.6
/// The `decide/program` group (MEDULLA-DECISION-ENGINE-R810.6): one program compiled from a
/// 200-example corpus (140 train, 60 test) with `bootstrap_few_shot_with_random_search`. Budget
/// in `group_budgets`: wall time at most 30 s, resident growth at most 512 MiB. A benchmark
/// never gates a PR.
fn bench_program_compile(c: &mut Criterion, report: &mut BTreeMap<String, serde_json::Value>) {
    use medulla_program::{NativeCompiler, OptimizerKind};
    let request = program_job_support::sized_program_request(
        "bench",
        Vec::new(),
        (140, 60),
        OptimizerKind::BootstrapFewShotWithRandomSearch,
    );
    let compile = || {
        black_box(NativeCompiler::compile(black_box(&request)).expect("the bench corpus compiles"));
    };
    compile();
    let mut group = c.benchmark_group("decide/program");
    group.sample_size(10);
    group.bench_function("compile", |b| b.iter(compile));
    group.finish();
    report.insert(
        "decide/program".to_string(),
        report_entry(REPORT_SAMPLES, compile),
    );
}

/// One report entry from `samples` timings of `work`.
fn report_entry(samples: usize, mut work: impl FnMut()) -> serde_json::Value {
    let before = high_water_mib();
    let mut timings: Vec<Duration> = (0..samples)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed()
        })
        .collect();
    timings.sort();
    serde_json::json!({
        "p50_ms": percentile_ms(&timings, 0.50),
        "p99_ms": percentile_ms(&timings, 0.99),
        "peak_mib": (high_water_mib() - before).max(0.0),
    })
}

/// Steps of the workflow the `decide/process_plan` group plans: the most a task plan holds
/// (`MAX_TASKS`, 64) less the start and end anchors.
const PROCESS_PLAN_STEPS: usize = 60;

// spec: MEDULLA-CQ-ORCH-R704.8
/// The `decide/process_plan` group (MEDULLA-CQ-ORCH-R704.8): the largest workflow a plan can
/// hold, a chain of tool steps, is planned by `plan_from_workflow` and sealed by
/// `plan_explicit_tasks`. Budget in `group_budgets`: p99 at most 30 ms.
fn bench_process_plan(c: &mut Criterion, report: &mut BTreeMap<String, serde_json::Value>) {
    use medulla_compute::process_plan::plan::plan_from_workflow;
    use medulla_decision_types::decision::process_plan::{
        BindingSource, BindingTarget, StepBinding,
    };
    use medulla_decision_types::decision::task_plan::plan_explicit_tasks;
    use medulla_types::process_import::{
        CanonicalFlow, CanonicalStep, CanonicalWorkflow, ProcessSourceSystem, SourceTrace,
        StepKind as ProcessStepKind,
    };
    let step = |id: String, kind: ProcessStepKind| CanonicalStep {
        id: id.clone(),
        kind,
        gate: None,
        label: id,
        role: None,
        reference: None,
    };
    let mut steps = vec![step("s0".to_string(), ProcessStepKind::Start)];
    let mut flows = Vec::new();
    let mut bindings = Vec::new();
    for n in 1..=PROCESS_PLAN_STEPS {
        steps.push(step(format!("s{n}"), ProcessStepKind::Agent));
        flows.push(CanonicalFlow {
            source: format!("s{}", n - 1),
            target: format!("s{n}"),
            condition: None,
            default: false,
        });
        bindings.push(StepBinding {
            step_id: format!("s{n}"),
            target: BindingTarget::Tool {
                server: "bench-server".to_string(),
                tool: format!("tool-{n}"),
                contract_iri: format!("urn:contract:{n}"),
            },
            source: BindingSource::Explicit,
        });
    }
    steps.push(step("end".to_string(), ProcessStepKind::End));
    flows.push(CanonicalFlow {
        source: format!("s{PROCESS_PLAN_STEPS}"),
        target: "end".to_string(),
        condition: None,
        default: false,
    });
    let workflow = CanonicalWorkflow {
        schema_version: 1,
        schema: "medulla.process.canonical/v1".to_string(),
        name: "bench".to_string(),
        steps,
        flows,
        digest: "bench".to_string(),
        source: SourceTrace {
            schema_version: 1,
            system: ProcessSourceSystem::Bpmn20,
            model_id: "bench".to_string(),
            elements: Default::default(),
        },
        annex: Vec::new(),
        losses: Vec::new(),
    };
    let no_callee =
        |_: &str| -> Option<medulla_compute::process_plan::plan::Callee> { None };
    let plan = || {
        let drafted =
            plan_from_workflow(black_box(&workflow), &bindings, &no_callee, 4, 86_400)
                .expect("the bench workflow plans");
        black_box(plan_explicit_tasks(&drafted.proposal).expect("the proposal seals"));
    };
    plan();
    let mut group = c.benchmark_group("decide/process_plan");
    group.bench_function("plan", |b| b.iter(plan));
    group.finish();
    report.insert(
        "decide/process_plan".to_string(),
        report_entry(REPORT_SAMPLES, plan),
    );
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.1.2
/// The `decide/harness_evidence_codec` group (MEDULLA-DECISION-ENGINE-R128.13.1.2):
/// encode plus digest of the widest admitted evidence, and the refusal path of
/// an over-limit frame as a second sub-benchmark. Budgets are `group_budgets`
/// in `benches/baselines/decide.json`: encode plus digest p99 at most 50 ms with
/// 64 MiB growth; the refusal path p99 at most 1 ms.
fn bench_harness_evidence_codec(
    c: &mut Criterion,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let evidence = widest_evidence();
    let limits = HarnessEvidenceLimits::from_policy(&DecisionPolicy::engine_default());
    let frame = serde_json::to_vec(&evidence).unwrap();
    assert!(
        limits.decode(&frame).is_ok(),
        "the widest evidence is admitted"
    );
    let oversize = vec![b'{'; limits.max_bytes as usize + 1];
    let encode = || {
        let frame = serde_json::to_vec(black_box(&evidence)).unwrap();
        black_box((frame.len(), evidence.digest()));
    };
    let refuse = || {
        black_box(limits.decode(black_box(&oversize)).unwrap_err());
    };
    let mut group = c.benchmark_group("decide/harness_evidence_codec");
    group.bench_function("encode_and_digest", |b| b.iter(encode));
    group.bench_function("refusal", |b| b.iter(refuse));
    group.finish();
    report.insert(
        "decide/harness_evidence_codec".to_string(),
        report_entry(REPORT_SAMPLES, encode),
    );
    report.insert(
        "decide/harness_evidence_codec_refusal".to_string(),
        report_entry(REPORT_SAMPLES, refuse),
    );
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.2.2
/// The `decide/harness_config_replay` group (MEDULLA-DECISION-ENGINE-R128.13.2.2):
/// a served `Decide` of a harness_config question, then its `DecisionLog` commit
/// (re-derivation and persistence) and read-back. Budget in `group_budgets`: p99
/// at most 500 ms, growth at most 128 MiB.
fn bench_harness_config_replay(
    c: &mut Criterion,
    rt: &Runtime,
    report: &mut BTreeMap<String, serde_json::Value>,
) {
    let (h, record) = rt.block_on(harness_config_record());
    let journey = |nonce: u64| {
        // Each pass commits a record with a distinct id, so persistence runs.
        let mut fresh = record.clone();
        fresh.created_at_ms += nonce;
        fresh.inputs.shortlist.now_ms += nonce;
        let fresh = reseal(fresh);
        let committed = rt.block_on(dispatch(
            &h,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::Commit {
                    record: Box::new(fresh.clone()),
                    evaluator: None,
                    approval: None,
                }),
            },
        ));
        assert!(
            committed.error.is_none(),
            "replay bench commit: {committed:?}"
        );
        let read = rt.block_on(dispatch(
            &h,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::Get {
                    tenant_id: TENANT.to_string(),
                    record_id: fresh.record_id.clone(),
                }),
            },
        ));
        black_box(read);
    };
    let nonce = std::cell::Cell::new(0u64);
    let next = || {
        nonce.set(nonce.get() + 1);
        nonce.get()
    };
    let mut group = c.benchmark_group("decide/harness_config_replay");
    group.bench_function("commit_and_read_back", |b| b.iter(|| journey(next())));
    group.finish();
    report.insert(
        "decide/harness_config_replay".to_string(),
        report_entry(REPORT_SAMPLES, || journey(next())),
    );
}

fn bench_decide(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let mut report: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    assert!(
        OWN_FIXTURE_FAMILIES
            .iter()
            .all(|kind| !FAMILIES.contains(kind)),
        "a family is benched by the generic fixture or its own, not both"
    );
    for kind in FAMILIES {
        let (h, asked) = rt.block_on(fixture(*kind));
        let group_name = format!("decide/{}", family_name(*kind));
        let mut group = c.benchmark_group(&group_name);
        group.bench_function("served", |b| {
            b.iter(|| black_box(decide(&rt, &h, &asked)));
        });
        group.finish();

        let before = high_water_mib();
        let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
            .map(|_| {
                let start = Instant::now();
                black_box(decide(&rt, &h, &asked));
                start.elapsed()
            })
            .collect();
        samples.sort();
        report.insert(
            group_name,
            serde_json::json!({
                "p50_ms": percentile_ms(&samples, 0.50),
                "p99_ms": percentile_ms(&samples, 0.99),
                "peak_mib": (high_water_mib() - before).max(0.0),
            }),
        );
    }
    bench_claims(c, &rt, &mut report);
    bench_writeback_queue(c, &rt, &mut report);
    bench_task_plan_bounds(c, &rt, &mut report);
    bench_tool_relevance_cap(c, &rt, &mut report);
    bench_program_lookup(c, &rt, &mut report);
    bench_capability_designation(c, "decide/capability_designation", 100_000, &mut report);
    bench_capability_designation(
        c,
        "decide/capability_designation/n10000",
        10_000,
        &mut report,
    );
    bench_retrieval_learning(c, &rt, &mut report);
    bench_load_set(c, &rt, &mut report);
    bench_usage_guidance(c, &rt, &mut report);
    bench_entry_trust(c, &rt, &mut report);
    bench_insight_selection(c, &rt, &mut report);
    bench_distilled_rank(c, &rt, &mut report);
    bench_topology_admit(c, &mut report);
    bench_agent_loop(c, &rt, &mut report);
    bench_failure_patterns(c, &rt, &mut report);
    bench_concentration_kernel(c, &mut report);
    bench_thought_search_kernel(c, &mut report);
    bench_served_kernels(c, &rt, &mut report);
    bench_belief_revision(c, &rt, &mut report);
    bench_skill_evolution(c, &rt, &mut report);
    bench_paired_rollouts(c, &mut report);
    bench_memory_scaling(c, &rt, &mut report);
    bench_trajectory_served(c, &rt, &mut report);
    bench_trajectory_join_kernel(c, &mut report);
    bench_harness_evidence_codec(c, &mut report);
    bench_process_plan(c, &mut report);
    bench_program_compile(c, &mut report);
    bench_harness_config_replay(c, &rt, &mut report);
    if let Ok(path) = std::env::var("DECIDE_BUDGETS_REPORT") {
        let body = serde_json::to_vec_pretty(&serde_json::json!({ "groups": report })).unwrap();
        std::fs::write(&path, body).expect("write decide budget report");
    }
}

criterion_group!(benches, bench_decide);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
