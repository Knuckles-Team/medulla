//! Human corrections served by `Method::CorrectionRecord`
//! (MEDULLA-DECISION-ENGINE-R702.3 to R702.6): the handler plans one batch that
//! holds the correction node, its three effect nodes, their edges and the
//! correction embedding; the refusals plan nothing; the correction reads back
//! by receipt id; another tenant reads nothing.

use super::super::correction::handle_correction_record;
use super::super::planned_write::PlannedWrite;
use super::*;
use medulla_agent_types::agent_guardrail::RuleInvariant;
use medulla_decision_stats::decision::correction::commit_correction;
use medulla_decision_types::correction::{
    task_class_of, Correction, CorrectionInput, CorrectionOp, CorrectionOutcomes, CorrectionRead, CorrectionRecorded,
    CorrectionSource, CORRECTION_NOT_FOUND, CORRECTION_ROLE_REQUIRED, OUTCOME_SELF_REPORTED,
};
use medulla_protocol_types::protocol::ResultPayload;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;

const GRAPH: &str = "__commons__";

fn corrector(tenant: &str) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_roles("decider", tenant, &["corrector"])
}

/// The action record every fixture correction targets: produced by `agent:planner`, a tool call.
const ACTION: &str = "action:7";
const PRODUCER: &str = "agent:planner";

fn correction(id: &str, target: &str, after: &str) -> CorrectionInput {
    CorrectionInput {
        correction_id: id.into(),
        target: target.into(),
        before: "the wrong answer".into(),
        after: after.into(),
        at_ms: 1_700_000_000_000,
        source: CorrectionSource::Human,
    }
}

/// The correction a fixture input becomes once the handler resolves it: the producer and the task
/// class come from the action record, the corrector from the verified principal.
fn stored(input: CorrectionInput) -> Correction {
    input.resolve(PRODUCER.into(), "decider".into(), task_class_of("tool_call"))
}

/// Store one `ActionRecord` the way ingestion does, owned by the test principal.
fn seed_action(core: &medulla_graph_core::graph::GraphCore, id: &str, tenant: &str, actor: &str) {
    let properties = serde_json::json!({
        "type": "ActionRecord",
        "actionKind": "tool_call",
        "actor": actor,
        "idempotencyKey": id,
        "tenant_id": tenant,
        "_owner": "decider",
    });
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&properties).unwrap());
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

async fn plan(
    h: &Harness,
    verified: &VerifiedRequestContext,
    op: CorrectionOp,
) -> Result<PlannedWrite, String> {
    handle_correction_record(&h.state, verified, GRAPH, op).await
}

fn refused(result: Result<PlannedWrite, String>) -> String {
    match result {
        Err(error) => error,
        Ok(_) => panic!("the operation must refuse"),
    }
}

fn reply<T: serde::de::DeserializeOwned>(write: &PlannedWrite) -> T {
    match &write.reply {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

/// Commit a planned batch to the core by hand, the way the router commits it.
fn commit(core: &medulla_graph_core::graph::GraphCore, write: &PlannedWrite) {
    for op in &write.operations {
        match op["op"].as_str().unwrap() {
            "upsert_node" => core.add_node(
                op["id"].as_str().unwrap().to_string(),
                rmp_serde::to_vec_named(&op["properties"]).unwrap(),
            ),
            "upsert_edge" => core
                .add_edge(
                    op["source"].as_str().unwrap().to_string(),
                    op["target"].as_str().unwrap().to_string(),
                    rmp_serde::to_vec_named(&op["properties"]).unwrap(),
                )
                .unwrap(),
            "add_embedding" => {
                let vector: Vec<f32> = serde_json::from_value(op["embedding"].clone()).unwrap();
                core.semantic_store
                    .write()
                    .add_embedding(op["id"].as_str().unwrap().to_string(), vector)
                    .unwrap();
            }
            other => panic!("unexpected operation {other}"),
        }
    }
}

fn publish_invariant(h: &Harness, invariant: &RuleInvariant) {
    let mut draft = test_component_draft(TENANT, &invariant.invariant_iri);
    draft.kind = AgentComponentKind::RuleInvariant;
    draft.attributes = invariant.to_attributes();
    h.store
        .publish_component(
            AgentComponentPublishRequest {
                context: mutation_context(
                    &h.store,
                    TENANT,
                    &format!("invariant-{}", invariant.invariant_iri),
                    1,
                    0,
                    "agent-component:publish",
                ),
                evaluation_receipt_digest: None,
                component: draft,
                body: None,
            },
            None,
        )
        .expect("invariant publishes");
}

fn record(c: CorrectionInput) -> CorrectionOp {
    CorrectionOp::Record { correction: c }
}

async fn harness() -> Harness {
    Harness::with_isolation(ServerState::test_isolation("decider")).await
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.3: `Record` plans the four nodes, the
/// three edges and the correction embedding as one batch.
// spec: MEDULLA-DECISION-ENGINE-R702.3
#[tokio::test]
async fn correction_reaches_the_handler() {
    let h = harness().await;
    seed_action(&core(&h).await, ACTION, TENANT, PRODUCER);
    let write = plan(&h, &corrector(TENANT), record(correction("c1", ACTION, "right")))
        .await
        .ok()
        .expect("plans");
    let recorded: CorrectionRecorded = reply(&write);
    let receipt = &recorded.receipt_id;
    let ids: Vec<&str> = write
        .operations
        .iter()
        .filter(|op| op["op"] == "upsert_node")
        .map(|op| op["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            format!("correction:{receipt}"),
            format!("rule:{receipt}"),
            format!("outcome:{receipt}"),
            format!("case:{receipt}"),
        ]
    );
    assert_eq!(
        write.operations.iter().filter(|op| op["op"] == "upsert_edge").count(),
        3
    );
    let embedding = write.operations.last().unwrap();
    assert_eq!(embedding["op"], "add_embedding");
    assert_eq!(embedding["id"], format!("correction:{receipt}"));
    // Every node is stamped with its class, owner and tenant.
    let first = &write.operations[0]["properties"];
    assert_eq!(first["type"], "Correction");
    assert_eq!(first["tenant_id"], TENANT);
    assert_eq!(first["_owner"], "decider");
    assert_eq!(recorded.effect.outcome.value, 0.0);
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.4: each refusal plans nothing.
// spec: MEDULLA-DECISION-ENGINE-R702.4
#[tokio::test]
async fn correction_refusals_commit_nothing() {
    let h = harness().await;
    let core = core(&h).await;
    seed_action(&core, ACTION, TENANT, PRODUCER);
    // An action the corrector produced itself, and one of another tenant.
    seed_action(&core, "action:own", TENANT, "decider");
    seed_action(&core, "action:foreign", "tenant-other", PRODUCER);
    let nodes_before = core.node_ids().len();

    // No verified role `corrector`.
    let plain = VerifiedRequestContext::verified_for_test_with_roles("decider", TENANT, &["test"]);
    let error = refused(plan(&h, &plain, record(correction("c1", ACTION, "right"))).await);
    assert!(error.starts_with(CORRECTION_ROLE_REQUIRED.as_str()), "{error}");

    // The target names no action record: absent, another tenant's and a node of another class
    // all refuse with the same bytes.
    let absent = refused(plan(&h, &corrector(TENANT), record(correction("c1", "action:none", "x"))).await);
    assert!(absent.starts_with(CORRECTION_NOT_FOUND.as_str()), "{absent}");
    let foreign =
        refused(plan(&h, &corrector(TENANT), record(correction("c1", "action:foreign", "x"))).await);
    assert_eq!(foreign, absent);
    let wrong_class = serde_json::json!({"type": "Note", "tenant_id": TENANT, "_owner": "decider"});
    core.add_node("note:1".to_string(), rmp_serde::to_vec_named(&wrong_class).unwrap());
    let nodes_before = nodes_before + 1;
    let note = refused(plan(&h, &corrector(TENANT), record(correction("c1", "note:1", "x"))).await);
    assert_eq!(note, absent);

    // The corrector is the producer of the corrected action, read from the record: a caller
    // cannot attribute the correction to another actor.
    let error = refused(plan(&h, &corrector(TENANT), record(correction("c2", "action:own", "right"))).await);
    assert!(error.starts_with(OUTCOME_SELF_REPORTED.as_str()), "{error}");

    // The candidate rule contradicts a stored invariant of its task class. Its id is the digest
    // of its rule text, so an invariant names that digest.
    let contradicted = correction("c3", ACTION, "right");
    let rule_id = commit_correction(&stored(contradicted.clone()))
        .unwrap()
        .effect
        .rule_candidate
        .rule_id;
    assert!(rule_id.starts_with("sha256:"), "{rule_id}");
    publish_invariant(
        &h,
        &RuleInvariant {
            invariant_iri: "inv:no-such-rule".into(),
            forbidden_rule_id: rule_id.clone(),
            on_class: task_class_of("tool_call"),
        },
    );
    let error = refused(plan(&h, &corrector(TENANT), record(contradicted)).await);
    assert!(error.starts_with("RULE_CONTRADICTS_INVARIANT"), "{error}");
    // The same rule text under a task class the invariant does not govern is not refused.
    publish_invariant(
        &h,
        &RuleInvariant {
            invariant_iri: "inv:other-class".into(),
            forbidden_rule_id: commit_correction(&stored(correction("c5", ACTION, "other text")))
                .unwrap()
                .effect
                .rule_candidate
                .rule_id,
            on_class: task_class_of("filing"),
        },
    );
    plan(&h, &corrector(TENANT), record(correction("c5", ACTION, "other text")))
        .await
        .ok()
        .expect("an invariant of another class does not govern it");

    // An oversized value is the shape error of R702.1.
    let error = refused(
        plan(
            &h,
            &corrector(TENANT),
            record(correction("c4", ACTION, &"x".repeat(8 * 1024 + 1))),
        )
        .await,
    );
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    assert_eq!(core.node_ids().len(), nodes_before, "a refusal wrote nothing");
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.5: all four nodes carry the receipt
/// id and read back by it; another tenant finds nothing.
// spec: MEDULLA-DECISION-ENGINE-R702.5
#[tokio::test]
async fn correction_reads_back_by_receipt() {
    let h = harness().await;
    let core = core(&h).await;
    seed_action(&core, ACTION, TENANT, PRODUCER);
    let write = plan(&h, &corrector(TENANT), record(correction("c1", ACTION, "right")))
        .await
        .ok()
        .expect("plans");
    let recorded: CorrectionRecorded = reply(&write);
    commit(&core, &write);
    let receipt = recorded.receipt_id.clone();
    for op in write.operations.iter().filter(|op| op["op"] == "upsert_node") {
        assert_eq!(op["properties"]["receipt_id"], receipt.as_str());
    }

    let get = CorrectionOp::Get { receipt_id: receipt.clone() };
    let read = plan(&h, &corrector(TENANT), get).await.ok().expect("reads");
    let read: CorrectionRead = reply(&read);
    assert_eq!(read.receipt_id, receipt);
    assert_eq!(read.correction, stored(correction("c1", ACTION, "right")));
    assert_eq!(read.correction.produced_by, PRODUCER);
    assert_eq!(read.correction.actor, "decider");
    assert_eq!(read.effect.rule_candidate.task_class, task_class_of("tool_call"));
    assert_eq!(read.effect, recorded.effect);

    // Another tenant reads the same as an absent receipt.
    let foreign = CorrectionOp::Get { receipt_id: receipt };
    let error = refused(plan(&h, &corrector("tenant-other"), foreign).await);
    assert!(error.starts_with(CORRECTION_NOT_FOUND.as_str()), "{error}");
    let absent = CorrectionOp::Get { receipt_id: "0".repeat(64) };
    let same = refused(plan(&h, &corrector("tenant-other"), absent).await);
    assert!(same.starts_with(CORRECTION_NOT_FOUND.as_str()), "{same}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.7: a review source yields the same three effects under
/// the same role, a rejection scores 0.0, and a review comment is stored as digests only.
// spec: MEDULLA-DECISION-ENGINE-R702.7
#[tokio::test]
async fn review_sources_yield_eval_cases_with_digests_only() {
    let h = harness().await;
    let core = core(&h).await;
    seed_action(&core, ACTION, TENANT, PRODUCER);

    // A rejection yields the three effects and an outcome of 0.0, and names its source.
    let mut rejection = correction("r1", ACTION, "better");
    rejection.source = CorrectionSource::ReviewRejection;
    let write = plan(&h, &corrector(TENANT), record(rejection)).await.ok().expect("plans");
    let recorded: CorrectionRecorded = reply(&write);
    assert_eq!(recorded.effect.outcome.value, 0.0);
    assert_eq!(write.operations[0]["properties"]["source"], "review_rejection");
    assert_eq!(
        write.operations.iter().filter(|op| op["op"] == "upsert_node").count(),
        4
    );

    // A review source needs the same verified role as a human.
    let mut unprivileged = correction("r2", ACTION, "better");
    unprivileged.source = CorrectionSource::ReviewRejection;
    let plain = VerifiedRequestContext::verified_for_test_with_roles("decider", TENANT, &["test"]);
    let error = refused(plan(&h, &plain, record(unprivileged)).await);
    assert!(error.starts_with(CORRECTION_ROLE_REQUIRED.as_str()), "{error}");

    // A review comment with text is refused; with digests it is stored, and the stored nodes
    // hold neither the output nor the comment text.
    let mut text = correction("r3", ACTION, "please rename this variable");
    text.source = CorrectionSource::ReviewComment;
    let error = refused(plan(&h, &corrector(TENANT), record(text)).await);
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    let mut digests = correction("r4", ACTION, &format!("sha256:{}", "c".repeat(64)));
    digests.before = format!("sha256:{}", "d".repeat(64));
    digests.source = CorrectionSource::ReviewComment;
    let write = plan(&h, &corrector(TENANT), record(digests.clone())).await.ok().expect("plans");
    commit(&core, &write);
    let receipt = reply::<CorrectionRecorded>(&write).receipt_id;
    let get = CorrectionOp::Get { receipt_id: receipt };
    let read: CorrectionRead = reply(&plan(&h, &corrector(TENANT), get).await.ok().expect("reads"));
    assert_eq!(read.correction, stored(digests));
    assert_eq!(read.correction.source, CorrectionSource::ReviewComment);
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.7 and R711.3: a correction of a skill feeds one
/// `SkillEvalCase` carrying digests only, with source `Correction` for a human and `ReviewSignal`
/// for a review source; a correction of anything else feeds none.
// spec: MEDULLA-DECISION-ENGINE-R702.7
#[tokio::test]
async fn skill_corrections_feed_skill_eval_cases() {
    let h = harness().await;
    let human = correction("f1", "skill:triage@3", "ask for the order id first");
    let write = plan(&h, &corrector(TENANT), record(human)).await.ok().expect("plans");
    let cases: Vec<&Value> = write
        .operations
        .iter()
        .filter(|op| op["op"] == "upsert_node" && op["properties"]["type"] == "SkillEvalCase")
        .collect();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["properties"]["source"], "correction");
    assert_eq!(cases[0]["properties"]["skill_ref"], "skill:triage@3");
    assert_eq!(cases[0]["properties"]["kind"], "golden");
    // Digests only: no property holds the corrected text.
    assert!(!cases[0].to_string().contains("ask for the order id first"));
    assert!(write.operations.iter().any(|op| {
        op["op"] == "upsert_edge" && op["properties"]["relationship"] == "yields_skill_case"
    }));

    let mut rejection = correction("f2", "skill:triage@3", "better");
    rejection.source = CorrectionSource::ReviewRejection;
    let write = plan(&h, &corrector(TENANT), record(rejection)).await.ok().expect("plans");
    let case = write
        .operations
        .iter()
        .find(|op| op["properties"]["type"] == "SkillEvalCase")
        .expect("a case");
    assert_eq!(case["properties"]["source"], "review_signal");

    let other = correction("f3", "answer:9", "better");
    let write = plan(&h, &corrector(TENANT), record(other)).await.ok().expect("plans");
    assert!(write.operations.iter().all(|op| op["properties"]["type"] != "SkillEvalCase"));
}

/// Verifies MEDULLA-DECISION-ENGINE-R702.8: the independent outcomes group per rule candidate,
/// the same rule text under two corrections is one candidate, and another tenant reads none.
// spec: MEDULLA-DECISION-ENGINE-R702.8
#[tokio::test]
async fn independent_outcomes_read_per_rule_candidate() {
    let h = harness().await;
    let core = core(&h).await;
    seed_action(&core, ACTION, TENANT, PRODUCER);
    seed_action(&core, "action:8", TENANT, PRODUCER);
    for input in [
        correction("c1", ACTION, "right"),
        correction("c2", "action:8", "right"),
        correction("c3", ACTION, "another rule"),
    ] {
        let write = plan(&h, &corrector(TENANT), record(input)).await.ok().expect("plans");
        commit(&core, &write);
    }
    let read = |tenant: &str, rule_id: Option<String>| {
        let verified = corrector(tenant);
        let h = &h;
        async move {
            let write = plan(h, &verified, CorrectionOp::Outcomes { rule_id })
                .await
                .ok()
                .expect("reads");
            assert!(write.operations.is_empty(), "the read writes nothing");
            reply::<CorrectionOutcomes>(&write)
        }
    };
    let all = read(TENANT, None).await;
    assert_eq!(all.candidates.len(), 2);
    let right_id = commit_correction(&stored(correction("c1", ACTION, "right")))
        .unwrap()
        .effect
        .rule_candidate
        .rule_id;
    let right = all.candidates.iter().find(|c| c.rule_id == right_id).unwrap();
    assert_eq!(right.independent, 2);
    assert_eq!(right.mean_value, 0.0);
    assert_eq!(right.task_class, task_class_of("tool_call"));
    let one = read(TENANT, Some(right_id)).await;
    assert_eq!(one.candidates.len(), 1);
    assert!(read("tenant-other", None).await.candidates.is_empty());
}

/// Verifies MEDULLA-DECISION-ENGINE-R059: the outcome aggregate joins the independent
/// correction outcomes through the one read of R702.8, so its labels equal what
/// `CorrectionOp::Outcomes` answers, and an aggregate that asks for none joins none.
// spec: MEDULLA-DECISION-ENGINE-R059
#[tokio::test]
async fn aggregate_joins_the_independent_correction_outcomes() {
    use medulla_decision_types::decision::statistical::log::{
        CorrectionLabelRequest, DecisionLogOp, OutcomeAggregate, OutcomeAggregateRequest,
    };
    use medulla_decision_types::decision::RecordWindow;

    let h = harness().await;
    let core = core(&h).await;
    seed_action(&core, ACTION, TENANT, PRODUCER);
    seed_action(&core, "action:8", TENANT, PRODUCER);
    for input in [
        correction("c1", ACTION, "right"),
        correction("c2", "action:8", "right"),
        correction("c3", ACTION, "another rule"),
    ] {
        let write = plan(&h, &corrector(TENANT), record(input)).await.ok().expect("plans");
        commit(&core, &write);
    }
    let aggregate = |corrections: Option<CorrectionLabelRequest>| {
        let h = &h;
        async move {
            let verified = corrector(TENANT);
            let response = super::super::log::handle_decision_log(
                &h.state,
                9,
                &verified,
                DecisionLogOp::Aggregate {
                    request: OutcomeAggregateRequest {
                        tenant_id: TENANT.to_string(),
                        question_id: None,
                        window: RecordWindow {
                            from_ms: 0,
                            to_ms: u64::MAX,
                        },
                        attribution: None,
                        corrections,
                        transfer: None,
                    },
                },
            )
            .await;
            decode::<OutcomeAggregate>(response).expect("aggregates")
        }
    };
    let none = aggregate(None).await;
    assert!(none.correction_labels.is_empty(), "no request, no join");

    let joined = aggregate(Some(CorrectionLabelRequest {
        graph: GRAPH.to_string(),
        rule_id: None,
    }))
    .await;
    let read = plan(&h, &corrector(TENANT), CorrectionOp::Outcomes { rule_id: None })
        .await
        .ok()
        .expect("reads");
    let outcomes = reply::<CorrectionOutcomes>(&read).candidates;
    assert_eq!(joined.correction_labels.len(), outcomes.len());
    for (label, outcome) in joined.correction_labels.iter().zip(&outcomes) {
        assert_eq!(label.rule_id, outcome.rule_id);
        assert_eq!(label.task_class, outcome.task_class);
        assert_eq!(label.independent, outcome.independent);
        assert_eq!(label.mean_value_micros, 0);
    }
    let one = aggregate(Some(CorrectionLabelRequest {
        graph: GRAPH.to_string(),
        rule_id: Some(outcomes[0].rule_id.clone()),
    }))
    .await;
    assert_eq!(one.correction_labels.len(), 1);
}
