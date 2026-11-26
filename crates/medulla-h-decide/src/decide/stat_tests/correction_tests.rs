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
    Correction, CorrectionOp, CorrectionRead, CorrectionRecorded, CorrectionSource,
    CORRECTION_NOT_FOUND, CORRECTION_ROLE_REQUIRED, OUTCOME_SELF_REPORTED,
};
use medulla_protocol_types::protocol::ResultPayload;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;

const GRAPH: &str = "__commons__";

fn corrector(tenant: &str) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_roles("decider", tenant, &["corrector"])
}

fn correction(id: &str, target: &str, after: &str) -> Correction {
    Correction {
        correction_id: id.into(),
        target: target.into(),
        before: "the wrong answer".into(),
        after: after.into(),
        produced_by: "agent:planner".into(),
        actor: "user:ada".into(),
        at_ms: 1_700_000_000_000,
        source: CorrectionSource::Human,
    }
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

fn record(c: Correction) -> CorrectionOp {
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
    let write = plan(&h, &corrector(TENANT), record(correction("c1", "answer:7", "right")))
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
    let nodes_before = core.node_ids().len();

    // No verified role `corrector`.
    let plain = VerifiedRequestContext::verified_for_test_with_roles("decider", TENANT, &["test"]);
    let error = refused(plan(&h, &plain, record(correction("c1", "answer:7", "right"))).await);
    assert!(error.starts_with(CORRECTION_ROLE_REQUIRED.as_str()), "{error}");

    // The actor produced the output it corrects.
    let mut own = correction("c2", "answer:7", "right");
    own.produced_by = own.actor.clone();
    let error = refused(plan(&h, &corrector(TENANT), record(own)).await);
    assert!(error.starts_with(OUTCOME_SELF_REPORTED.as_str()), "{error}");

    // The candidate rule contradicts a stored invariant.
    let contradicted = correction("c3", "eg:task", "right");
    let rule_id = commit_correction(&contradicted)
        .unwrap()
        .effect
        .rule_candidate
        .rule_id;
    publish_invariant(
        &h,
        &RuleInvariant {
            invariant_iri: "inv:no-such-rule".into(),
            forbidden_rule_id: rule_id,
            on_class: "eg:task".into(),
        },
    );
    let error = refused(plan(&h, &corrector(TENANT), record(contradicted)).await);
    assert!(error.starts_with("RULE_CONTRADICTS_INVARIANT"), "{error}");

    // An oversized value is the shape error of R702.1.
    let error = refused(
        plan(
            &h,
            &corrector(TENANT),
            record(correction("c4", "answer:7", &"x".repeat(8 * 1024 + 1))),
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
    let write = plan(&h, &corrector(TENANT), record(correction("c1", "answer:7", "right")))
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
    assert_eq!(read.correction, correction("c1", "answer:7", "right"));
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

    // A rejection yields the three effects and an outcome of 0.0, and names its source.
    let mut rejection = correction("r1", "answer:9", "better");
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
    let mut unprivileged = correction("r2", "answer:9", "better");
    unprivileged.source = CorrectionSource::ReviewRejection;
    let plain = VerifiedRequestContext::verified_for_test_with_roles("decider", TENANT, &["test"]);
    let error = refused(plan(&h, &plain, record(unprivileged)).await);
    assert!(error.starts_with(CORRECTION_ROLE_REQUIRED.as_str()), "{error}");

    // A review comment with text is refused; with digests it is stored, and the stored nodes
    // hold neither the output nor the comment text.
    let mut text = correction("r3", "answer:9", "please rename this variable");
    text.source = CorrectionSource::ReviewComment;
    let error = refused(plan(&h, &corrector(TENANT), record(text)).await);
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    let mut digests = correction("r4", "answer:9", &format!("sha256:{}", "c".repeat(64)));
    digests.before = format!("sha256:{}", "d".repeat(64));
    digests.source = CorrectionSource::ReviewComment;
    let write = plan(&h, &corrector(TENANT), record(digests.clone())).await.ok().expect("plans");
    commit(&core, &write);
    let receipt = reply::<CorrectionRecorded>(&write).receipt_id;
    let get = CorrectionOp::Get { receipt_id: receipt };
    let read: CorrectionRead = reply(&plan(&h, &corrector(TENANT), get).await.ok().expect("reads"));
    assert_eq!(read.correction, digests);
    assert_eq!(read.correction.source, CorrectionSource::ReviewComment);
}
