//! MEDULLA-DECISION-ENGINE-R137.100: a served `InstructionDisclosure` question
//! discloses the latest initial version of each named set before execution, and
//! a runtime refinement only once its trigger has occurred. A stale pin and a
//! set the tenant does not hold refuse with their registered codes.

use std::time::Instant;

use super::super::stat_executor::ExecutionContext;
use super::*;
use medulla_decision_types::decision::instruction_disclosure::{
    instructions_hash, DisclosureTiming, InstructionVersion,
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
    timing: DisclosureTiming,
    trigger: Option<&str>,
) -> InstructionVersion {
    InstructionVersion {
        tenant_id: tenant.to_string(),
        instruction_set_id: set.to_string(),
        version,
        digest: instructions_hash(&format!("{set}-{version}")),
        timing,
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
    use DisclosureTiming::{Initial, Runtime};
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
        designation: None,
        reader: None,
        assistant_profile: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        Some(view),
        0,
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
    use DisclosureTiming::Initial;
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

// ---------------------------------------------------------------------------
// MEDULLA-DECISION-ENGINE-R143.3-R143.5: the disclosure EVENT mode of the same
// kind. The prior event is read from the caller's own committed log.
// ---------------------------------------------------------------------------

use medulla_decision_types::decision::instruction_disclosure::{
    DisclosureEvent, DisclosureTrigger,
};
use medulla_decision_types::decision::statistical::declared::{DeclaredOption, DeclaredText};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::{EvidenceClass, ResolutionKind};

const SET: &str = "set-events";

fn event_request(
    h: &Harness,
    tenant: &str,
    trigger: Option<&str>,
    instructions: Option<&str>,
    extra: Vec<TypedParam>,
) -> DecideRequest {
    let schema = publish_route_schema_in(h, tenant);
    let mut params = extra;
    if let Some(trigger) = trigger {
        params.push(text("trigger", trigger));
    }
    let mut request = declared_request(
        &schema,
        tenant,
        QuestionKind::InstructionDisclosure,
        SET,
        DecisionPolicyRef::Default,
        params,
    );
    let class = if trigger == Some("initial") {
        "initial"
    } else {
        "runtime_refinement"
    };
    request.candidates = CandidateSource::Declared {
        options: medulla_types::contract::BoundedVec::new(vec![DeclaredOption {
            option_id: SET.to_string(),
            classification: medulla_types::contract::BoundedVec::new(vec![class.to_string()])
                .unwrap(),
            numbers: medulla_types::contract::BoundedVec::default(),
            texts: medulla_types::contract::BoundedVec::new(
                instructions
                    .map(|body| {
                        vec![DeclaredText {
                            key: "instructions".to_string(),
                            text: body.to_string(),
                        }]
                    })
                    .unwrap_or_default(),
            )
            .unwrap(),
        }])
        .unwrap(),
    };
    request
}

fn int(name: &str, value: i64) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Int(value),
    }
}

fn decide_event(
    h: &Harness,
    tenant: &str,
    now_ms: u64,
    request: &DecideRequest,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
        now_ms,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        None,
        0,
        // The verified principal's persistence id, as the served path seals it,
        // so `DecisionLog.commit` by "decider" accepts the record.
        super::retrieval_tests::principal_of("decider"),
        Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

async fn commit_event(h: &Harness, record: &StatisticalDecisionRecord) -> Result<(), String> {
    decode::<DecisionLogCommitted>(
        log_tests::log_op(
            h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
                approval: None,
            },
        )
        .await,
    )
    .map(|_| ())
}

fn event_of(record: &StatisticalDecisionRecord) -> DisclosureEvent {
    record
        .answer
        .as_ref()
        .and_then(FamilyAnswer::disclosure_event)
        .expect("the record carries its typed disclosure event")
        .clone()
}

fn logged_rows(h: &Harness) -> usize {
    let reader = super::super::stat_log::LogReader {
        tenant_id: TENANT.to_string(),
        principal: super::retrieval_tests::principal_of("decider"),
        roles: Vec::new(),
        retention: super::super::stat_retention::Retention::none(),
    };
    super::super::stat_log::visible_entries(&h.store, &reader)
        .unwrap()
        .len()
}

// spec: MEDULLA-DECISION-ENGINE-R143.3
#[tokio::test]
async fn decide_routes_disclosure_events() {
    let h = Harness::new().await;

    let initial = decide_event(
        &h,
        TENANT,
        1_000,
        &event_request(&h, TENANT, Some("initial"), Some("be brief"), vec![]),
    )
    .unwrap();
    assert_eq!(disclosed(&initial), vec![format!("{SET}@v1")]);
    assert_eq!(initial.resolution_kind, ResolutionKind::Constraint);
    let first = event_of(&initial);
    assert_eq!(
        (first.version, first.trigger, first.at_ms),
        (1, DisclosureTrigger::Initial, 1_000)
    );
    assert_eq!(first.instructions_hash, instructions_hash("be brief"));
    commit_event(&h, &initial).await.unwrap();

    // Each refinement reads the committed prior event and yields the next version.
    let refinement = decide_event(
        &h,
        TENANT,
        2_000,
        &event_request(
            &h,
            TENANT,
            Some("relevance_update"),
            Some("be brief and cite sources"),
            vec![int("prior_version", 1)],
        ),
    )
    .unwrap();
    assert_eq!(disclosed(&refinement), vec![format!("{SET}@v2")]);
    assert_eq!(
        event_of(&refinement).trigger,
        DisclosureTrigger::RelevanceUpdate
    );
    commit_event(&h, &refinement).await.unwrap();
    let timeout = decide_event(
        &h,
        TENANT,
        3_000,
        &event_request(
            &h,
            TENANT,
            Some("timeout"),
            Some("be brief, cite sources, ask when unsure"),
            vec![int("prior_version", 2), int("elapsed_s", 300)],
        ),
    )
    .unwrap();
    assert_eq!(event_of(&timeout).version, 3);
    assert_ne!(
        event_of(&timeout).instructions_hash,
        first.instructions_hash
    );

    // Tenant B has committed nothing: a refinement finds no prior event.
    let other = decide_event(
        &h,
        OTHER_TENANT,
        4_000,
        &event_request(
            &h,
            OTHER_TENANT,
            Some("relevance_update"),
            Some("be brief and cite sources"),
            vec![],
        ),
    );
    assert!(
        other.unwrap_err().starts_with("DISCLOSURE_PRIOR_MISSING"),
        "tenant B sees no prior event of tenant A"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R143.4
#[tokio::test]
async fn invalid_trigger_or_hash_refuses() {
    let h = Harness::new().await;
    let refused = |result: Result<StatisticalDecisionRecord, String>, code: &str| {
        let error = result.expect_err(code);
        assert!(error.starts_with(code), "{code}: {error}");
    };
    // No known trigger.
    refused(
        decide_event(
            &h,
            TENANT,
            1_000,
            &event_request(&h, TENANT, None, Some("a"), vec![]),
        ),
        "DISCLOSURE_TRIGGER_MISSING",
    );
    refused(
        decide_event(
            &h,
            TENANT,
            1_000,
            &event_request(&h, TENANT, Some("sometime"), Some("a"), vec![]),
        ),
        "DISCLOSURE_TRIGGER_MISSING",
    );
    // A computed hash the caller did not expect.
    refused(
        decide_event(
            &h,
            TENANT,
            1_000,
            &event_request(
                &h,
                TENANT,
                Some("initial"),
                Some("a"),
                vec![text("expected_hash", &instructions_hash("b"))],
            ),
        ),
        "DISCLOSURE_HASH_MISMATCH",
    );
    // An instruction set with no text abstains.
    let undefined = decide_event(
        &h,
        TENANT,
        1_000,
        &event_request(&h, TENANT, Some("initial"), None, vec![]),
    )
    .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &undefined.outcome else {
        panic!("expected an abstention: {:?}", undefined.outcome)
    };
    assert!(format!("{reasons:?}").contains("DISCLOSURE_SET_UNDEFINED"));

    let initial = decide_event(
        &h,
        TENANT,
        1_000,
        &event_request(&h, TENANT, Some("initial"), Some("a"), vec![]),
    )
    .unwrap();
    commit_event(&h, &initial).await.unwrap();
    let rows = logged_rows(&h);

    // The initial disclosure cannot repeat.
    refused(
        decide_event(
            &h,
            TENANT,
            2_000,
            &event_request(&h, TENANT, Some("initial"), Some("b"), vec![]),
        ),
        "DISCLOSURE_PRIOR_UNEXPECTED",
    );
    // A refinement that changes nothing.
    refused(
        decide_event(
            &h,
            TENANT,
            2_000,
            &event_request(&h, TENANT, Some("constraint_change"), Some("a"), vec![]),
        ),
        "DISCLOSURE_HASH_UNCHANGED",
    );
    // A timeout before disclosure.timeout seconds elapsed.
    refused(
        decide_event(
            &h,
            TENANT,
            2_000,
            &event_request(
                &h,
                TENANT,
                Some("timeout"),
                Some("b"),
                vec![int("elapsed_s", 10)],
            ),
        ),
        "DISCLOSURE_TIMEOUT_NOT_ELAPSED",
    );
    // A refinement built on a version older than the latest committed one.
    let second = decide_event(
        &h,
        TENANT,
        2_000,
        &event_request(
            &h,
            TENANT,
            Some("relevance_update"),
            Some("b"),
            vec![int("prior_version", 1)],
        ),
    )
    .unwrap();
    commit_event(&h, &second).await.unwrap();
    refused(
        decide_event(
            &h,
            TENANT,
            3_000,
            &event_request(
                &h,
                TENANT,
                Some("relevance_update"),
                Some("c"),
                vec![int("prior_version", 1)],
            ),
        ),
        "DISCLOSURE_VERSION_STALE",
    );
    // A stale commit is refused and leaves the log unchanged.
    let before = logged_rows(&h);
    assert_eq!(before, rows + 1);
    let replayed_stale = commit_event(&h, &second).await;
    assert!(
        replayed_stale.is_ok(),
        "an identical re-commit is idempotent"
    );
    assert_eq!(logged_rows(&h), before);
}

// spec: MEDULLA-DECISION-ENGINE-R143.5
#[tokio::test]
async fn disclosure_binds_version_and_hash() {
    let h = Harness::new().await;
    let initial = decide_event(
        &h,
        TENANT,
        1_000,
        &event_request(&h, TENANT, Some("initial"), Some("be brief"), vec![]),
    )
    .unwrap();
    let hash = instructions_hash("be brief");
    let premise = fact(
        &initial,
        &format!("disclosure_event:{SET}@v1 trigger=initial at_ms=1000 hash={hash}"),
    )
    .expect("one Claim premise per event");
    assert_eq!(premise.class, PremiseClass::Claim);
    assert_eq!(
        premise.provenance,
        PremiseProvenance::Request {
            field: "candidates".to_string()
        }
    );
    assert_eq!(initial.evidence_class, EvidenceClass::Claim);
    assert_eq!(event_of(&initial).instructions_hash, hash);
    commit_event(&h, &initial).await.unwrap();

    // The refinement's event stays separate from the initial one, by trigger.
    let refinement = decide_event(
        &h,
        TENANT,
        2_000,
        &event_request(
            &h,
            TENANT,
            Some("constraint_change"),
            Some("be brief, no jargon"),
            vec![int("prior_version", 1)],
        ),
    )
    .unwrap();
    let refined_hash = instructions_hash("be brief, no jargon");
    assert!(fact(
        &refinement,
        &format!(
            "disclosure_event:{SET}@v2 trigger=constraint_change at_ms=2000 hash={refined_hash}"
        )
    )
    .is_some());
    assert!(fact(
        &refinement,
        &format!("disclosure_event:{SET}@v1 trigger=initial at_ms=1000 hash={hash}")
    )
    .is_none());
    assert_eq!(event_of(&refinement).version, 2);
}

// spec: MEDULLA-DECISION-ENGINE-R143.6
#[tokio::test]
async fn disclosure_journey() {
    let h = Harness::new().await;
    let refused = |result: Result<StatisticalDecisionRecord, String>, code: &str| {
        let error = result.expect_err(code);
        assert!(error.starts_with(code), "{code}: {error}");
    };
    let hash_of = |body: &str| instructions_hash(body);
    let event_fact = |version: u64, trigger: &str, at_ms: u64, body: &str| {
        format!(
            "disclosure_event:{SET}@v{version} trigger={trigger} at_ms={at_ms} hash={}",
            hash_of(body)
        )
    };

    // 1. The initial disclosure: a Constraint resolution over a Claim, with one
    //    Claim premise naming version, trigger, time and hash.
    let initial = decide_event(
        &h,
        TENANT,
        1_000,
        &event_request(&h, TENANT, Some("initial"), Some("be brief"), vec![]),
    )
    .unwrap();
    assert_eq!(disclosed(&initial), vec![format!("{SET}@v1")]);
    assert_eq!(initial.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(initial.evidence_class, EvidenceClass::Claim);
    let premise = fact(&initial, &event_fact(1, "initial", 1_000, "be brief"))
        .expect("the initial event premise");
    assert_eq!(premise.class, PremiseClass::Claim);
    commit_event(&h, &initial).await.unwrap();

    // 2. Every refusal on top of the committed initial event; none of them
    //    writes a record.
    let rows = logged_rows(&h);
    refused(
        decide_event(
            &h,
            TENANT,
            1_500,
            &event_request(&h, TENANT, None, Some("be brief"), vec![]),
        ),
        "DISCLOSURE_TRIGGER_MISSING",
    );
    refused(
        decide_event(
            &h,
            TENANT,
            1_500,
            &event_request(
                &h,
                TENANT,
                Some("relevance_update"),
                Some("x"),
                vec![text("expected_hash", &hash_of("y"))],
            ),
        ),
        "DISCLOSURE_HASH_MISMATCH",
    );
    refused(
        decide_event(
            &h,
            TENANT,
            1_500,
            &event_request(&h, TENANT, Some("initial"), Some("again"), vec![]),
        ),
        "DISCLOSURE_PRIOR_UNEXPECTED",
    );
    refused(
        decide_event(
            &h,
            TENANT,
            1_500,
            &event_request(
                &h,
                TENANT,
                Some("constraint_change"),
                Some("be brief"),
                vec![],
            ),
        ),
        "DISCLOSURE_HASH_UNCHANGED",
    );
    refused(
        decide_event(
            &h,
            TENANT,
            1_500,
            &event_request(
                &h,
                TENANT,
                Some("timeout"),
                Some("be brief, again"),
                vec![int("elapsed_s", 10)],
            ),
        ),
        "DISCLOSURE_TIMEOUT_NOT_ELAPSED",
    );
    let undefined = decide_event(
        &h,
        TENANT,
        1_500,
        &event_request(&h, TENANT, Some("relevance_update"), None, vec![]),
    )
    .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &undefined.outcome else {
        panic!("expected an abstention: {:?}", undefined.outcome)
    };
    assert!(format!("{reasons:?}").contains("DISCLOSURE_SET_UNDEFINED"));
    assert_eq!(
        logged_rows(&h),
        rows,
        "refusals and abstentions write nothing"
    );

    // 3. The three runtime refinements, each committed through the DecisionLog
    //    and read back as the next prior event.
    let relevance = decide_event(
        &h,
        TENANT,
        2_000,
        &event_request(
            &h,
            TENANT,
            Some("relevance_update"),
            Some("be brief and cite sources"),
            vec![int("prior_version", 1)],
        ),
    )
    .unwrap();
    assert_eq!(disclosed(&relevance), vec![format!("{SET}@v2")]);
    assert!(fact(
        &relevance,
        &event_fact(2, "relevance_update", 2_000, "be brief and cite sources")
    )
    .is_some());
    commit_event(&h, &relevance).await.unwrap();
    refused(
        decide_event(
            &h,
            TENANT,
            2_500,
            &event_request(
                &h,
                TENANT,
                Some("relevance_update"),
                Some("late"),
                vec![int("prior_version", 1)],
            ),
        ),
        "DISCLOSURE_VERSION_STALE",
    );

    let constraint = decide_event(
        &h,
        TENANT,
        3_000,
        &event_request(
            &h,
            TENANT,
            Some("constraint_change"),
            Some("be brief, cite sources, no jargon"),
            vec![int("prior_version", 2)],
        ),
    )
    .unwrap();
    assert_eq!(disclosed(&constraint), vec![format!("{SET}@v3")]);
    commit_event(&h, &constraint).await.unwrap();

    let timeout = decide_event(
        &h,
        TENANT,
        4_000,
        &event_request(
            &h,
            TENANT,
            Some("timeout"),
            Some("be brief, cite sources, no jargon, ask when unsure"),
            vec![int("prior_version", 3), int("elapsed_s", 300)],
        ),
    )
    .unwrap();
    assert_eq!(disclosed(&timeout), vec![format!("{SET}@v4")]);
    for record in [&relevance, &constraint, &timeout] {
        assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
        assert_eq!(record.evidence_class, EvidenceClass::Claim);
    }
    commit_event(&h, &timeout).await.unwrap();

    // 4. Distinct versions and hashes across the four committed events.
    let events = [&initial, &relevance, &constraint, &timeout].map(event_of);
    let versions: Vec<u32> = events.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3, 4]);
    let triggers: Vec<DisclosureTrigger> = events.iter().map(|e| e.trigger).collect();
    assert_eq!(
        triggers,
        vec![
            DisclosureTrigger::Initial,
            DisclosureTrigger::RelevanceUpdate,
            DisclosureTrigger::ConstraintChange,
            DisclosureTrigger::Timeout,
        ]
    );
    let mut hashes: Vec<String> = events.iter().map(|e| e.instructions_hash.clone()).collect();
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), 4, "every event carries its own hash");
    assert_eq!(logged_rows(&h), rows + 3);

    // 5. Cross-tenant negative: tenant B sees none of tenant A's log. A
    //    refinement finds no prior event, and B's own initial disclosure is
    //    version 1, not a continuation of A's chain.
    refused(
        decide_event(
            &h,
            OTHER_TENANT,
            5_000,
            &event_request(
                &h,
                OTHER_TENANT,
                Some("timeout"),
                Some("be brief, cite sources, no jargon, ask when unsure"),
                vec![int("elapsed_s", 300)],
            ),
        ),
        "DISCLOSURE_PRIOR_MISSING",
    );
    let other = decide_event(
        &h,
        OTHER_TENANT,
        5_000,
        &event_request(&h, OTHER_TENANT, Some("initial"), Some("be brief"), vec![]),
    )
    .unwrap();
    assert_eq!(event_of(&other).version, 1);
}
