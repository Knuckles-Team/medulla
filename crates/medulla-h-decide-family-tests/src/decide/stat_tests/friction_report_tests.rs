//! Friction reports served through `Method::FrictionReport` (MEDULLA-DECISION-ENGINE-R710.3 to
//! R710.5): reports group by skill and feed failure patterns, every refusal writes nothing, and a
//! group walks the whole machine with an actor and evidence on every step.

use medulla_decision_outcome_types::decision::promotion::RevisionStage;
use medulla_decision_outcome_types::decision::revision_list::RevisionRow;
use medulla_h_decide::decide::failure_ingest::handle_failure_ingest;
use medulla_h_decide::decide::friction::handle_friction_report;
use medulla_h_decide_base::decide::planned_write::PlannedWrite;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_jobs::{encode_artifact, receipt_key, record_key};
use medulla_protocol_types::protocol::ResultPayload;
use serde_json::{json, Value};
use {
    medulla_decision_program_types::friction::FrictionFiled,
    medulla_decision_program_types::friction::FrictionGroupView,
    medulla_decision_program_types::friction::FrictionGroups,
    medulla_decision_program_types::friction::FrictionMoved,
    medulla_decision_program_types::friction::FrictionOp,
    medulla_decision_program_types::friction::FrictionReport,
    medulla_decision_program_types::friction::FrictionState,
    medulla_decision_program_types::friction::FrictionTransition,
    medulla_decision_program_types::friction::FRICTION_APPROVAL_ROLE_REQUIRED,
    medulla_decision_program_types::friction::FRICTION_EVIDENCE_REQUIRED,
    medulla_decision_program_types::friction::FRICTION_EVIDENCE_UNKNOWN,
    medulla_decision_program_types::friction::FRICTION_SELF_APPROVED,
    medulla_decision_program_types::friction::FRICTION_SKILL_UNKNOWN,
    medulla_decision_program_types::friction::FRICTION_TEXT_TOO_LARGE,
    medulla_decision_program_types::friction::FRICTION_TRANSITION_ILLEGAL,
};
use {
    medulla_decision_statistical_types::failure_patterns::FailureIngestResult,
    medulla_decision_statistical_types::failure_patterns::FailureOp,
};

const GRAPH: &str = "__commons__";
const SKILL: &str = "skill:triage@3";

fn all_scopes() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("agent-7", TENANT, &["*"])
}

/// A second human: it holds every scope and neither reports nor moves a group.
fn approver() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("agent-9", TENANT, &["*"])
}

/// A harness that knows both callers.
async fn two_callers() -> Harness {
    let mut isolation = ServerState::test_isolation("agent-7");
    isolation.register_agent(medulla_identity_core_types::acl::AgentIdentity {
        agent_id: "agent-9".into(),
        role: medulla_identity_core_types::acl::AgentRole::System,
        teams: Vec::new(),
        roles: Vec::new(),
    });
    Harness::with_isolation(isolation).await
}

fn without_approve() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes(
        "agent-7",
        TENANT,
        &["knowledge:flag", "knowledge:curate"],
    )
}

async fn core(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    h.state
        .read()
        .await
        .registry
        .get(GRAPH)
        .expect("graph exists")
        .core
        .clone()
}

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

fn commit(core: &medulla_graph_core::graph::GraphCore, write: &PlannedWrite) {
    for operation in &write.operations {
        match operation["op"].as_str() {
            Some("upsert_node") => put(
                core,
                operation["id"].as_str().unwrap(),
                operation["properties"].clone(),
            ),
            Some("upsert_edge") => {
                core.add_edge(
                    operation["source"].as_str().unwrap().to_string(),
                    operation["target"].as_str().unwrap().to_string(),
                    rmp_serde::to_vec_named(&operation["properties"]).unwrap(),
                )
                .unwrap();
            }
            other => panic!("unexpected operation {other:?}"),
        }
    }
}

fn reply<T: serde::de::DeserializeOwned>(write: &PlannedWrite) -> T {
    match &write.reply {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn run(
    h: &Harness,
    who: &VerifiedRequestContext,
    op: FrictionOp,
) -> Result<PlannedWrite, String> {
    handle_friction_report(&h.state, who, op).await
}

fn report(id: &str, run_id: &str, at_ms: u64) -> FrictionReport {
    FrictionReport {
        report_id: id.into(),
        tenant_id: "ignored".into(),
        skill_ref: SKILL.into(),
        skill_version: "3".into(),
        tried: "asked for the order".into(),
        expected: "an order id".into(),
        happened: "an empty answer".into(),
        proposed_fix: Some("ask for the id first".into()),
        run_id: run_id.into(),
        reported_by: "ignored".into(),
        at_ms,
    }
}

fn file(report: FrictionReport) -> FrictionOp {
    FrictionOp::Report {
        graph: GRAPH.into(),
        report,
    }
}

fn group_op() -> FrictionOp {
    FrictionOp::Group {
        graph: GRAPH.into(),
        skill_ref: None,
        limit: None,
    }
}

fn move_op(group_ref: &str, to: FrictionState, evidence: Option<&str>, at_ms: u64) -> FrictionOp {
    FrictionOp::Transition {
        graph: GRAPH.into(),
        transition: FrictionTransition {
            group_ref: group_ref.into(),
            to,
            evidence_ref: evidence.map(str::to_string),
        },
        at_ms,
    }
}

async fn seed(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    let core = core(h).await;
    put(&core, SKILL, json!({"type": "Skill", "tenant_id": TENANT}));
    for run_id in ["run-a", "run-b"] {
        put(
            &core,
            &format!("trace-{run_id}"),
            json!({"node_type": "RunTrace", "run_id": run_id, "tenant": TENANT}),
        );
    }
    core
}

/// File the three reports of the journey: two runs, three distinct reports.
async fn file_three(
    h: &Harness,
    core: &medulla_graph_core::graph::GraphCore,
    who: &VerifiedRequestContext,
) -> FrictionFiled {
    let mut last = None;
    for (id, run_id, at) in [
        ("r1", "run-a", 10),
        ("r2", "run-a", 20),
        ("r3", "run-b", 30),
    ] {
        let write = run(h, who, file(report(id, run_id, at)))
            .await
            .expect("files");
        commit(core, &write);
        last = Some(reply::<FrictionFiled>(&write));
    }
    last.unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R710.3
#[tokio::test]
async fn report_feeds_failure_patterns() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    let who = all_scopes();
    let filed = file_three(&h, &core, &who).await;
    assert_eq!(filed.group.report_count, 3);
    assert_eq!(filed.group.distinct_runs, 2);
    assert_eq!(filed.group.state, FrictionState::Reported);

    // A repeat of one report id is not counted twice.
    let again = run(&h, &who, file(report("r1", "run-a", 10)))
        .await
        .expect("repeats");
    assert!(again.operations.is_empty());
    assert!(!reply::<FrictionFiled>(&again).counted);

    // One group of the skill, three reports.
    let groups: FrictionGroups = reply(&run(&h, &who, group_op()).await.unwrap());
    assert_eq!(groups.groups.len(), 1);
    assert_eq!(groups.groups[0].report_count, 3);

    // The three reports are one recurring failure pattern, so ingest files one gap.
    let ingest = handle_failure_ingest(
        &h.state,
        &who,
        FailureOp::Ingest {
            graph: GRAPH.into(),
            since_ms: None,
        },
    )
    .await
    .expect("plans");
    let result: FailureIngestResult = reply(&ingest);
    assert_eq!(result.filed.len(), 1);
    assert!(result.patterns.iter().any(|pattern| pattern.count == 3));
}

// spec: MEDULLA-DECISION-ENGINE-R710.4
#[tokio::test]
async fn refusals_write_nothing() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    let who = all_scopes();
    let filed = file_three(&h, &core, &who).await;
    let group_ref = filed.group.group_ref.clone();
    let code = |result: Result<PlannedWrite, String>| result.expect_err("must refuse");

    let mut unknown = report("r9", "run-a", 5);
    unknown.skill_ref = "skill:nowhere@1".into();
    let error = code(run(&h, &who, file(unknown)).await);
    assert!(
        error.starts_with(FRICTION_SKILL_UNKNOWN.as_str()),
        "{error}"
    );
    let mut big = report("r9", "run-a", 5);
    big.tried = "x".repeat(2 * 1024 + 1);
    let error = code(run(&h, &who, file(big)).await);
    assert!(
        error.starts_with(FRICTION_TEXT_TOO_LARGE.as_str()),
        "{error}"
    );

    // A move outside the machine, and a move without its evidence.
    let error = code(
        run(
            &h,
            &who,
            move_op(&group_ref, FrictionState::Fixed, Some("rev:1"), 40),
        )
        .await,
    );
    assert!(
        error.starts_with(FRICTION_TRANSITION_ILLEGAL.as_str()),
        "{error}"
    );
    let triage = run(
        &h,
        &who,
        move_op(&group_ref, FrictionState::Triaged, None, 40),
    )
    .await
    .expect("triage needs no evidence");
    commit(&core, &triage);
    let error = code(
        run(
            &h,
            &who,
            move_op(&group_ref, FrictionState::RootCaused, None, 41),
        )
        .await,
    );
    assert!(
        error.starts_with(FRICTION_EVIDENCE_REQUIRED.as_str()),
        "{error}"
    );
    let error = code(
        run(
            &h,
            &who,
            move_op(&group_ref, FrictionState::RootCaused, Some("gap:none"), 41),
        )
        .await,
    );
    assert!(
        error.starts_with(FRICTION_EVIDENCE_REQUIRED.as_str()),
        "{error}"
    );

    // Approval needs its own scope.
    let error = code(
        run(
            &h,
            &without_approve(),
            move_op(&group_ref, FrictionState::Approved, Some("x"), 42),
        )
        .await,
    );
    assert!(
        error.starts_with(FRICTION_APPROVAL_ROLE_REQUIRED.as_str()),
        "{error}"
    );

    // Nothing moved: the group is still triaged.
    let got: FrictionGroupView = reply(
        &run(
            &h,
            &who,
            FrictionOp::Get {
                graph: GRAPH.into(),
                group_ref,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(got.group.state, FrictionState::Triaged);
    assert_eq!(got.steps.len(), 1);
}

// spec: MEDULLA-DECISION-ENGINE-R710.5
#[tokio::test]
async fn steps_record_actor_and_evidence() {
    let h = two_callers().await;
    let core = seed(&h).await;
    put(
        &core,
        "gap:g1",
        json!({"type": "FailureGap", "tenant_id": TENANT}),
    );
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let revision = RevisionRow {
        id: "rev:1".into(),
        subject_kind: "skill".into(),
        subject_ref: SKILL.into(),
        stage: RevisionStage::Evaluated,
        digest: "d".into(),
        evaluation_receipt_id: None,
        created_ms: 1,
        author: "principal:a".into(),
    };
    store
        .put_decision_artifacts(
            TENANT,
            &[
                (revision.key(), encode_artifact(&revision).unwrap()),
                (receipt_key("sha256:held-out"), b"receipt".to_vec()),
                (record_key("verdict:ok"), b"record".to_vec()),
            ],
        )
        .unwrap();
    let who = all_scopes();
    let second = approver();
    let filed = file_three(&h, &core, &who).await;
    let group_ref = filed.group.group_ref.clone();
    let approver_id = second.principal_persistence_id();

    let walk: [(FrictionState, Option<String>); 8] = [
        (FrictionState::Triaged, None),
        (FrictionState::RootCaused, Some("gap:g1".into())),
        (FrictionState::Fixed, Some("rev:1".into())),
        (FrictionState::Evaluated, Some("sha256:held-out".into())),
        (FrictionState::Approved, Some(approver_id.clone())),
        (FrictionState::Measured, Some("window:1,window:2".into())),
        (FrictionState::RegressionChecked, Some("verdict:ok".into())),
        (FrictionState::Closed, None),
    ];
    for (index, (to, evidence)) in walk.iter().enumerate() {
        let caller = if *to == FrictionState::Approved {
            &second
        } else {
            &who
        };
        let write = run(
            &h,
            caller,
            move_op(&group_ref, *to, evidence.as_deref(), 100 + index as u64),
        )
        .await
        .unwrap_or_else(|error| panic!("{to:?}: {error}"));
        let moved: FrictionMoved = reply(&write);
        assert_eq!(moved.group.state, *to);
        assert_eq!(moved.step.actor, caller.principal_persistence_id());
        assert_eq!(moved.step.evidence_ref, *evidence);
        commit(&core, &write);
    }
    let got: FrictionGroupView = reply(
        &run(
            &h,
            &who,
            FrictionOp::Get {
                graph: GRAPH.into(),
                group_ref: group_ref.clone(),
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(got.group.state, FrictionState::Closed);
    assert_eq!(got.steps.len(), 8);
    assert_eq!(got.steps[4].to, FrictionState::Approved);
    assert_eq!(got.report_ids, ["r1", "r2", "r3"]);

    // A report on the closed group opens the next generation.
    let next = run(&h, &who, file(report("r4", "run-b", 200)))
        .await
        .expect("files");
    let opened: FrictionFiled = reply(&next);
    assert_eq!(opened.group.report_count, 1);
    assert_ne!(opened.group.group_ref, group_ref);
    assert_eq!(opened.group.state, FrictionState::Reported);

    // Groups list by report count, most first.
    commit(&core, &next);
    let groups: FrictionGroups = reply(&run(&h, &who, group_op()).await.unwrap());
    assert_eq!(groups.groups[0].report_count, 3);
    assert_eq!(groups.groups[1].report_count, 1);
}

// spec: MEDULLA-DECISION-ENGINE-R710.4
#[tokio::test]
async fn approval_is_two_person_and_receipts_resolve_in_the_tenant() {
    let h = two_callers().await;
    let core = seed(&h).await;
    put(
        &core,
        "gap:g1",
        json!({"type": "FailureGap", "tenant_id": TENANT}),
    );
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let revision = RevisionRow {
        id: "rev:1".into(),
        subject_kind: "skill".into(),
        subject_ref: SKILL.into(),
        stage: RevisionStage::Evaluated,
        digest: "d".into(),
        evaluation_receipt_id: None,
        created_ms: 1,
        author: "principal:a".into(),
    };
    store
        .put_decision_artifacts(
            TENANT,
            &[(revision.key(), encode_artifact(&revision).unwrap())],
        )
        .unwrap();
    // A receipt and a verdict that belong to another tenant.
    store
        .put_decision_artifacts(
            "other-tenant",
            &[
                (receipt_key("sha256:foreign"), b"receipt".to_vec()),
                (record_key("verdict:foreign"), b"record".to_vec()),
            ],
        )
        .unwrap();
    let who = all_scopes();
    let second = approver();
    let group_ref = file_three(&h, &core, &who).await.group.group_ref;
    for (index, (to, evidence)) in [
        (FrictionState::Triaged, None),
        (FrictionState::RootCaused, Some("gap:g1")),
        (FrictionState::Fixed, Some("rev:1")),
    ]
    .into_iter()
    .enumerate()
    {
        let write = run(
            &h,
            &who,
            move_op(&group_ref, to, evidence, 100 + index as u64),
        )
        .await
        .expect("moves");
        commit(&core, &write);
    }
    let refuse = |result: Result<PlannedWrite, String>| result.expect_err("must refuse");

    // A missing receipt and another tenant's receipt refuse alike: no existence oracle.
    let missing = refuse(
        run(
            &h,
            &who,
            move_op(
                &group_ref,
                FrictionState::Evaluated,
                Some("sha256:none"),
                110,
            ),
        )
        .await,
    );
    let foreign = refuse(
        run(
            &h,
            &who,
            move_op(
                &group_ref,
                FrictionState::Evaluated,
                Some("sha256:foreign"),
                110,
            ),
        )
        .await,
    );
    assert!(
        missing.starts_with(FRICTION_EVIDENCE_UNKNOWN.as_str()),
        "{missing}"
    );
    assert_eq!(
        missing.replace("sha256:none", ""),
        foreign.replace("sha256:foreign", ""),
        "a missing and a foreign receipt answer alike"
    );
    // A non-empty string is no longer enough, for the verdict either.
    let fixed = run(
        &h,
        &who,
        move_op(&group_ref, FrictionState::Evaluated, Some("x"), 110),
    )
    .await;
    assert!(fixed
        .err()
        .unwrap()
        .starts_with(FRICTION_EVIDENCE_UNKNOWN.as_str()));
    store
        .put_decision_artifacts(
            TENANT,
            &[(receipt_key("sha256:held-out"), b"receipt".to_vec())],
        )
        .unwrap();
    let evaluated = run(
        &h,
        &who,
        move_op(
            &group_ref,
            FrictionState::Evaluated,
            Some("sha256:held-out"),
            111,
        ),
    )
    .await
    .expect("a stored receipt resolves");
    commit(&core, &evaluated);

    // The reporter and the earlier mover cannot approve; a second person can.
    let own = who.principal_persistence_id();
    let error = refuse(
        run(
            &h,
            &who,
            move_op(&group_ref, FrictionState::Approved, Some(&own), 112),
        )
        .await,
    );
    assert!(
        error.starts_with(FRICTION_SELF_APPROVED.as_str()),
        "{error}"
    );
    let other = second.principal_persistence_id();
    let approved = run(
        &h,
        &second,
        move_op(&group_ref, FrictionState::Approved, Some(&other), 113),
    )
    .await
    .expect("a second principal approves");
    commit(&core, &approved);
    let measured = run(
        &h,
        &who,
        move_op(
            &group_ref,
            FrictionState::Measured,
            Some("window:1,window:2"),
            114,
        ),
    )
    .await
    .expect("measures");
    commit(&core, &measured);
    let error = refuse(
        run(
            &h,
            &who,
            move_op(
                &group_ref,
                FrictionState::RegressionChecked,
                Some("verdict:foreign"),
                115,
            ),
        )
        .await,
    );
    assert!(
        error.starts_with(FRICTION_EVIDENCE_UNKNOWN.as_str()),
        "{error}"
    );
}
