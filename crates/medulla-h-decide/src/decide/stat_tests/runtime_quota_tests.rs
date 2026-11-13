//! MEDULLA-DECISION-ENGINE-R133.3 to R133.5: a served `RuntimeQuota` question
//! answers the quota one run gets. The knobs are the defaults, a task class in
//! the caller's tenant graph overrides them (clamped to the ceiling), and a
//! request only tightens. A request above a ceiling refuses before the task
//! class is looked up; another tenant's task class is never read.

use std::time::Instant;

use super::super::stat_executor::ExecutionContext;
use super::*;
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, ResolutionKind};

const CLASS: &str = "task-class:build";
const OTHER_TENANT: &str = "tenant-other";

fn int(name: &str, value: i64) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Int(value),
    }
}

fn class_param() -> TypedParam {
    TypedParam {
        name: "task_class".to_string(),
        value: TypedValue::Iri(CLASS.to_string()),
    }
}

fn quota_request(h: &Harness, params: Vec<TypedParam>) -> DecideRequest {
    let (schema, _) = publish_route_schema(h);
    declared_request(
        &schema,
        TENANT,
        QuestionKind::RuntimeQuota,
        "quota",
        DecisionPolicyRef::Default,
        params,
    )
}

fn options(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an Advisory outcome: {:?}", record.outcome)
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

fn answer(wall: u64, memory: u64, processes: u64, grace: u64) -> Vec<String> {
    vec![
        format!("wall_time_ms:{wall}"),
        format!("memory_bytes:{memory}"),
        format!("process_count:{processes}"),
        format!("grace_ms:{grace}"),
    ]
}

fn defaults() -> Vec<String> {
    answer(600_000, 2_147_483_648, 32, 10_000)
}

/// A graph holding one task class node with the given properties.
fn view_with(properties: serde_json::Value) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    core.add_node(
        CLASS.to_string(),
        rmp_serde::to_vec_named(&properties).unwrap(),
    );
    core.analysis_snapshot()
}

fn decide_view(
    h: &Harness,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        Some(view),
        "decider".to_string(),
        Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

fn fact<'a>(
    record: &'a StatisticalDecisionRecord,
    fact: &str,
) -> Option<&'a medulla_decision_types::decision::PremiseRef> {
    record.premises.iter().find(|p| p.fact == fact)
}

// spec: MEDULLA-DECISION-ENGINE-R133.3
#[tokio::test]
async fn decide_routes_runtime_quota() {
    let h = Harness::new().await;

    // No request and no task class: the knob defaults, served as a statistical answer.
    let batch = decide(&h, quota_request(&h, vec![])).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::RuntimeQuota);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(options(record), defaults());

    // A request below the default tightens that field only.
    let batch = decide(
        &h,
        quota_request(&h, vec![int("requested_wall_time_ms", 5_000)]),
    )
    .await
    .unwrap();
    assert_eq!(
        options(&batch.records.as_slice()[0]),
        answer(5_000, 2_147_483_648, 32, 10_000)
    );

    // A request above the default but under the ceiling never raises it.
    let batch = decide(
        &h,
        quota_request(&h, vec![int("requested_process_count", 100)]),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch.records.as_slice()[0]), defaults());

    // A task class overrides the defaults; an override above its ceiling clamps to it.
    let view = view_with(serde_json::json!({
        "defaultWallTimeMs": 120_000,
        "defaultMemoryBytes": 99_999_999_999u64,
        "defaultProcessCount": 8,
    }));
    let record = decide_view(&h, &quota_request(&h, vec![class_param()]), &view).unwrap();
    assert_eq!(options(&record), answer(120_000, 8_589_934_592, 8, 10_000));
    // A request still only tightens the class default.
    let record = decide_view(
        &h,
        &quota_request(
            &h,
            vec![
                class_param(),
                int("requested_wall_time_ms", 60_000),
                int("requested_process_count", 16),
            ],
        ),
        &view,
    )
    .unwrap();
    assert_eq!(options(&record), answer(60_000, 8_589_934_592, 8, 10_000));
}

// spec: MEDULLA-DECISION-ENGINE-R133.3
#[tokio::test]
async fn cross_tenant_task_class_is_not_read() {
    let h = Harness::new().await;
    let foreign = view_with(serde_json::json!({
        "tenant_id": OTHER_TENANT,
        "defaultWallTimeMs": 1_000,
        "defaultProcessCount": 1,
    }));
    let record = decide_view(&h, &quota_request(&h, vec![class_param()]), &foreign).unwrap();
    assert_eq!(options(&record), defaults());
    assert!(fact(&record, "runtime_quota.missing:task_class").is_some());

    // The same node owned by the caller's tenant is read.
    let own = view_with(serde_json::json!({
        "tenant_id": TENANT,
        "defaultWallTimeMs": 1_000,
    }));
    let record = decide_view(&h, &quota_request(&h, vec![class_param()]), &own).unwrap();
    assert_eq!(options(&record)[0], "wall_time_ms:1000");
    assert!(fact(&record, "runtime_quota.missing:task_class").is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R133.4
#[tokio::test]
async fn above_ceiling_refuses_before_task_lookup() {
    let h = Harness::new().await;
    for (name, value) in [
        ("requested_wall_time_ms", 3_600_001),
        ("requested_memory_bytes", 8_589_934_593),
        ("requested_process_count", 129),
        ("requested_grace_ms", 60_001),
    ] {
        // No task class: the ceiling alone refuses.
        let refused = decide(&h, quota_request(&h, vec![int(name, value)]))
            .await
            .unwrap_err();
        assert!(refused.contains("PARAMETER_INVALID"), "{refused}");
        assert!(refused.contains("QUOTA_ABOVE_CEILING"), "{refused}");
        assert!(
            refused.contains(&name.replace("requested_", "")),
            "{refused}"
        );

        // A task class with no graph to read: the ceiling still refuses first,
        // so the lookup is never attempted.
        let refused = decide(&h, quota_request(&h, vec![class_param(), int(name, value)]))
            .await
            .unwrap_err();
        assert!(refused.contains("QUOTA_ABOVE_CEILING"), "{refused}");
    }
    // The ceiling itself is accepted.
    let batch = decide(
        &h,
        quota_request(&h, vec![int("requested_wall_time_ms", 3_600_000)]),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch.records.as_slice()[0]), defaults());
}

// spec: MEDULLA-DECISION-ENGINE-R133.4
#[tokio::test]
async fn missing_task_class_answers_defaults() {
    let h = Harness::new().await;
    // Named, but the graph holds no such node.
    let empty = medulla_graph_core::graph::GraphCore::new().analysis_snapshot();
    let record = decide_view(&h, &quota_request(&h, vec![class_param()]), &empty).unwrap();
    assert_eq!(options(&record), defaults());
    let missing = fact(&record, "runtime_quota.missing:task_class").unwrap();
    assert_eq!(missing.class, PremiseClass::Claim);
    assert_eq!(
        record.evidence_class,
        medulla_decision_types::decision::EvidenceClass::Claim
    );

    // No task class named at all: the same answer and the same premise.
    let batch = decide(&h, quota_request(&h, vec![])).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(options(record), defaults());
    assert!(fact(record, "runtime_quota.missing:task_class").is_some());
}

// spec: MEDULLA-DECISION-ENGINE-R133.5
#[tokio::test]
async fn premises_name_each_value_and_default_source() {
    let h = Harness::new().await;
    let view = view_with(serde_json::json!({ "defaultMemoryBytes": 1_073_741_824u64 }));
    let record = decide_view(
        &h,
        &quota_request(
            &h,
            vec![class_param(), int("requested_wall_time_ms", 5_000)],
        ),
        &view,
    )
    .unwrap();

    for (name, value) in [
        ("wall_time_ms", 5_000u64),
        ("memory_bytes", 1_073_741_824),
        ("process_count", 32),
        ("grace_ms", 10_000),
    ] {
        let decided = fact(&record, &format!("runtime_quota.{name}={value}"))
            .unwrap_or_else(|| panic!("no decided premise for {name}"));
        assert_eq!(decided.class, PremiseClass::Definition);
        assert!(matches!(
            decided.provenance,
            PremiseProvenance::Policy { .. }
        ));
    }
    let asked = fact(&record, "runtime_quota.requested_wall_time_ms=5000").unwrap();
    assert_eq!(asked.class, PremiseClass::Claim);
    assert!(matches!(
        asked.provenance,
        PremiseProvenance::Request { .. }
    ));
    // Only the supplied request value has a request premise.
    assert!(!record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("runtime_quota.requested_memory_bytes")));

    let source = |name: &str, from: &str| {
        fact(
            &record,
            &format!("runtime_quota.default_source:{name}={from}"),
        )
        .unwrap_or_else(|| panic!("no default-source premise for {name}={from}"))
    };
    assert_eq!(
        source("memory_bytes", "task_class").class,
        PremiseClass::Observation
    );
    assert_eq!(
        source("wall_time_ms", "knob").class,
        PremiseClass::Definition
    );
    assert_eq!(
        source("process_count", "knob").class,
        PremiseClass::Definition
    );
    assert_eq!(source("grace_ms", "knob").class, PremiseClass::Definition);
    // The task class was found, so nothing is missing.
    assert!(fact(&record, "runtime_quota.missing:task_class").is_none());
    // The knob reads are premises bound to the policy (R179.5).
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "knob:quota.wall_time_ms_default=600000ms"));
}
