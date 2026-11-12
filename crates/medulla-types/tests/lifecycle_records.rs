//! Lifecycle record models (MEDULLA-SDLC-R002.1, R002.2, R003.1, R005.1,
//! R006.1, R006.7).

use medulla_decision_types::decision::knobs::{KnobSafety, KnobValue};
use medulla_decision_types::decision::lifecycle::KNOBS;
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_source_types::{
    lifecycle::record_deployment, lifecycle::release_range, lifecycle::Defect,
    lifecycle::DefectIntake, lifecycle::DefectStatus, lifecycle::Deployment,
    lifecycle::DeploymentInput, lifecycle::DeploymentOutcome, lifecycle::LifecycleKind,
    lifecycle::LifecycleTraceRequest, lifecycle::Release, lifecycle::Severity,
    lifecycle::SpecRowState, lifecycle::SpecState, lifecycle::SpecStatusAnswer, lifecycle::Symptom,
    lifecycle::TraceLimits,
};

fn release(version: &str, digests: &[&str]) -> Release {
    Release {
        repo: "medulla".into(),
        version: version.into(),
        git_ref: format!("refs/tags/v{version}"),
        changeset_head: "c3".into(),
        artifact_digests: digests.iter().map(|d| d.to_string()).collect(),
        published_at: 100,
    }
}

fn input(digest: &str, outcome: DeploymentOutcome) -> DeploymentInput {
    DeploymentInput {
        environment: "prod".into(),
        workload: "engine".into(),
        image_digest: digest.into(),
        started_at: 1,
        finished_at: 2,
        outcome,
    }
}

// spec: MEDULLA-SDLC-R002.1
#[test]
fn records_round_trip() {
    let rel = release("1.0.0", &["sha256:aa"]);
    let json = serde_json::to_string(&rel).unwrap();
    assert_eq!(serde_json::from_str::<Release>(&json).unwrap(), rel);

    let dep = record_deployment(&[rel], input("sha256:aa", DeploymentOutcome::RolledBack));
    let json = serde_json::to_string(&dep).unwrap();
    assert!(json.contains("\"rolled_back\""));
    assert_eq!(serde_json::from_str::<Deployment>(&json).unwrap(), dep);

    let bad = json.replace("rolled_back", "exploded");
    assert!(serde_json::from_str::<Deployment>(&bad).is_err());
}

// spec: MEDULLA-SDLC-R002.2
#[test]
fn unknown_digest_stays_unattributed() {
    let releases = [
        release("1.0.0", &["sha256:aa"]),
        release("1.1.0", &["sha256:bb"]),
    ];
    let known = record_deployment(&releases, input("sha256:bb", DeploymentOutcome::Succeeded));
    assert_eq!(known.release_ref.as_deref(), Some("medulla@1.1.0"));
    assert_eq!(known.outcome, DeploymentOutcome::Succeeded);

    let unknown = record_deployment(&releases, input("sha256:zz", DeploymentOutcome::Succeeded));
    assert_eq!(unknown.release_ref, None);
    assert_eq!(unknown.outcome, DeploymentOutcome::Unattributed);

    let chain: Vec<String> = ["c1", "c2", "c3", "c4", "c5"].map(String::from).to_vec();
    assert_eq!(release_range(&chain, None, "c2").unwrap(), ["c1", "c2"]);
    assert_eq!(
        release_range(&chain, Some("c2"), "c5").unwrap(),
        ["c3", "c4", "c5"]
    );
    assert!(release_range(&chain, Some("c5"), "c5").unwrap().is_empty());
    assert_eq!(
        release_range(&chain, None, "nope").unwrap_err().code(),
        "RELEASE_HEAD_UNKNOWN"
    );
}

// spec: MEDULLA-SDLC-R003.1
#[test]
fn defect_round_trips() {
    let defect = Defect {
        id: "def-1".into(),
        repo: "medulla".into(),
        title: "gate failed".into(),
        intake: DefectIntake::GateFailure,
        signature_digest: "ab".repeat(32),
        severity: Severity::High,
        status: DefectStatus::New,
        symptoms: vec![Symptom {
            intake: DefectIntake::GateFailure,
            source_record_id: "gv-1".into(),
            observed_at: 7,
            text: "boom".into(),
        }],
        created_at: 5,
    };
    let json = serde_json::to_string(&defect).unwrap();
    assert!(json.contains("\"gate_failure\"") && json.contains("\"new\""));
    assert_eq!(serde_json::from_str::<Defect>(&json).unwrap(), defect);
    assert!(serde_json::from_str::<Defect>(&json.replace("\"new\"", "\"limbo\"")).is_err());
    assert!(serde_json::from_str::<Defect>(&json.replace("gate_failure", "telepathy")).is_err());
}

// spec: MEDULLA-SDLC-R005.1
#[test]
fn status_answer_round_trips() {
    let answer = SpecStatusAnswer {
        repo: "medulla".into(),
        rev: "c9".into(),
        rows: vec![SpecRowState {
            id: "MEDULLA-X-R001".into(),
            state: SpecState::Verified,
            landed_by: vec!["c1".into()],
            verified_by: vec!["t1".into()],
            withdrawn: false,
        }],
    };
    let json = serde_json::to_string(&answer).unwrap();
    assert!(json.contains("\"VERIFIED\""));
    assert_eq!(
        serde_json::from_str::<SpecStatusAnswer>(&json).unwrap(),
        answer
    );
    assert!(serde_json::from_str::<SpecStatusAnswer>(&json.replace("VERIFIED", "DONE")).is_err());
    assert!(SpecState::Specified < SpecState::Landed && SpecState::Landed < SpecState::Verified);
}

// spec: MEDULLA-SDLC-R006.1
#[test]
fn trace_request_bounds() {
    assert!(LifecycleTraceRequest::new("r".into(), 1, vec![]).is_ok());
    // The ceiling is the lifecycle.trace_max_depth knob, verified by the
    // served handler; decode verifies only the lower bound (MEDULLA-SDLC-R006.7).
    assert!(LifecycleTraceRequest::new("r".into(), 16, vec![LifecycleKind::Defect]).is_ok());
    assert_eq!(
        LifecycleTraceRequest::new("r".into(), 0, vec![])
            .unwrap_err()
            .code(),
        "LIFECYCLE_TRACE_DEPTH"
    );
    assert!(serde_json::from_str::<LifecycleTraceRequest>(r#"{"anchor":"r","depth":0}"#).is_err());
    let ok: LifecycleTraceRequest =
        serde_json::from_str(r#"{"anchor":"r","depth":3,"kinds":["test_case"]}"#).unwrap();
    assert_eq!(ok.kinds, [LifecycleKind::TestCase]);
    assert_eq!(
        LifecycleKind::parse("widget").unwrap_err().code(),
        "LIFECYCLE_KIND_UNKNOWN"
    );
}

// spec: MEDULLA-SDLC-R006.7
#[test]
fn trace_limits_come_from_knobs() {
    let depth = KNOBS
        .iter()
        .find(|spec| spec.name == "lifecycle.trace_max_depth")
        .expect("lifecycle.trace_max_depth knob declared");
    assert_eq!(depth.unit, "levels");
    assert_eq!(depth.default, KnobValue::Integer(8));
    assert_eq!(depth.min, KnobValue::Integer(1));
    assert_eq!(depth.max, KnobValue::Integer(16));
    assert_eq!(depth.safety, KnobSafety::Tunable);

    let budget = KNOBS
        .iter()
        .find(|spec| spec.name == "lifecycle.trace_node_budget")
        .expect("lifecycle.trace_node_budget knob declared");
    assert_eq!(budget.unit, "records");
    assert_eq!(budget.default, KnobValue::Integer(10_000));
    assert_eq!(budget.min, KnobValue::Integer(100));
    assert_eq!(budget.max, KnobValue::Integer(100_000));
    assert_eq!(budget.safety, KnobSafety::Tunable);

    // Decode defaults the policy ref and keeps only the depth lower bound; the
    // knob range is the ceiling the served handler verifies.
    let request: LifecycleTraceRequest =
        serde_json::from_str(r#"{"anchor":"r","depth":3}"#).unwrap();
    assert_eq!(request.policy, DecisionPolicyRef::Default);
    assert!(KNOBS[0].admits(&KnobValue::Integer(3)));
    assert!(!KNOBS[0].admits(&KnobValue::Integer(17)));

    // The walk takes the resolved limits as a value, not a constant.
    let limits = TraceLimits {
        max_depth: 8,
        node_budget: 10_000,
    };
    let json = serde_json::to_string(&limits).unwrap();
    assert_eq!(serde_json::from_str::<TraceLimits>(&json).unwrap(), limits);
}

fn chain(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

// spec: MEDULLA-SDLC-R002.3
#[test]
fn a_release_contains_the_changesets_after_the_previous_release() {
    use medulla_source_types::lifecycle::handle_record_release;

    let first = release("1.0.0", &["sha256:aa"]);
    let mut second = release("1.1.0", &["sha256:bb"]);
    second.changeset_head = "c5".into();
    second.published_at = 200;
    let ids = chain(&["c1", "c2", "c3", "c4", "c5"]);
    let write = handle_record_release(std::slice::from_ref(&first), &ids, second).unwrap();
    assert_eq!(write.contains, chain(&["c4", "c5"]));
    assert!(!write.replayed);
    assert_eq!(write.seal.len(), 64);
}

// spec: MEDULLA-SDLC-R002.4
#[test]
fn unknown_head_and_taken_version_refuse() {
    use medulla_source_types::lifecycle::{handle_record_release, REFUSALS};

    let stored = release("1.0.0", &["sha256:aa"]);
    let ids = chain(&["c1", "c2", "c3"]);

    let mut absent = release("2.0.0", &[]);
    absent.changeset_head = "zz".into();
    let refusal = handle_record_release(std::slice::from_ref(&stored), &ids, absent).unwrap_err();
    assert_eq!(refusal.code(), "RELEASE_HEAD_UNKNOWN");

    let mut taken = stored.clone();
    taken.git_ref = "refs/tags/other".into();
    let refusal = handle_record_release(std::slice::from_ref(&stored), &ids, taken).unwrap_err();
    assert_eq!(refusal.code(), "RELEASE_VERSION_TAKEN");

    for code in [
        "RELEASE_HEAD_UNKNOWN",
        "RELEASE_VERSION_TAKEN",
        "RELEASE_SEALED",
    ] {
        assert!(REFUSALS.contains(&code));
    }
}

// spec: MEDULLA-SDLC-R002.5
#[test]
fn an_identical_release_replays_with_the_same_seal() {
    use medulla_source_types::lifecycle::{handle_record_release, release_seal};

    let stored = release("1.0.0", &["sha256:aa"]);
    let ids = chain(&["c1", "c2", "c3"]);
    let write = handle_record_release(std::slice::from_ref(&stored), &ids, stored.clone()).unwrap();
    assert!(write.replayed);
    assert_eq!(write.seal, release_seal(&stored));
    let mut changed = stored.clone();
    changed.artifact_digests.push("sha256:cc".into());
    assert_ne!(release_seal(&changed), write.seal);
}
