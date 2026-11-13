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

use criterion::{criterion_group, Criterion};
use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_agent_types::agent_topology::{
    LeasePlan, SlotPlan, StopRule, TopologyPlan, TopologyPolicy,
};
use medulla_compute::assemble::admit_topology;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, TypedParam, TypedValue,
};
use medulla_decision_types::decision::task_plan::{TaskPlanProposal, TaskPlanTask};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_h_decide::decide::stat_test_support::{publish_route_schema, request, Harness, TENANT};
use medulla_mutation_types::capacity_lease::LeasePriority;
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
    bench_topology_admit(c, &mut report);
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
