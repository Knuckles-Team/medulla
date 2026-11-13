//! MEDULLA-DECISION-ENGINE-R137.100: a served `InstructionDisclosure` question
//! discloses the latest initial version of each named set before execution, and
//! a runtime refinement only once its trigger has occurred. A stale pin and a
//! set the tenant does not hold refuse with their registered codes.

use std::time::Instant;

use super::super::stat_executor::ExecutionContext;
use super::*;
use medulla_decision_types::decision::instruction_disclosure::{
    instructions_hash, InstructionPhase, InstructionVersion,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance};

const GRAPH: &str = "instructions";
const TRIGGER_A: &str = "eg:trigger/relevance";
const TRIGGER_B: &str = "eg:trigger/timeout";
const OTHER_TENANT: &str = "tenant-other";

fn version(
    tenant: &str,
    set: &str,
    version: u64,
    phase: InstructionPhase,
    trigger: Option<&str>,
) -> InstructionVersion {
    InstructionVersion {
        tenant_id: tenant.to_string(),
        instruction_set_id: set.to_string(),
        version,
        digest: instructions_hash(&format!("{set}-{version}")),
        phase,
        trigger_iri: trigger.map(str::to_string),
    }
}

fn digest_of(set: &str, version: u64) -> String {
    instructions_hash(&format!("{set}-{version}"))
}

fn view_of(versions: &[InstructionVersion]) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    for stored in versions {
        core.add_node(
            stored.node_id(),
            rmp_serde::to_vec_named(&stored.to_node_properties()).unwrap(),
        );
    }
    core.analysis_snapshot()
}

/// Two sets: `set-a` holds two initial versions and two runtime refinements
/// behind different triggers, `set-b` only an initial version.
fn held() -> Vec<InstructionVersion> {
    use InstructionPhase::{Initial, Runtime};
    vec![
        version(TENANT, "set-a", 1, Initial, None),
        version(TENANT, "set-a", 2, Initial, None),
        version(TENANT, "set-a", 3, Runtime, Some(TRIGGER_A)),
        version(TENANT, "set-a", 4, Runtime, Some(TRIGGER_B)),
        version(TENANT, "set-b", 1, Initial, None),
    ]
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn disclosure_request(
    h: &Harness,
    phase: &str,
    triggers: &[&str],
    sets: &[&str],
    pinned: Option<serde_json::Value>,
) -> DecideRequest {
    let (schema, _) = publish_route_schema(h);
    let mut params = vec![
        text("graph", GRAPH),
        text("phase", phase),
        text("run_id", "run-1"),
        TypedParam {
            name: "sequence".to_string(),
            value: TypedValue::Int(7),
        },
        text("instruction_set_ids", &serde_json::json!(sets).to_string()),
    ];
    if !triggers.is_empty() {
        params.push(TypedParam {
            name: "occurred_triggers".to_string(),
            value: TypedValue::IriList(
                medulla_types::contract::BoundedVec::new(
                    triggers.iter().map(|t| t.to_string()).collect(),
                )
                .unwrap(),
            ),
        });
    }
    if let Some(pinned) = pinned {
        params.push(text("pinned_digests", &pinned.to_string()));
    }
    declared_request(
        &schema,
        TENANT,
        QuestionKind::InstructionDisclosure,
        "disclosure",
        DecisionPolicyRef::Default,
        params,
    )
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

fn disclosed(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an Advisory outcome: {:?}", record.outcome)
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

fn fact<'a>(
    record: &'a StatisticalDecisionRecord,
    fact: &str,
) -> Option<&'a medulla_decision_types::decision::PremiseRef> {
    record.premises.iter().find(|p| p.fact == fact)
}

// spec: MEDULLA-DECISION-ENGINE-R137.100
#[tokio::test]
async fn initial_then_runtime_disclosure() {
    let h = Harness::new().await;
    let view = view_of(&held());
    let sets = ["set-a", "set-b"];

    // The initial phase discloses the latest initial version of each set; the
    // runtime refinements are withheld.
    let record = decide_view(
        &h,
        &disclosure_request(&h, "initial", &[], &sets, None),
        &view,
    )
    .unwrap();
    assert_eq!(
        disclosed(&record),
        vec![
            format!("set-a:2:{}", digest_of("set-a", 2)),
            format!("set-b:1:{}", digest_of("set-b", 1)),
        ]
    );
    assert_eq!(record.question.kind, QuestionKind::InstructionDisclosure);
    assert!(fact(&record, "instruction_disclosure.phase=initial").is_some());
    assert!(fact(&record, "instruction_disclosure.trigger=none").is_some());
    let version = fact(
        &record,
        &format!(
            "instruction_disclosure.version:set-a=2:{}",
            digest_of("set-a", 2)
        ),
    )
    .expect("a premise per selected version");
    assert_eq!(version.class, PremiseClass::Observation);
    assert_eq!(
        version.provenance,
        PremiseProvenance::Observation {
            evaluation_id: "instruction-version:set-a:2".to_string()
        }
    );

    // A runtime question before any trigger occurred discloses nothing: the
    // refinements stay withheld.
    let record = decide_view(
        &h,
        &disclosure_request(&h, "runtime", &[], &sets, None),
        &view,
    )
    .unwrap();
    assert!(disclosed(&record).is_empty());

    // The first trigger releases only its own refinement.
    let record = decide_view(
        &h,
        &disclosure_request(&h, "runtime", &[TRIGGER_A], &sets, None),
        &view,
    )
    .unwrap();
    assert_eq!(
        disclosed(&record),
        vec![format!("set-a:3:{}", digest_of("set-a", 3))]
    );
    assert!(fact(&record, "instruction_disclosure.phase=runtime").is_some());
    assert!(fact(
        &record,
        &format!("instruction_disclosure.trigger={TRIGGER_A}")
    )
    .is_some());
    assert!(fact(
        &record,
        &format!("instruction_disclosure.trigger={TRIGGER_B}")
    )
    .is_none());

    // With both triggers the latest triggered refinement wins.
    let record = decide_view(
        &h,
        &disclosure_request(&h, "runtime", &[TRIGGER_A, TRIGGER_B], &sets, None),
        &view,
    )
    .unwrap();
    assert_eq!(
        disclosed(&record),
        vec![format!("set-a:4:{}", digest_of("set-a", 4))]
    );

    // A trigger no version waits for releases nothing.
    let record = decide_view(
        &h,
        &disclosure_request(&h, "runtime", &["eg:trigger/unknown"], &sets, None),
        &view,
    )
    .unwrap();
    assert!(disclosed(&record).is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R137.100
#[tokio::test]
async fn stale_or_foreign_version_refuses() {
    use InstructionPhase::Initial;
    let h = Harness::new().await;
    let mut versions = held();
    // Another tenant holds a newer version of set-a and a set of its own.
    versions.push(version(OTHER_TENANT, "set-a", 9, Initial, None));
    versions.push(version(OTHER_TENANT, "set-foreign", 1, Initial, None));
    let view = view_of(&versions);

    // A pin on the digest of an older version is stale; the latest is disclosed
    // only when the pin matches it.
    let stale = decide_view(
        &h,
        &disclosure_request(
            &h,
            "initial",
            &[],
            &["set-a"],
            Some(serde_json::json!({"set-a": digest_of("set-a", 1)})),
        ),
        &view,
    )
    .unwrap_err();
    assert!(stale.starts_with("DISCLOSURE_VERSION_STALE"), "{stale}");
    let fresh = decide_view(
        &h,
        &disclosure_request(
            &h,
            "initial",
            &[],
            &["set-a"],
            Some(serde_json::json!({"set-a": digest_of("set-a", 2)})),
        ),
        &view,
    )
    .unwrap();
    assert_eq!(
        disclosed(&fresh),
        vec![format!("set-a:2:{}", digest_of("set-a", 2))],
        "another tenant's newer version is never disclosed"
    );

    // A set only another tenant holds, and a set nobody holds, refuse alike.
    let foreign = decide_view(
        &h,
        &disclosure_request(&h, "initial", &[], &["set-foreign"], None),
        &view,
    )
    .unwrap_err();
    let unknown = decide_view(
        &h,
        &disclosure_request(&h, "initial", &[], &["set-nobody"], None),
        &view,
    )
    .unwrap_err();
    assert!(foreign.starts_with("DISCLOSURE_UNAUTHORIZED"), "{foreign}");
    assert!(unknown.starts_with("DISCLOSURE_UNAUTHORIZED"), "{unknown}");
    assert_eq!(
        foreign.replace("set-foreign", "SET"),
        unknown.replace("set-nobody", "SET"),
        "a foreign set and a missing set answer the same way"
    );
}
