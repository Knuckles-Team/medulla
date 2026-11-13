//! The `harness_evolution` knobs on the served contrast decision
//! (MEDULLA-DECISION-ENGINE-R179.7): the pinned policy moves the adoption and
//! the sealed record lists every knob the decision read.

use std::time::Instant;

use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
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

const CONDITIONS: &str = "sha256:conditions";
const BASELINE: &str = "trace:baseline";
const GRAPH: &str = "contrast-knobs";

fn q32(fraction: f64) -> i64 {
    (fraction * (1u64 << 32) as f64) as i64
}

fn contrast_schema() -> FeatureSchemaBody {
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

fn option(version: &str, trace: &str, eval: f64, cost: i64) -> DeclaredOption {
    let number = |key: &str, q32: i64| DeclaredNumber {
        key: key.to_string(),
        q32,
    };
    DeclaredOption {
        option_id: version.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![
            number("cost_delta", cost << 32),
            number("eval_score_delta", q32(eval)),
            number("retry_delta", 0),
            number("tool_selection_delta", 0),
        ])
        .unwrap(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "probe_trace_id".to_string(),
            text: trace.to_string(),
        }])
        .unwrap(),
    }
}

fn graph_view(versions: &[&str], traces: &[&str]) -> medulla_graph_core::graph::GraphView {
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
    core.analysis_snapshot()
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn contrast_request(
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

fn decide_contrast(
    h: &Harness,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> StatisticalDecisionRecord {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
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
        "decider".to_string(),
        Instant::now(),
    )
    .unwrap();
    batch.records.as_slice()[0].clone()
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
    let view = graph_view(&[a, b], &[BASELINE, "trace:a", "trace:b"]);
    // a improves slightly more (0.11) but costs more; b is within the default
    // 0.02 epsilon of it (0.10) and is cheaper.
    let options = || vec![option(a, "trace:a", 0.11, 5), option(b, "trace:b", 0.10, 1)];
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
