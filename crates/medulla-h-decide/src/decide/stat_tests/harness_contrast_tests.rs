//! The `harness_evolution` knobs on the served contrast decision
//! (MEDULLA-DECISION-ENGINE-R179.7): the pinned policy moves the adoption and
//! the sealed record lists every knob the decision read.

use std::time::Instant;

use medulla_decision_types::decision::harness_contrast::HarnessContrastAbstainReason;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::{DeclaredOption, DeclaredText};
use medulla_decision_types::decision::statistical::features::{
    FeatureSchemaBody, FEATURE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalDecisionRecord,
    StatisticalOutcome, StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, DecisionPolicyRef};
use medulla_types::contract::BoundedVec;

use super::super::stat_executor::ExecutionContext;
use super::*;
use medulla_decision_types::decision::program_store::{RunRecord, RunRecordStored};
use medulla_decision_types::decision::run_governance::{
    RunOutcome, RunTraceRecord, StepKind, StepOutcome, TraceStep,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

pub(super) const CONDITIONS: &str = "sha256:conditions";
pub(super) const BASELINE: &str = "trace:baseline";
pub(super) const GRAPH: &str = "contrast-knobs";

pub(super) fn contrast_schema() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(
            [
                "cost_delta",
                "eval_score_delta",
                "retry_delta",
                "tool_selection_delta",
            ]
            .into_iter()
            .map(number_feature)
            .collect(),
        )
        .unwrap(),
    }
}

pub(super) fn option(version: &str, trace: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: version.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "probe_trace_id".to_string(),
            text: trace.to_string(),
        }])
        .unwrap(),
    }
}

/// The evidence graph: the held versions, the probe runs and the `:Score`
/// rows (`run`, `value` on a 0..1 range, grader kind) the caller ingested.
pub(super) fn graph_view(
    versions: &[&str],
    traces: &[&str],
    scores: &[(&str, f64, &str)],
) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    let put = |id: &str, props: serde_json::Value| {
        core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    };
    for version in versions {
        put(version, serde_json::json!({ "_harness_component": true }));
    }
    for trace in traces {
        put(
            &format!("harness-probe-run:{trace}"),
            serde_json::json!({
                "_harness_probe_run": true,
                "trace_id": trace,
                "conditions_digest": CONDITIONS,
            }),
        );
    }
    let mut runs: Vec<&str> = scores.iter().map(|(run, _, _)| *run).collect();
    runs.sort();
    runs.dedup();
    for run in runs {
        put(run, serde_json::json!({ "node_type": "ExperimentRun" }));
    }
    for (index, (run, value, grader)) in scores.iter().enumerate() {
        let score = format!("score:{index}");
        put(
            &score,
            serde_json::json!({
                "node_type": "Score",
                "value": value,
                "range_min": 0.0,
                "range_max": 1.0,
                "grader_kind": grader,
            }),
        );
        // `scoresRun` is an object property: an edge from the score to its run.
        core.add_typed_edge(
            score,
            run.to_string(),
            "scoresRun".to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
        )
        .unwrap();
    }
    core.analysis_snapshot()
}

pub(super) fn trace_record(
    run_id: &str,
    steps: &[(StepKind, &str, StepOutcome)],
) -> RunTraceRecord {
    RunTraceRecord {
        schema_version: 1,
        run_id: run_id.to_string(),
        task_signature: "sig".to_string(),
        model_id: "model".to_string(),
        steps: BoundedVec::new(
            steps
                .iter()
                .enumerate()
                .map(|(index, (kind, action, outcome))| TraceStep {
                    index: index as u32,
                    kind: *kind,
                    action_ref: action.to_string(),
                    outcome: *outcome,
                    arguments: BoundedVec::default(),
                })
                .collect(),
        )
        .unwrap(),
        outcome: RunOutcome::Succeeded,
        started_at_ms: 1,
    }
}

/// Commit one run's trace through the served `DecisionLog` as `who` in `TENANT`.
pub(super) async fn commit_trace(h: &Harness, who: &str, trace: RunTraceRecord) {
    commit_trace_in(h, TENANT, who, trace).await;
}

/// Commit one run's trace through the served `DecisionLog` as `who`, whose
/// verified context names `tenant`.
async fn commit_trace_in(h: &Harness, tenant: &str, who: &str, trace: RunTraceRecord) {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, tenant);
    let stored = decode::<RunRecordStored>(
        super::super::log::handle_decision_log(
            &h.state,
            9,
            &verified,
            DecisionLogOp::RunRecord {
                tenant_id: tenant.to_string(),
                record: RunRecord::Trace { trace },
            },
        )
        .await,
    );
    assert!(stored.is_ok(), "{stored:?}");
}

/// Append priced usage for each run through the served `Usage` method:
/// `cost` whole micro-units per run.
pub(super) async fn ledger(h: &Harness, tenant: &str, costs: &[(&str, u64)]) {
    let service =
        VerifiedRequestContext::verified_for_test_with_scopes("svc", tenant, &["usage:write"]);
    let price = medulla_types::usage::PriceEntry {
        schema_version: medulla_types::usage::USAGE_SCHEMA_VERSION,
        model_key: "m1".into(),
        valid_from_ms: 0,
        in_per_mtok_micro: 1,
        out_per_mtok_micro: 0,
        cached_per_mtok_micro: 0,
    };
    let put = crate::usage::handle_usage(
        &h.state,
        1,
        &service,
        medulla_types::usage::UsageOp::PutPrice {
            catalog: medulla_types::usage::PriceCatalog {
                aliases: Default::default(),
                entries: vec![price],
            },
        },
    )
    .await;
    assert!(put.error.is_none(), "{:?}", put.refusal_text());
    let records = costs
        .iter()
        .map(|(run, cost)| medulla_types::usage::UsageRecord {
            schema_version: medulla_types::usage::USAGE_SCHEMA_VERSION,
            recorded_at_ms: 1_000,
            child_id: "child".into(),
            operation: "probe".into(),
            tenant: tenant.into(),
            principal: "principal:svc".into(),
            run_id: run.to_string(),
            model_key: "m1".into(),
            tokens_in: cost * 1_000_000,
            tokens_out: 0,
            tokens_cached: 0,
            latency_ms: 1,
            outcome: medulla_types::usage::UsageOutcome::Ok,
            content_digest: "a".repeat(64),
        })
        .collect();
    let appended = crate::usage::handle_usage(
        &h.state,
        2,
        &service,
        medulla_types::usage::UsageOp::Append { records },
    )
    .await;
    assert!(appended.error.is_none(), "{:?}", appended.refusal_text());
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

pub(super) fn contrast_request(
    schema_pin: &ComponentDependency,
    policy: DecisionPolicyRef,
    options: Vec<DeclaredOption>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "harness.contrast".to_string(),
            kind: QuestionKind::HarnessContrast,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema_pin.clone(),
        head: None,
        policy,
        params: BoundedVec::new(vec![
            text("task_ref", "task:deploy"),
            TypedParam {
                name: "task_class".to_string(),
                value: TypedValue::Iri("eg:taskclass/deploy".to_string()),
            },
            text("conditions_digest", CONDITIONS),
            text("baseline_trace_id", BASELINE),
            text("graph", GRAPH),
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

pub(super) fn decide_contrast(
    h: &Harness,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> StatisticalDecisionRecord {
    decide_contrast_in(h, TENANT, request, view).unwrap()
}

fn decide_contrast_in(
    h: &Harness,
    tenant: &str,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
        reader: None,
    };
    let batch = super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        Some(view),
        // The served path records the verified principal's persistence id.
        VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT)
            .principal_persistence_id(),
        Instant::now(),
    )?;
    Ok(batch.records.as_slice()[0].clone())
}

fn adopted(record: &StatisticalDecisionRecord) -> Option<String> {
    match &record.outcome {
        StatisticalOutcome::Explored { option_id, .. } => Some(option_id.clone()),
        _ => None,
    }
}

fn knob_facts(record: &StatisticalDecisionRecord) -> Vec<String> {
    record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("knob:harness_evolution."))
        .map(|p| p.fact.clone())
        .collect()
}

/// Verifies MEDULLA-DECISION-ENGINE-R179.7: Harness evolution knobs.
// spec: MEDULLA-DECISION-ENGINE-R179.7
#[tokio::test]
async fn contrast_reads_the_pinned_harness_evolution_knobs() {
    let h = Harness::new().await;
    let schema_pin = h
        .publish_test_component(
            "schema-contrast",
            AgentComponentKind::FeatureSchema,
            "contrast deltas",
            Some(&contrast_schema()),
            None,
        )
        .unwrap();
    let (a, b) = ("harness-component:prompt:a", "harness-component:prompt:b");
    let view = graph_view(
        &[a, b],
        &[BASELINE, "trace:a", "trace:b"],
        &[
            (BASELINE, 0.5, "human"),
            ("trace:a", 0.61, "human"),
            ("trace:b", 0.60, "human"),
        ],
    );
    let run = [(StepKind::ToolCall, "t", StepOutcome::Ok)];
    for id in [BASELINE, "trace:a", "trace:b"] {
        commit_trace(&h, "decider", trace_record(id, &run)).await;
    }
    ledger(&h, TENANT, &[(BASELINE, 0), ("trace:a", 5), ("trace:b", 1)]).await;
    // a improves slightly more (0.11) but costs more; b is within the default
    // 0.02 epsilon of it (0.10) and is cheaper.
    let options = || vec![option(a, "trace:a"), option(b, "trace:b")];
    let pinned = |name: &str, key: &str, value: KnobValue| {
        let mut policy = DecisionPolicy::engine_default();
        policy.knobs.insert(KnobName::new(key), value);
        DecisionPolicyRef::Pinned {
            component: h.publish_policy(name, &policy),
        }
    };

    let default = decide_contrast(
        &h,
        &contrast_request(&schema_pin, DecisionPolicyRef::Default, options()),
        &view,
    );
    assert_eq!(adopted(&default).as_deref(), Some(b));
    let facts = knob_facts(&default);
    assert_eq!(facts.len(), 4, "one premise per harness_evolution knob");
    assert!(facts.contains(&"knob:harness_evolution.cost_tie_epsilon=0.020000".to_string()));
    assert!(facts.contains(&"knob:harness_evolution.min_candidates=2".to_string()));

    let strict = decide_contrast(
        &h,
        &contrast_request(
            &schema_pin,
            pinned(
                "policy-eps-zero",
                "harness_evolution.cost_tie_epsilon",
                KnobValue::DecimalMicros(0),
            ),
            options(),
        ),
        &view,
    );
    assert_eq!(adopted(&strict).as_deref(), Some(a));
    assert!(knob_facts(&strict)
        .contains(&"knob:harness_evolution.cost_tie_epsilon=0.000000".to_string()));

    let high = decide_contrast(
        &h,
        &contrast_request(
            &schema_pin,
            pinned(
                "policy-min-delta",
                "harness_evolution.min_eval_delta",
                KnobValue::DecimalMicros(150_000),
            ),
            options(),
        ),
        &view,
    );
    assert_eq!(adopted(&high), None);
    assert!(matches!(high.outcome, StatisticalOutcome::Abstained { .. }));
    assert!(
        knob_facts(&high).contains(&"knob:harness_evolution.min_eval_delta=0.150000".to_string())
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R803.7: the engine measures each delta
/// from the stored traces, the `:Score` rows and the usage ledger.
// spec: MEDULLA-DECISION-ENGINE-R803.7
#[tokio::test]
async fn contrast_is_measured_from_stored_traces() {
    let h = Harness::new().await;
    let schema_pin = h
        .publish_test_component(
            "schema-contrast-measured",
            AgentComponentKind::FeatureSchema,
            "contrast deltas",
            Some(&contrast_schema()),
            None,
        )
        .unwrap();
    let (a, b) = ("harness-component:prompt:a", "harness-component:prompt:b");
    let ok = |action| (StepKind::ToolCall, action, StepOutcome::Ok);
    let failed = |action| (StepKind::ToolCall, action, StepOutcome::Error);
    // The baseline retries one failed call; a is clean and selects more tools.
    commit_trace(
        &h,
        "decider",
        trace_record(BASELINE, &[failed("t"), ok("t")]),
    )
    .await;
    commit_trace(&h, "decider", trace_record("trace:a", &[ok("t"), ok("u")])).await;
    // b's trace was committed by another principal and is absent to the decider.
    commit_trace(&h, "stranger", trace_record("trace:b", &[ok("t")])).await;
    ledger(&h, TENANT, &[(BASELINE, 9), ("trace:a", 4), ("trace:b", 1)]).await;
    let view = graph_view(
        &[a, b],
        &[BASELINE, "trace:a", "trace:b"],
        &[
            (BASELINE, 0.5, "human"),
            (BASELINE, 0.7, "human"),
            ("trace:a", 0.8, "api"),
            // A model judge's score never counts as independent evidence.
            ("trace:a", 0.1, "model_judge"),
            ("trace:b", 0.9, "human"),
        ],
    );
    let request = |versions: Vec<DeclaredOption>| {
        contrast_request(&schema_pin, DecisionPolicyRef::Default, versions)
    };

    // b's trace is absent: the contrast abstains insufficient_evidence and
    // records no measured number for it.
    let abstained = decide_contrast(
        &h,
        &request(vec![option(a, "trace:a"), option(b, "trace:b")]),
        &view,
    );
    assert!(matches!(
        abstained.outcome,
        StatisticalOutcome::Abstained { .. }
    ));

    // Over a alone and a second held version probed by the same trace, the
    // adoption is a, decided by the measured deltas only.
    let both = decide_contrast(
        &h,
        &request(vec![option(a, "trace:a"), option(b, "trace:a")]),
        &view,
    );
    let decision = both
        .inputs
        .harness_contrast
        .expect("the contrast decision is sealed");
    let row = decision
        .candidate_version_refs
        .as_slice()
        .iter()
        .position(|id| id == a)
        .expect("a is a candidate");
    let values = medulla_decision_types::decision::harness_contrast::matrix_values(&decision);
    let width = medulla_decision_types::decision::harness_contrast::CONTRAST_FEATURE_NAMES.len();
    let names = medulla_decision_types::decision::harness_contrast::CONTRAST_FEATURE_NAMES;
    let cell = |name: &str| values[row * width + names.iter().position(|n| *n == name).unwrap()];
    assert_eq!(cell("retry_delta"), -(1i64 << 32));
    assert_eq!(cell("tool_selection_delta"), 1i64 << 32);
    assert_eq!(cell("cost_delta"), -5 * (1i64 << 32));
    assert_eq!(
        cell("eval_score_delta"),
        ((200_000i128 << 32) / 1_000_000) as i64
    );

    // A stored probe run holding another conditions digest refuses.
    let mut mismatched = view.clone();
    mismatched.overlay_remove_node("harness-probe-run:trace:a");
    mismatched.overlay_add_node(
        "harness-probe-run:trace:a".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({
            "_harness_probe_run": true,
            "trace_id": "trace:a",
            "conditions_digest": "sha256:other",
        }))
        .unwrap(),
    );
    let refused = decide_contrast_in(
        &h,
        TENANT,
        &request(vec![option(a, "trace:a"), option(b, "trace:a")]),
        &mismatched,
    )
    .unwrap_err();
    assert!(
        refused.contains("HARNESS_CONTRAST_TRACE_MISMATCH"),
        "{refused}"
    );
}

/// `request` with its baseline trace param replaced by `baseline`.
fn with_baseline(mut request: DecideRequest, baseline: &str) -> DecideRequest {
    let params: Vec<TypedParam> = request
        .params
        .iter()
        .map(|param| {
            if param.name == "baseline_trace_id" {
                text("baseline_trace_id", baseline)
            } else {
                param.clone()
            }
        })
        .collect();
    request.params = BoundedVec::new(params).unwrap();
    request
}

/// Verifies MEDULLA-DECISION-ENGINE-R803.7: two tenants share one store and
/// each seeds its own traces, `:Score` rows and usage ledger. The verified
/// tenant is the only tenant the contrast reads, so tenant B asking about
/// tenant A's run ids gets exactly the answer it gets for ids that exist
/// nowhere: an `insufficient_evidence` abstention with no measured number.
// spec: MEDULLA-DECISION-ENGINE-R803.7
#[tokio::test]
async fn contrast_never_reads_another_tenants_runs() {
    const OTHER: &str = "tenant-other";
    let h = Harness::new().await;
    let schema_pin = h
        .publish_test_component(
            "schema-contrast-tenants",
            AgentComponentKind::FeatureSchema,
            "contrast deltas",
            Some(&contrast_schema()),
            None,
        )
        .unwrap();
    // Tenant B pins the same feature schema in its own library: a pin resolves
    // only inside the tenant that asks.
    let other_pin = {
        let encoded = encode_body(&contrast_schema()).unwrap();
        let mut draft = test_component_draft(OTHER, "schema-contrast-tenants");
        draft.kind = AgentComponentKind::FeatureSchema;
        draft.summary = "contrast deltas".to_string();
        draft.classification = vec!["eg:capability/retrieval/web-search".to_string()];
        draft.content_digest = encoded.content_digest;
        draft.attributes = encoded.attributes;
        h.commit_in(OTHER, draft, None).unwrap()
    };
    let (a, b) = ("harness-component:prompt:a", "harness-component:prompt:b");
    let ok = |action| (StepKind::ToolCall, action, StepOutcome::Ok);
    let failed = |action| (StepKind::ToolCall, action, StepOutcome::Error);

    // Tenant A: a baseline and a probe run, scored and priced.
    commit_trace(
        &h,
        "decider",
        trace_record(BASELINE, &[failed("t"), ok("t")]),
    )
    .await;
    commit_trace(&h, "decider", trace_record("trace:a", &[ok("t")])).await;
    ledger(&h, TENANT, &[(BASELINE, 9), ("trace:a", 4)]).await;
    let view_a = graph_view(
        &[a, b],
        &[BASELINE, "trace:a"],
        &[(BASELINE, 0.5, "human"), ("trace:a", 0.8, "human")],
    );

    // Tenant B: its own baseline and probe run in the same store.
    let (other_base, other_probe) = ("trace:other-base", "trace:other-a");
    commit_trace_in(
        &h,
        OTHER,
        "decider",
        trace_record(other_base, &[failed("t"), ok("t")]),
    )
    .await;
    commit_trace_in(&h, OTHER, "decider", trace_record(other_probe, &[ok("t")])).await;
    ledger(&h, OTHER, &[(other_base, 7), (other_probe, 2)]).await;
    let view_b = graph_view(
        &[a, b],
        &[other_base, other_probe],
        &[(other_base, 0.4, "human"), (other_probe, 0.9, "human")],
    );

    let ask_with = |pin: &ComponentDependency, baseline: &str, probe: &str| {
        with_baseline(
            contrast_request(
                pin,
                DecisionPolicyRef::Default,
                vec![option(a, probe), option(b, probe)],
            ),
            baseline,
        )
    };
    let ask = |baseline: &str, probe: &str| ask_with(&schema_pin, baseline, probe);
    let ask_b = |baseline: &str, probe: &str| ask_with(&other_pin, baseline, probe);

    // Each tenant reads its own evidence.
    let own_a = decide_contrast_in(&h, TENANT, &ask(BASELINE, "trace:a"), &view_a).unwrap();
    assert!(own_a.inputs.harness_contrast.is_some());
    let own_b = decide_contrast_in(&h, OTHER, &ask_b(other_base, other_probe), &view_b).unwrap();
    assert!(own_b.inputs.harness_contrast.is_some());

    // Ids that exist nowhere abstain insufficient_evidence.
    let unknown = decide_contrast_in(
        &h,
        OTHER,
        &ask_b("trace:nowhere-base", "trace:nowhere-a"),
        &view_b,
    )
    .unwrap();
    assert!(
        matches!(unknown.outcome, StatisticalOutcome::Abstained { .. }),
        "unknown ids must abstain: {:?}",
        unknown.outcome
    );
    let abstention = |record: &StatisticalDecisionRecord| {
        let decision = record
            .inputs
            .harness_contrast
            .as_ref()
            .expect("an abstained contrast is sealed with its decision");
        assert!(
            decision.probe_trace_ids.iter().all(Option::is_none),
            "no probe may be counted as measured"
        );
        decision.abstain_reason
    };
    assert_eq!(
        abstention(&unknown),
        Some(HarnessContrastAbstainReason::InsufficientEvidence)
    );

    // Tenant B asking about tenant A's run ids gets the same answer: nothing
    // of A's traces, scores or ledger is visible.
    let foreign = decide_contrast_in(&h, OTHER, &ask_b(BASELINE, "trace:a"), &view_b).unwrap();
    assert_eq!(foreign.outcome, unknown.outcome);
    assert_eq!(abstention(&foreign), abstention(&unknown));
}
