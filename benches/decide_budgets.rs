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
#[allow(dead_code)]
#[path = "../tests/common/test_support.rs"]
mod test_support;

use criterion::{criterion_group, Criterion};
use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_agent_types::agent_topology::{
    LeasePlan, SlotPlan, StopRule, TopologyPlan, TopologyPolicy,
};
use medulla_compute::assemble::admit_topology;
use medulla_decision_outcome_types::decision::concentration::{
    concentration_verdict, ConcentrationPolicy, EditFact, EditStatus,
};
use medulla_decision_types::decision::program_store::{
    compile_program, program_key, ProgramKnobs, MAX_PROGRAM_STEPS,
};
use medulla_decision_types::decision::run_governance::{
    RunOutcome, RunTraceRecord, StepKind, StepOutcome, TraceArgument, TraceScalar, TraceStep,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, TypedParam, TypedValue,
};
use medulla_decision_types::decision::task_plan::{TaskPlanProposal, TaskPlanTask};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_h_decide::decide::stat_test_support::{
    declared_request, publish_route_schema, request, Harness, TENANT,
};
use medulla_mutation_types::capacity_lease::LeasePriority;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::claims::{ClaimOp, EvidenceChannel};
use medulla_types::contract::BoundedVec;
use std::hint::black_box;
use tokio::runtime::{Builder, Runtime};

/// Every registered family. `family_name` matches exhaustively, so a new
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
    QuestionKind::ProcessRemediation,
    QuestionKind::IngestShortfall,
    QuestionKind::CapacityProposal,
    QuestionKind::ProbeSelection,
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
        QuestionKind::FeedbackLoop => "feedback_loop",
        QuestionKind::PlanningAdmission => "planning_admission",
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
        QuestionKind::ProcessRemediation => "process_remediation",
        QuestionKind::IngestShortfall => "ingest_shortfall",
        QuestionKind::CapacityProposal => "capacity_proposal",
        QuestionKind::ProbeSelection => "probe_selection",
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
    use medulla_types::wire::BatchOperation;
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
        status,
        policy_revision: 1,
    };
    let facts: Vec<EditFact> = (0..CONCENTRATION_FACTS)
        .map(|n| fact(n, EditStatus::Shipped))
        .collect();
    let candidate = EditFact {
        edit_id: "edit:candidate".to_string(),
        dimension_iri: dimension.to_string(),
        round: 50,
        status: EditStatus::Proposed,
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
    if index % 25 == 0 {
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

fn bench_decide(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let mut report: BTreeMap<String, serde_json::Value> = BTreeMap::new();
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
    bench_topology_admit(c, &mut report);
    bench_agent_loop(c, &rt, &mut report);
    bench_concentration_kernel(c, &mut report);
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
