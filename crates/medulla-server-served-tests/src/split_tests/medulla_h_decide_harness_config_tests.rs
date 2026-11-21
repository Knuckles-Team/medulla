//! The served `harness_config` question: routing, fact reads, the three-part
//! answer, its premises and its sealed outcome (MEDULLA-DECISION-ENGINE-R128).

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_agent_types::agent_component::{
    MeasuredFact, ObservationRef, ShellProficiency, StoppingBehaviour,
};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::statistical::harness_config::{
    ActionSpace, HarnessConfigAnswer, HarnessPart, PlanScaffold,
};
use medulla_decision_types::decision::statistical::log::DecisionLogEntry;
use medulla_decision_types::decision::statistical::log::{DecisionLogCommitted, DecisionLogOp};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, DecisionBatch, QuestionKind, QuestionSafety,
    StatisticalDecisionRecord, StatisticalOutcome, StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, DecisionPolicyRef, EvidenceClass, PremiseClass,
    PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_h_decide::decide::stat_test_support::{
    decode, publish_route_schema, request, Harness, TENANT,
};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;

async fn dispatch(h: &Harness, method: Method) -> Response {
    let method = rmp_serde::from_slice(&rmp_serde::to_vec_named(&method).unwrap()).unwrap();
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 128,
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

async fn route_fixture() -> (Harness, DecideRequest) {
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
    let asked = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    (h, asked)
}

/// The knobs and facts one served question is built from.
pub(super) struct Cfg {
    pub(super) shell: Option<ShellProficiency>,
    pub(super) stopping: Option<StoppingBehaviour>,
    pub(super) shell_centric: Option<bool>,
    pub(super) ablation: Option<bool>,
    pub(super) bound: Option<u64>,
    pub(super) window: u32,
    pub(super) expected: Option<i64>,
    pub(super) required: Vec<&'static str>,
    pub(super) tools: Vec<(&'static str, Vec<&'static str>, i64)>,
    pub(super) budget: i64,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            shell: Some(ShellProficiency::High),
            stopping: Some(StoppingBehaviour::Balanced),
            shell_centric: Some(false),
            ablation: Some(true),
            bound: Some(10_000),
            window: 8192,
            expected: Some(4096),
            required: vec!["eg:cap/a", "eg:cap/b"],
            tools: vec![
                ("tool-a", vec!["eg:cap/a"], 100),
                ("tool-b", vec!["eg:cap/b"], 50),
                ("tool-c", vec!["eg:cap/a", "eg:cap/c"], 400),
            ],
            budget: 1000,
        }
    }
}

const PROFILE: &str = "model-profile-1";
const TASK: &str = "urn:task:coding";
const SWARM: &str = "https://knuckles-team.github.io/ontology/swarm#";
const GRAPH: &str = "__commons__";

fn measured<T>(value: T, run: &str) -> MeasuredFact<T> {
    MeasuredFact {
        value,
        evaluation_run_ref: ObservationRef {
            evaluation_id: run.to_string(),
            digest: format!("sha256:{run}"),
        },
    }
}

fn param(name: &str, value: TypedValue) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value,
    }
}

fn number(key: &str, value: i64) -> DeclaredNumber {
    DeclaredNumber {
        key: key.to_string(),
        q32: value,
    }
}

fn options(cfg: &Cfg) -> Vec<DeclaredOption> {
    cfg.tools
        .iter()
        .map(|(id, capabilities, tokens)| DeclaredOption {
            option_id: (*id).to_string(),
            classification: BoundedVec::new(capabilities.iter().map(|c| c.to_string()).collect())
                .unwrap(),
            numbers: BoundedVec::new(vec![number("schema_tokens", *tokens)]).unwrap(),
            texts: BoundedVec::default(),
        })
        .collect()
}

fn config_request(
    schema: &medulla_agent_types::agent_component::ComponentDependency,
    cfg: &Cfg,
    profile: &str,
    policy: DecisionPolicyRef,
) -> DecideRequest {
    let mut params = vec![
        param("model_profile_ref", TypedValue::Iri(profile.to_string())),
        param("task_class", TypedValue::Iri(TASK.to_string())),
        param("usable_context_budget_tokens", TypedValue::Int(cfg.budget)),
        param("priority", TypedValue::Text("interactive".to_string())),
        param("graph", TypedValue::Text(GRAPH.to_string())),
        param(
            "required_capabilities",
            TypedValue::IriList(
                BoundedVec::new(cfg.required.iter().map(|c| c.to_string()).collect()).unwrap(),
            ),
        ),
    ];
    if let Some(expected) = cfg.expected {
        params.push(param(
            "expected_trajectory_tokens",
            TypedValue::Int(expected),
        ));
    }
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "harness.config".to_string(),
            kind: QuestionKind::HarnessConfig,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options(cfg)).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

/// A harness with the profile and task-class facts of `cfg` published.
async fn config_fixture(cfg: &Cfg) -> (Harness, DecideRequest) {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let asked = seed_config(&h, cfg).await;
    (h, asked)
}

/// Publish the profile and task-class facts of `cfg` into `h` and return the
/// question that asks for them.
pub(super) async fn seed_config(h: &Harness, cfg: &Cfg) -> DecideRequest {
    h.publish_model_profile(
        TENANT,
        PROFILE,
        cfg.window,
        cfg.shell.map(|v| measured(v, "eval:shell")),
        cfg.stopping.map(|v| measured(v, "eval:stopping")),
    )
    .unwrap();
    let mut task = serde_json::Map::new();
    if let Some(centric) = cfg.shell_centric {
        task.insert(format!("{SWARM}shellCentric"), centric.into());
    }
    if let Some(bound) = cfg.bound {
        task.insert(format!("{SWARM}taskTokenUpperBound"), bound.into());
    }
    h.seed_node(GRAPH, TASK, task.into()).await;
    let mut profile = serde_json::Map::new();
    if let Some(no_gain) = cfg.ablation {
        profile.insert(format!("{SWARM}planScaffoldNoGainAblation"), no_gain.into());
        profile.insert(
            format!("{SWARM}planScaffoldNoGainAblationRun"),
            "eval:ablation".into(),
        );
        profile.insert(
            format!("{SWARM}planScaffoldNoGainAblationRunDigest"),
            "sha256:eval:ablation".into(),
        );
    }
    h.seed_node(GRAPH, PROFILE, profile.into()).await;
    let (schema_pin, _) = publish_route_schema(h);
    config_request(&schema_pin, cfg, PROFILE, DecisionPolicyRef::Default)
}

async fn decided(h: &Harness, asked: DecideRequest) -> StatisticalDecisionRecord {
    let batch: DecisionBatch = decode(
        dispatch(
            h,
            Method::Decide {
                request: Box::new(asked),
            },
        )
        .await,
    )
    .expect("a served harness_config decision");
    batch.records.as_slice()[0].clone()
}

async fn refused(h: &Harness, asked: DecideRequest) -> Response {
    let response = dispatch(
        h,
        Method::Decide {
            request: Box::new(asked),
        },
    )
    .await;
    assert!(response.result.is_none(), "{response:?}");
    response
}

fn assert_refused_with(response: &Response, code: &str) {
    assert_eq!(response.error.as_deref(), Some("PARAMETER_INVALID"));
    assert!(
        response
            .error_detail
            .as_deref()
            .is_some_and(|detail| detail.contains(code)),
        "{response:?}"
    );
}

fn option_ids(record: &StatisticalDecisionRecord) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            scores.iter().map(|s| s.option_id.clone()).collect()
        }
        other => panic!("expected an advisory outcome, got {other:?}"),
    }
}

fn premise<'a>(record: &'a StatisticalDecisionRecord, prefix: &str) -> &'a PremiseRef {
    record
        .premises
        .iter()
        .find(|p| p.fact.starts_with(prefix))
        .unwrap_or_else(|| panic!("no premise {prefix}: {:?}", record.premises))
}

fn has_premise(record: &StatisticalDecisionRecord, prefix: &str) -> bool {
    record.premises.iter().any(|p| p.fact.starts_with(prefix))
}

pub(super) fn answer(record: &StatisticalDecisionRecord) -> HarnessConfigAnswer {
    record
        .inputs
        .harness_evaluation
        .clone()
        .expect("the record carries the harness evidence")
        .answer
}

fn certificate(
    record: &StatisticalDecisionRecord,
) -> Option<medulla_decision_types::decision::statistical::harness_config::HarnessSolverCertificate>
{
    record
        .inputs
        .harness_evaluation
        .clone()
        .expect("the record carries the harness evidence")
        .tool_subset_certificate
}

fn pinned_policy(h: &Harness, name: &str, knobs: &[(&str, i64)]) -> DecisionPolicyRef {
    let mut policy = DecisionPolicy::engine_default();
    for (key, micros) in knobs {
        policy
            .knobs
            .insert(KnobName::new(key), KnobValue::DecimalMicros(*micros));
    }
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(name, &policy),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R128.10
#[tokio::test]
async fn decide_routes_harness_config() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    let record = decided(&h, asked.clone()).await;
    assert_eq!(record.question.kind, QuestionKind::HarnessConfig);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    for name in [
        "harness_config.context_window_tokens=",
        "harness_config.expected_trajectory_tokens=",
        "harness_config.task_token_upper_bound=",
        "harness_config.pressure_q32=",
        "harness_config.shell_proficiency=",
        "harness_config.shell_centric=",
        "harness_config.stopping_behaviour=",
        "harness_config.plan_scaffold_ablation=",
        "knob:harness.soft_threshold=",
        "knob:harness.hard_threshold=",
        "knob:harness.recent_window_fraction=",
    ] {
        assert!(has_premise(&record, name), "{name}: {:?}", record.premises);
    }
    // A request whose candidates are not declared options is refused.
    let (h, route) = route_fixture().await;
    let mut library = route;
    library.question.kind = QuestionKind::HarnessConfig;
    let response = refused(&h, library.clone()).await;
    assert_refused_with(&response, "declares its own tool candidates");
    // Another tenant's request is denied at admission.
    library.tenant_id = "tenant-other".into();
    let foreign = refused(&h, library).await;
    assert_eq!(foreign.error.as_deref(), Some("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R128.8
#[tokio::test]
async fn unknown_profile_is_refused() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let cfg = Cfg::default();
    let (h, asked) = config_fixture(&cfg).await;
    let (schema_pin, _) = publish_route_schema(&h);
    let asked = config_request(&schema_pin, &cfg, "no-such-profile", asked.policy.clone());
    assert_refused_with(&refused(&h, asked).await, "HARNESS_PROFILE_UNKNOWN");
}

// spec: MEDULLA-DECISION-ENGINE-R128.8
#[tokio::test]
async fn other_tenant_profile_is_refused() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let cfg = Cfg::default();
    let (h, _) = config_fixture(&cfg).await;
    h.publish_model_profile("tenant-other", "foreign-profile", 8192, None, None)
        .unwrap();
    let (schema_pin, _) = publish_route_schema(&h);
    let asked = config_request(
        &schema_pin,
        &cfg,
        "foreign-profile",
        DecisionPolicyRef::Default,
    );
    assert_refused_with(&refused(&h, asked).await, "HARNESS_PROFILE_UNKNOWN");
}

// spec: MEDULLA-DECISION-ENGINE-R128.8
#[tokio::test]
async fn unknown_task_fact_stays_unknown() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg {
        shell_centric: None,
        ..Cfg::default()
    })
    .await;
    let record = decided(&h, asked).await;
    let answer = answer(&record);
    match &answer.action_space {
        HarnessPart::Abstained {
            missing_fact,
            conservative_default,
        } => {
            assert_eq!(missing_fact.as_slice(), ["shellCentric".to_string()]);
            assert_eq!(*conservative_default, ActionSpace::ToolSet);
        }
        other => panic!("an absent shellCentric must abstain, got {other:?}"),
    }
    assert!(!option_ids(&record)
        .iter()
        .any(|id| id.starts_with("action_space:")));
    assert!(has_premise(&record, "harness_config.missing:shellCentric"));
    assert!(!has_premise(&record, "harness_config.shell_centric="));
}

// spec: MEDULLA-DECISION-ENGINE-R128.9
#[tokio::test]
async fn tool_set_reuses_exact_subset() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let cfg = Cfg::default();
    let (h, asked) = config_fixture(&cfg).await;
    let record = decided(&h, asked.clone()).await;
    let HarnessPart::Decided { value } = answer(&record).tool_subset else {
        panic!("the tool subset is decided for a decided tool_set action space");
    };
    // The same inputs asked as a `tool_subset` question.
    let again_asked = asked.clone();
    let mut subset = asked;
    subset.question.kind = QuestionKind::ToolSubset;
    subset.params = BoundedVec::new(vec![
        param(
            "required_capabilities",
            TypedValue::IriList(
                BoundedVec::new(cfg.required.iter().map(|c| c.to_string()).collect()).unwrap(),
            ),
        ),
        param("context_budget_tokens", TypedValue::Int(cfg.budget)),
    ])
    .unwrap();
    let expected = option_ids(&decided(&h, subset).await);
    assert_eq!(value.as_slice(), expected.as_slice());
    assert_eq!(expected, vec!["tool-a".to_string(), "tool-b".to_string()]);
    assert!(option_ids(&record).contains(&"tool_subset:[tool-a,tool-b]".to_string()));
    // The verified solver certificate enters the sealed record, and the same
    // inputs solve to the same certificate.
    let stored = certificate(&record).expect("the solve's certificate is in the record");
    assert!(!stored.model_digest.is_empty());
    assert!(!stored.solver_certificate_digest.is_empty());
    let again = decided(&h, again_asked).await;
    assert_eq!(certificate(&again), Some(stored));
}

// spec: MEDULLA-DECISION-ENGINE-R128.9
#[tokio::test]
async fn shell_only_carries_an_empty_subset() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg {
        shell_centric: Some(true),
        ..Cfg::default()
    })
    .await;
    let record = decided(&h, asked).await;
    assert_eq!(certificate(&record), None, "shell only runs no solve");
    let answer = answer(&record);
    assert_eq!(
        answer.action_space,
        HarnessPart::Decided {
            value: ActionSpace::ShellOnly
        }
    );
    assert_eq!(
        answer.tool_subset,
        HarnessPart::Decided {
            value: BoundedVec::default()
        }
    );
    let ids = option_ids(&record);
    assert!(
        ids.contains(&"action_space:shell_only".to_string()),
        "{ids:?}"
    );
    assert!(ids.contains(&"tool_subset:[]".to_string()), "{ids:?}");
}

// spec: MEDULLA-DECISION-ENGINE-R128.9
#[tokio::test]
async fn uncoverable_capability_abstains_the_subset_part() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg {
        required: vec!["eg:cap/a", "eg:cap/zzz"],
        ..Cfg::default()
    })
    .await;
    let record = decided(&h, asked).await;
    match answer(&record).tool_subset {
        HarnessPart::Abstained {
            missing_fact,
            conservative_default,
        } => {
            assert_eq!(
                missing_fact.as_slice(),
                ["coverageOf:eg:cap/zzz".to_string()]
            );
            assert!(conservative_default.is_empty());
        }
        other => panic!("an uncoverable capability must abstain the part, got {other:?}"),
    }
    assert!(has_premise(
        &record,
        "harness_config.missing:coverageOf:eg:cap/zzz"
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R128.4
#[tokio::test]
async fn policy_knobs_replace_default_thresholds() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    let default = decided(&h, asked.clone()).await;
    let mut pinned = asked;
    pinned.policy = pinned_policy(
        &h,
        "policy-soft-half",
        &[("harness.soft_threshold", 500_000)],
    );
    let record = decided(&h, pinned).await;
    let fraction = |record: &StatisticalDecisionRecord| {
        let HarnessPart::Decided { value } = answer(record).soft_threshold else {
            panic!("the threshold is decided with the context policy");
        };
        (value.numerator(), value.denominator())
    };
    assert_eq!(fraction(&default), (2_576_980_378, 1 << 32));
    assert_eq!(fraction(&record), (2_147_483_648, 1 << 32));
    assert_eq!(
        premise(&record, "knob:harness.soft_threshold=").fact,
        "knob:harness.soft_threshold=0.500000"
    );
    assert_eq!(
        premise(&record, "knob:harness.hard_threshold=").fact,
        "knob:harness.hard_threshold=0.850000"
    );
    assert_eq!(
        premise(&record, "knob:harness.recent_window_fraction=").fact,
        "knob:harness.recent_window_fraction=0.300000"
    );
    // Pressure is expected tokens over the window and does not move with a threshold.
    assert_eq!(
        premise(&record, "harness_config.pressure_q32=").fact,
        premise(&default, "harness_config.pressure_q32=").fact
    );
}

// spec: MEDULLA-DECISION-ENGINE-R128.4
#[tokio::test]
async fn inverted_thresholds_refuse() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, mut asked) = config_fixture(&Cfg::default()).await;
    asked.policy = pinned_policy(
        &h,
        "policy-inverted",
        &[
            ("harness.soft_threshold", 900_000),
            ("harness.hard_threshold", 800_000),
        ],
    );
    assert_refused_with(&refused(&h, asked).await, "HARNESS_THRESHOLDS_INVALID");
}

// spec: MEDULLA-DECISION-ENGINE-R128.11
#[tokio::test]
async fn premises_and_why_not_cover_each_part() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    let record = decided(&h, asked).await;
    for (prefix, class) in [
        ("harness_config.model_profile_ref=", PremiseClass::Claim),
        ("harness_config.task_class=", PremiseClass::Claim),
        (
            "harness_config.usable_context_budget_tokens=",
            PremiseClass::Claim,
        ),
        ("harness_config.context_window_tokens=", PremiseClass::Claim),
        (
            "harness_config.expected_trajectory_tokens=",
            PremiseClass::Claim,
        ),
        (
            "harness_config.task_token_upper_bound=",
            PremiseClass::Claim,
        ),
        ("harness_config.pressure_q32=", PremiseClass::Definition),
        (
            "harness_config.shell_proficiency=",
            PremiseClass::Observation,
        ),
        ("harness_config.shell_centric=", PremiseClass::Claim),
        (
            "harness_config.stopping_behaviour=",
            PremiseClass::Observation,
        ),
        (
            "harness_config.plan_scaffold_ablation=",
            PremiseClass::Observation,
        ),
    ] {
        assert_eq!(premise(&record, prefix).class, class, "{prefix}");
    }
    assert_eq!(
        premise(&record, "harness_config.shell_proficiency=").provenance,
        PremiseProvenance::Observation {
            evaluation_id: "eval:shell".to_string()
        }
    );
    // One why-not for each rejected value of each part, text = rule code.
    for (fact, code, class) in [
        (
            "harness_config.why_not:context_policy:none",
            "TASK_BOUND_EXCEEDS_WINDOW",
        ),
        (
            "harness_config.why_not:context_policy:elide",
            "ELIDE_ONLY_NOT_SELECTED",
        ),
        (
            "harness_config.why_not:plan_scaffold:on",
            "ABLATION_SHOWS_NO_GAIN",
        ),
        (
            "harness_config.why_not:action_space:shell_only",
            "SHELL_ONLY_CONDITIONS_NOT_MET",
        ),
        (
            "harness_config.why_not:tool_subset:tool-c",
            "NOT_IN_MINIMAL_COVER",
        ),
    ]
    .map(|(fact, code)| (fact, code, PremiseClass::Definition))
    {
        let why = premise(&record, fact);
        assert_eq!(why.fact, fact);
        assert_eq!(why.subject, code, "{fact}");
        assert_eq!(why.class, class, "{fact}");
    }
    assert_eq!(
        record
            .premises
            .iter()
            .filter(|p| p.fact.starts_with("harness_config.why_not:"))
            .count(),
        5,
        "{:?}",
        record.premises
    );
}

// spec: MEDULLA-DECISION-ENGINE-R128.11
#[tokio::test]
async fn missing_fact_adds_a_claim_premise() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg {
        shell: None,
        ..Cfg::default()
    })
    .await;
    let record = decided(&h, asked).await;
    let missing = premise(&record, "harness_config.missing:shellProficiency");
    assert_eq!(missing.class, PremiseClass::Claim);
    assert!(!has_premise(&record, "harness_config.shell_proficiency="));
}

// spec: MEDULLA-DECISION-ENGINE-R128.12
#[tokio::test]
async fn all_parts_abstained_seals_abstention() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg {
        shell: None,
        stopping: None,
        shell_centric: None,
        ablation: None,
        bound: None,
        expected: None,
        required: Vec::new(),
        ..Cfg::default()
    })
    .await;
    let record = decided(&h, asked).await;
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("every part abstains: {:?}", record.outcome);
    };
    let mut fields: Vec<&str> = reasons
        .iter()
        .map(|reason| match reason {
            AbstainReason::UnknownFact { field, .. } => field.as_str(),
            other => panic!("unexpected reason {other:?}"),
        })
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "expected_trajectory_tokens",
            "shellCentric",
            "shellProficiency",
            "stoppingBehaviour"
        ]
    );
}

// spec: MEDULLA-DECISION-ENGINE-R128.12
#[tokio::test]
async fn mixed_parts_seal_an_advisory_outcome() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg {
        shell: None,
        ..Cfg::default()
    })
    .await;
    let record = decided(&h, asked).await;
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert_eq!(
        option_ids(&record),
        vec![
            "context_policy:elide_then_summarize".to_string(),
            "soft_threshold:2576980378/4294967296".to_string(),
            "hard_threshold:3650722202/4294967296".to_string(),
            "recent_window_fraction:1288490189/4294967296".to_string(),
            "plan_scaffold:off".to_string(),
        ]
    );
    let evidence = record
        .inputs
        .harness_evaluation
        .clone()
        .expect("the record fills the harness evidence");
    assert_eq!(evidence.model_profile.component_id, PROFILE);
    assert_eq!(evidence.request.model_profile_ref, PROFILE);
    assert!(matches!(
        evidence.answer.action_space,
        HarnessPart::Abstained { .. }
    ));
    assert!(matches!(
        evidence.answer.tool_subset,
        HarnessPart::Abstained { .. }
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R128.1
// spec: MEDULLA-DECISION-ENGINE-R128.13.1
// spec: MEDULLA-DECISION-ENGINE-R128.13.1.1
#[tokio::test]
async fn harness_config_replay_refuses_a_relabelled_generic_record() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = route_fixture().await;
    let baseline: DecisionBatch = decode(
        dispatch(
            &h,
            Method::Decide {
                request: Box::new(asked),
            },
        )
        .await,
    )
    .unwrap();
    let record = baseline.records.as_slice()[0].clone();
    let committed_response = dispatch(
        &h,
        Method::DecisionLog {
            op: Box::new(DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            }),
        },
    )
    .await;
    assert!(
        committed_response.error.is_none(),
        "authorized Route DecisionLog control: {committed_response:?}"
    );
    let committed: DecisionLogCommitted = decode(committed_response).unwrap();
    assert_eq!(committed.record_id, record.record_id);
    for evidence in [false, true] {
        let mut relabelled = record.clone();
        if evidence {
            relabelled.inputs.harness_evaluation = Some(unverified_harness_evidence());
        } else {
            relabelled.question.kind = QuestionKind::HarnessConfig;
        }
        relabelled.inputs_digest = medulla_query_types::solve_scalar::digest_text(
            medulla_h_decide::decide::stat_decide::STATISTICAL_INPUTS_DOMAIN,
            &(&relabelled.question, &relabelled.inputs),
        );
        relabelled.record_id = format!(
            "decision:{}",
            relabelled.inputs_digest.trim_start_matches("sha256:"),
        );
        relabelled.record_digest =
            medulla_decision_types::decision::digest::statistical_record_digest(&relabelled);
        assert_refused_with(
            &dispatch(
                &h,
                Method::DecisionLog {
                    op: Box::new(DecisionLogOp::Commit {
                        record: Box::new(relabelled.clone()),
                        evaluator: None,
                    }),
                },
            )
            .await,
            "HARNESS_REPLAY_MISMATCH",
        );
        assert!(h
            .store
            .decision_artifact(
                TENANT,
                &medulla_persistence_records::decision_jobs::record_key(&relabelled.record_id),
            )
            .unwrap()
            .is_none());
    }
}

fn unverified_harness_evidence(
) -> medulla_decision_types::decision::statistical::harness_config::HarnessConfigEvaluationEvidence
{
    let missing = |value: serde_json::Value| {
        serde_json::json!({
            "decision": "abstained", "missing_fact": ["unverified"],
            "conservative_default": value
        })
    };
    serde_json::from_value(serde_json::json!({
        "model_profile": {"component_id": "profile", "kind": "model_profile",
            "definition_digest": "sha256:unverified"},
        "request": {"model_profile_ref": "profile", "task_class": "task",
            "usable_context_budget_tokens": 10, "priority": "interactive"},
        "answer": {
            "context_policy": missing(serde_json::json!("elide_then_summarize")),
            "soft_threshold": missing(serde_json::json!({"numerator": 1, "denominator": 4294967296u64})),
            "hard_threshold": missing(serde_json::json!({"numerator": 2, "denominator": 4294967296u64})),
            "recent_window_fraction": missing(serde_json::json!({"numerator": 1, "denominator": 4294967296u64})),
            "plan_scaffold": missing(serde_json::json!("on")),
            "action_space": missing(serde_json::json!("tool_set")),
            "tool_subset": missing(serde_json::json!([]))
        }
    })).unwrap()
}

/// The stored node of one assistant profile, in the layout the served profile
/// store writes, under `tenant_scope`.
async fn seed_assistant_profile(
    h: &Harness,
    tenant_scope: &str,
    profile: &medulla_identity_types::assistant_profile::AssistantProfile,
) {
    h.seed_node(
        GRAPH,
        &medulla_identity_types::assistant_profile::profile_node_id(&profile.profile_id),
        serde_json::json!({
            "type": "AssistantProfile",
            "tenant_scope": tenant_scope,
            "json": serde_json::to_string(profile).unwrap(),
        }),
    )
    .await;
}

fn assistant_profile(
    id: &str,
    owner: &str,
    allowlist: &[&str],
    hints: medulla_identity_types::assistant_profile::ModelHints,
) -> medulla_identity_types::assistant_profile::AssistantProfile {
    medulla_identity_types::assistant_profile::AssistantProfile {
        profile_id: id.to_string(),
        tenant: TENANT.to_string(),
        name: id.to_string(),
        instructions: "answer briefly".to_string(),
        document_set_ids: Vec::new(),
        tool_allowlist: allowlist.iter().map(|tool| tool.to_string()).collect(),
        model_hints: hints,
        owner_principal: owner.to_string(),
        shares: Vec::new(),
        revision: 3,
        schema_version: medulla_identity_types::assistant_profile::ASSISTANT_PROFILE_SCHEMA_VERSION,
    }
}

fn no_hints() -> medulla_identity_types::assistant_profile::ModelHints {
    medulla_identity_types::assistant_profile::ModelHints {
        preferred_model_profile_refs: Vec::new(),
        max_context_tokens: None,
    }
}

fn with_profile(mut asked: DecideRequest, profile_id: &str) -> DecideRequest {
    let mut params = asked.params.as_slice().to_vec();
    params.push(param(
        "assistant_profile_ref",
        TypedValue::Iri(profile_id.to_string()),
    ));
    asked.params = BoundedVec::new(params).unwrap();
    asked
}

fn without_param(mut asked: DecideRequest, name: &str) -> DecideRequest {
    let params: Vec<TypedParam> = asked
        .params
        .as_slice()
        .iter()
        .filter(|p| p.name != name)
        .cloned()
        .collect();
    asked.params = BoundedVec::new(params).unwrap();
    asked
}

fn effective_request(
    record: &StatisticalDecisionRecord,
) -> medulla_decision_types::decision::statistical::harness_config::HarnessConfigRequestBody {
    record
        .inputs
        .harness_evaluation
        .clone()
        .expect("the record carries the harness evidence")
        .request
}

// spec: MEDULLA-DECISION-ENGINE-R128.16
#[tokio::test]
async fn allowlist_narrows_tool_subset() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let cfg = Cfg::default();
    let scope = medulla_server_api::carrier::tenant_scope_of(TENANT);
    let (h, asked) = config_fixture(&cfg).await;
    // Without a profile the cheapest cover is tool-a + tool-b.
    let plain = decided(&h, asked.clone()).await;
    assert!(option_ids(&plain).contains(&"tool_subset:[tool-a,tool-b]".to_string()));
    assert!(!has_premise(&plain, "profile:"));

    // The allowlist removes tool-a, so the exact solve covers capability a
    // with tool-c; the usable budget is the smaller of the request and the
    // profile's maximum; the removed tool carries its why-not; the profile
    // enters as a premise.
    let narrowing = assistant_profile(
        "prof-narrow",
        "decider",
        &["tool-b", "tool-c"],
        medulla_identity_types::assistant_profile::ModelHints {
            preferred_model_profile_refs: Vec::new(),
            max_context_tokens: Some(500),
        },
    );
    seed_assistant_profile(&h, &scope, &narrowing).await;
    let record = decided(&h, with_profile(asked.clone(), "prof-narrow")).await;
    let HarnessPart::Decided { value } = answer(&record).tool_subset else {
        panic!("the narrowed subset is decided");
    };
    let mut chosen = value.as_slice().to_vec();
    chosen.sort();
    assert_eq!(chosen, vec!["tool-b".to_string(), "tool-c".to_string()]);
    let body = effective_request(&record);
    assert_eq!(body.usable_context_budget_tokens, 500);
    assert_eq!(body.assistant_profile_ref.as_deref(), Some("prof-narrow"));
    assert_eq!(body.model_profile_ref, PROFILE);
    assert_eq!(
        premise(&record, "profile:prof-narrow@3").provenance,
        PremiseProvenance::Request {
            field: "assistant_profile_ref".to_string()
        }
    );
    let removed = premise(&record, "harness_config.why_not:tool_subset:tool-a");
    assert_eq!(removed.subject, "PROFILE_TOOL_NOT_ALLOWED");

    // A request value always wins over a hint: a budget below the profile's
    // maximum stays as asked.
    let roomy = assistant_profile(
        "prof-roomy",
        "decider",
        &["tool-a", "tool-b", "tool-c"],
        medulla_identity_types::assistant_profile::ModelHints {
            preferred_model_profile_refs: Vec::new(),
            max_context_tokens: Some(5000),
        },
    );
    seed_assistant_profile(&h, &scope, &roomy).await;
    let record = decided(&h, with_profile(asked.clone(), "prof-roomy")).await;
    assert_eq!(
        effective_request(&record).usable_context_budget_tokens,
        1000
    );
    assert!(option_ids(&record).contains(&"tool_subset:[tool-a,tool-b]".to_string()));

    // An absent model_profile_ref takes the first preferred profile the
    // library holds, skipping one it does not.
    let hinted = assistant_profile(
        "prof-hinted",
        "decider",
        &["tool-a", "tool-b", "tool-c"],
        medulla_identity_types::assistant_profile::ModelHints {
            preferred_model_profile_refs: vec!["no-such-profile".to_string(), PROFILE.to_string()],
            max_context_tokens: None,
        },
    );
    seed_assistant_profile(&h, &scope, &hinted).await;
    let record = decided(
        &h,
        with_profile(
            without_param(asked.clone(), "model_profile_ref"),
            "prof-hinted",
        ),
    )
    .await;
    assert_eq!(effective_request(&record).model_profile_ref, PROFILE);
    // None of the preferred profiles exists: refused, not defaulted.
    let lost = assistant_profile(
        "prof-lost",
        "decider",
        &["tool-a"],
        medulla_identity_types::assistant_profile::ModelHints {
            preferred_model_profile_refs: vec!["no-such-profile".to_string()],
            max_context_tokens: None,
        },
    );
    seed_assistant_profile(&h, &scope, &lost).await;
    let response = refused(
        &h,
        with_profile(
            without_param(asked.clone(), "model_profile_ref"),
            "prof-lost",
        ),
    )
    .await;
    assert_refused_with(&response, "HARNESS_PROFILE_UNKNOWN");

    // An allowlist that removes every declared tool abstains the action space
    // with PROFILE_TOOL_NOT_ALLOWED and an empty tool set.
    let closed = assistant_profile("prof-closed", "decider", &["tool-zzz"], no_hints());
    seed_assistant_profile(&h, &scope, &closed).await;
    let record = decided(&h, with_profile(asked.clone(), "prof-closed")).await;
    let HarnessPart::Abstained {
        missing_fact,
        conservative_default,
    } = answer(&record).action_space
    else {
        panic!("an emptied candidate set abstains the action space");
    };
    assert_eq!(
        missing_fact.as_slice(),
        ["PROFILE_TOOL_NOT_ALLOWED".to_string()]
    );
    assert_eq!(conservative_default, ActionSpace::ToolSet);
    match answer(&record).tool_subset {
        HarnessPart::Abstained {
            conservative_default,
            ..
        } => assert!(conservative_default.is_empty()),
        other => panic!("the tool subset follows the abstained action space: {other:?}"),
    }
    assert!(has_premise(
        &record,
        "harness_config.missing:PROFILE_TOOL_NOT_ALLOWED"
    ));
    assert!(!option_ids(&record)
        .iter()
        .any(|id| id.starts_with("action_space:")));

    // Cross-tenant negative: another tenant's profile, an absent one and one
    // this caller may not read all refuse alike.
    let foreign = assistant_profile("prof-foreign", "decider", &["tool-a"], no_hints());
    seed_assistant_profile(&h, "carrier-tenant-other", &foreign).await;
    let private = assistant_profile("prof-private", "someone-else", &["tool-a"], no_hints());
    seed_assistant_profile(&h, &scope, &private).await;
    for id in ["prof-foreign", "prof-private", "prof-absent"] {
        let response = refused(&h, with_profile(asked.clone(), id)).await;
        assert_refused_with(&response, "PROFILE_NOT_VISIBLE");
    }
    // A profile reference needs the graph that holds the profile.
    let response = refused(
        &h,
        with_profile(without_param(asked, "graph"), "prof-narrow"),
    )
    .await;
    assert_refused_with(&response, "needs the graph param");
}

// spec: MEDULLA-DECISION-ENGINE-R656.4
/// Verifies MEDULLA-DECISION-ENGINE-R656.4: a served `Decide` of kind
/// `harness_config` that names an assistant profile the verified caller cannot
/// read is refused with `PROFILE_NOT_VISIBLE`, the same for another tenant's
/// profile, another principal's unshared profile and one that does not exist;
/// a refusal commits no decision record, and the caller's own profile still
/// constrains the run and enters the record as a `profile:<id>@<revision>`
/// premise.
#[tokio::test]
async fn profile_ref_without_access_is_refused() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let cfg = Cfg::default();
    let scope = medulla_server_api::carrier::tenant_scope_of(TENANT);
    let (h, asked) = config_fixture(&cfg).await;

    let own = assistant_profile("prof-own", "decider", &["tool-b", "tool-c"], no_hints());
    seed_assistant_profile(&h, &scope, &own).await;
    let foreign = assistant_profile("prof-foreign", "decider", &["tool-a"], no_hints());
    seed_assistant_profile(&h, "carrier-tenant-other", &foreign).await;
    let private = assistant_profile("prof-private", "someone-else", &["tool-a"], no_hints());
    seed_assistant_profile(&h, &scope, &private).await;

    // The refusal is the one non-disclosing code for every profile the caller
    // cannot read, whether it is absent, another tenant's or unshared.
    for id in ["prof-foreign", "prof-private", "prof-absent"] {
        let response = refused(&h, with_profile(asked.clone(), id)).await;
        assert_refused_with(&response, "PROFILE_NOT_VISIBLE");
    }

    // The same request with the caller's own profile is decided under the
    // profile's constraints, so the refusals above changed nothing.
    let record = decided(&h, with_profile(asked, "prof-own")).await;
    let mut chosen = match answer(&record).tool_subset {
        HarnessPart::Decided { value } => value.as_slice().to_vec(),
        other => panic!("the profile-constrained subset is decided: {other:?}"),
    };
    chosen.sort();
    assert_eq!(chosen, vec!["tool-b".to_string(), "tool-c".to_string()]);
    assert!(has_premise(&record, "profile:prof-own@3"));
}

// ---------------------------------------------------------------------------
// Commit, replay and limits (MEDULLA-DECISION-ENGINE-R128.13)
// ---------------------------------------------------------------------------

async fn commit_response(h: &Harness, record: &StatisticalDecisionRecord) -> Response {
    dispatch(
        h,
        Method::DecisionLog {
            op: Box::new(DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            }),
        },
    )
    .await
}

fn stored(h: &Harness, record_id: &str) -> bool {
    h.store
        .decision_artifact(
            TENANT,
            &medulla_persistence_records::decision_jobs::record_key(record_id),
        )
        .unwrap()
        .is_some()
}

/// A record whose content was changed, with its ids and digests recomputed so
/// only the replay can tell it was changed.
fn resealed(mut record: StatisticalDecisionRecord) -> StatisticalDecisionRecord {
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

fn pinned_integer_policy(h: &Harness, name: &str, knobs: &[(&str, i64)]) -> DecisionPolicyRef {
    let mut policy = DecisionPolicy::engine_default();
    for (key, value) in knobs {
        policy
            .knobs
            .insert(KnobName::new(key), KnobValue::Integer(*value));
    }
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(name, &policy),
    }
}

fn declared(id: String, capability: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id,
        classification: BoundedVec::new(vec![capability.to_string()]).unwrap(),
        numbers: BoundedVec::new(vec![number("schema_tokens", 10)]).unwrap(),
        texts: BoundedVec::default(),
    }
}

fn with_options(mut asked: DecideRequest, options: Vec<DeclaredOption>) -> DecideRequest {
    asked.candidates = CandidateSource::Declared {
        options: BoundedVec::new(options).unwrap(),
    };
    asked
}

// spec: MEDULLA-DECISION-ENGINE-R128.13
// spec: MEDULLA-DECISION-ENGINE-R128.13.2
// spec: MEDULLA-DECISION-ENGINE-R128.13.2.1
#[tokio::test]
async fn harness_config_record_commits() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    let record = decided(&h, asked).await;
    assert!(!stored(&h, &record.record_id));
    let response = commit_response(&h, &record).await;
    assert!(response.error.is_none(), "{response:?}");
    let committed: DecisionLogCommitted = decode(response).unwrap();
    assert_eq!(committed.record_id, record.record_id);
    assert!(!committed.replayed);
    // Read back: the committed record has the identical premises and answer.
    let read: Option<DecisionLogEntry> = decode(
        dispatch(
            &h,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::Get {
                    tenant_id: TENANT.to_string(),
                    record_id: record.record_id.clone(),
                }),
            },
        )
        .await,
    )
    .unwrap();
    let entry = read.expect("the committed record reads back");
    assert_eq!(entry.record.premises, record.premises);
    assert_eq!(
        entry.record.inputs.harness_evaluation,
        record.inputs.harness_evaluation
    );
    assert_eq!(entry.record.outcome, record.outcome);
    // A repeat is an idempotent replay.
    let again: DecisionLogCommitted = decode(commit_response(&h, &record).await).unwrap();
    assert!(again.replayed);
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.2.1
#[tokio::test]
async fn forged_answer_refuses_replay() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    let record = decided(&h, asked).await;
    let mut forged = record.clone();
    let evidence = forged.inputs.harness_evaluation.as_mut().unwrap();
    evidence.answer.plan_scaffold = HarnessPart::Abstained {
        missing_fact: BoundedVec::new(vec!["forged".to_string()]).unwrap(),
        conservative_default: PlanScaffold::On,
    };
    let forged = resealed(forged);
    assert_refused_with(
        &commit_response(&h, &forged).await,
        "HARNESS_REPLAY_MISMATCH",
    );
    assert!(!stored(&h, &forged.record_id));
    assert!(!stored(&h, &record.record_id));
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.2.1
#[tokio::test]
async fn changed_premise_refuses_replay() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    let record = decided(&h, asked).await;
    for (prefix, changed) in [
        // Read from the pinned profile again: a changed window cannot replay.
        (
            "harness_config.context_window_tokens=",
            "harness_config.context_window_tokens=1",
        ),
        // A task fact taken from the stored premise: flipping it changes the answer.
        (
            "harness_config.shell_centric=",
            "harness_config.shell_centric=true",
        ),
        // A premise the derivation never produced.
        (
            "harness_config.pressure_q32=",
            "harness_config.pressure_q32=1",
        ),
    ] {
        let mut forged = record.clone();
        let mut premises = forged.premises.as_slice().to_vec();
        let premise = premises
            .iter_mut()
            .find(|p| p.fact.starts_with(prefix))
            .unwrap_or_else(|| panic!("no premise {prefix}"));
        premise.fact = changed.to_string();
        forged.premises = BoundedVec::new(premises).unwrap();
        let forged = resealed(forged);
        assert_refused_with(
            &commit_response(&h, &forged).await,
            "HARNESS_REPLAY_MISMATCH",
        );
        assert!(!stored(&h, &forged.record_id), "{prefix}");
    }
    assert!(!stored(&h, &record.record_id));
}

fn long_options(count: usize, id_bytes: usize) -> Vec<DeclaredOption> {
    (0..count)
        .map(|i| declared(format!("tool-{i:04}-{}", "x".repeat(id_bytes)), "eg:cap/a"))
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.1.2
#[tokio::test]
async fn over_limit_evidence_refuses_before_allocation() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, base) = config_fixture(&Cfg::default()).await;
    // Items: 64 options with a capability each, past a 64-item limit.
    let items = {
        let mut asked = with_options(base.clone(), long_options(64, 4));
        asked.policy =
            pinned_integer_policy(&h, "evidence-items", &[("harness.evidence_max_items", 64)]);
        asked
    };
    // One string: a task class past a 1 KiB string limit.
    let string = {
        let mut asked = base.clone();
        let mut params = asked.params.as_slice().to_vec();
        for param in &mut params {
            if param.name == "task_class" {
                param.value = TypedValue::Iri(format!("urn:task:{}", "t".repeat(2048)));
            }
        }
        asked.params = BoundedVec::new(params).unwrap();
        asked.policy = pinned_integer_policy(
            &h,
            "evidence-string",
            &[("harness.evidence_max_string_bytes", 1024)],
        );
        asked
    };
    // Bytes: forty 2 KiB option ids, past a 64 KiB frame limit.
    let bytes = {
        let mut asked = with_options(base.clone(), long_options(40, 2048));
        asked.policy = pinned_integer_policy(
            &h,
            "evidence-bytes",
            &[("harness.evidence_max_bytes", 65_536)],
        );
        asked
    };
    for (name, asked) in [("items", items), ("string", string), ("bytes", bytes)] {
        let record = decided(&h, asked).await;
        let response = commit_response(&h, &record).await;
        assert_refused_with(&response, "HARNESS_EVIDENCE_OVER_LIMIT");
        assert!(!stored(&h, &record.record_id), "{name}");
    }
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.2.2
#[tokio::test]
async fn over_limit_replay_refuses_before_start() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, base) = config_fixture(&Cfg::default()).await;
    // Premises: the sealed record holds more than 16.
    let premises = {
        let mut asked = base.clone();
        asked.policy = pinned_integer_policy(
            &h,
            "replay-premises",
            &[("harness.replay_max_premises", 16)],
        );
        asked
    };
    // Candidates: nine declared options, past an 8-candidate limit.
    let candidates = {
        let mut asked = with_options(base.clone(), long_options(9, 4));
        asked.policy = pinned_integer_policy(
            &h,
            "replay-candidates",
            &[("harness.replay_max_candidates", 8)],
        );
        asked
    };
    // Bytes: forty 2 KiB option ids, past a 64 KiB replay size.
    let bytes = {
        let mut asked = with_options(base.clone(), long_options(40, 2048));
        asked.policy =
            pinned_integer_policy(&h, "replay-bytes", &[("harness.replay_max_bytes", 65_536)]);
        asked
    };
    for (name, asked) in [
        ("premises", premises),
        ("candidates", candidates),
        ("bytes", bytes),
    ] {
        let record = decided(&h, asked).await;
        assert_refused_with(&commit_response(&h, &record).await, "HARNESS_REPLAY_LIMIT");
        assert!(!stored(&h, &record.record_id), "{name}");
    }
}

/// A published `Tool` of the harness tenant that claims to be read-only,
/// declared by `declared_by`.
fn publish_read_only_tool(
    h: &Harness,
    id: &str,
    declared_by: medulla_agent_types::agent_component::ToolFactDeclarer,
) {
    use medulla_agent_types::agent_component::{AgentComponentFacts, ToolEffect, ToolFactDeclarer};
    let mut draft = medulla_persistence_records::agent_component::test_component_draft(TENANT, id);
    draft.facts = AgentComponentFacts::Tool {
        effect: ToolEffect::Read,
        required_scopes: Vec::new(),
        input_schema_digest: None,
        output_schema_digest: None,
        read_only_hint: Some(true),
        destructive_hint: None,
        idempotent_hint: None,
        open_world_hint: None,
        modalities: Default::default(),
        cost: None,
        latency_declared: None,
        side_effects: None,
        declared_by: Some(declared_by),
        schema_tokens: Some(100),
        read_only_unproven: declared_by == ToolFactDeclarer::McpHint,
    };
    h.commit(draft, None).unwrap();
}

// spec: MEDULLA-TYPED-PACKS-R045.7
#[tokio::test]
async fn mcp_hint_read_only_premise_is_unproven() {
    use medulla_agent_types::agent_component::ToolFactDeclarer;
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (h, asked) = config_fixture(&Cfg::default()).await;
    publish_read_only_tool(&h, "tool-a", ToolFactDeclarer::McpHint);
    publish_read_only_tool(&h, "tool-b", ToolFactDeclarer::Connector);
    let record = decided(&h, asked).await;
    let label_of = |tool: &str| {
        record
            .premises
            .iter()
            .find(|p| p.subject == tool && p.fact.starts_with("harness_config.tool_read_only="))
            .map(|p| p.fact.clone())
    };
    assert_eq!(
        label_of("tool-a").as_deref(),
        Some("harness_config.tool_read_only=unproven"),
        "an MCP hint read-only claim must be labelled unproven"
    );
    assert_eq!(
        label_of("tool-b").as_deref(),
        Some("harness_config.tool_read_only=certified"),
        "a connector-declared read-only claim is certified"
    );
    assert_eq!(
        label_of("tool-c"),
        None,
        "a tool the library does not hold carries no read-only premise"
    );
    let claim = premise(&record, "harness_config.tool_read_only=unproven");
    assert_eq!(claim.class, PremiseClass::Claim);
}
