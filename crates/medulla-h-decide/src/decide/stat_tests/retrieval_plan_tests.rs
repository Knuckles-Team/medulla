//! Proven retrieval-plan templates are facts keyed by (task class, composed
//! schema digest) (MEDULLA-TYPED-PACKS-R078): the candidate source reads a
//! template only under the schema digest that proved it, a newer digest hides
//! the older template without deleting it, and another tenant's proven
//! templates never enter this tenant's candidates.

use super::retrieval_tests::{
    attest, clone_executed, evaluate_by, executed_template, learn, outcome, sql, template,
};
use super::*;
use medulla_decision_types::decision::statistical::retrieval::{
    RetrievalPathTemplate, StoredRetrievalOutcome,
};
use serde_json::json;

const TASK_CLASS: &str = "urn:task:incident-triage";

// spec: MEDULLA-TYPED-PACKS-R078
#[tokio::test]
async fn templates_are_keyed_by_task_class_and_schema_digest() {
    let h = Harness::new().await;
    let template_entry = executed_template(&h).await;

    // Prove a template under schema digest A.
    clone_executed(&h, &template_entry, "rec-a");
    let mut attested_a = outcome("rec-a", &["x"], &["x"]);
    attested_a.path = Some(template("sha256:schema-a"));
    learn(&h, "decider", attest(attested_a)).await.unwrap();
    evaluate_by(&h, "evaluator", "rec-a", true).await;
    let rows = sql(
        &h,
        "decider",
        "SELECT task_class, composed_digest, skill_ref, successes, failures \
         FROM decision_proven_paths",
    )
    .await;
    assert_eq!(
        rows,
        vec![vec![
            json!(TASK_CLASS),
            json!("sha256:schema-a"),
            json!("skill.incident-triage"),
            json!(1),
            json!(0)
        ]],
        "the proven template is a fact keyed by task class and schema digest"
    );
    // The topology-to-skill edges are stored beside the template.
    let edges = sql(
        &h,
        "decider",
        "SELECT template_json FROM decision_proven_paths WHERE composed_digest = 'sha256:schema-a'",
    )
    .await;
    let stored: RetrievalPathTemplate = serde_json::from_str(edges[0][0].as_str().unwrap()).unwrap();
    assert_eq!(stored.skill_ref, Some("skill.incident-triage".to_string()));
    assert_eq!(stored.edges.as_slice().first().unwrap().relationship, "AFFECTS");

    // Prove a second template under the newer schema digest B.
    clone_executed(&h, &template_entry, "rec-b");
    let mut attested_b = outcome("rec-b", &["y"], &["y"]);
    attested_b.path = Some(template("sha256:schema-b"));
    learn(&h, "decider", attest(attested_b)).await.unwrap();
    evaluate_by(&h, "evaluator", "rec-b", true).await;
    // The candidate source for the new schema digest sees only the template
    // keyed to it -- the older digest's template is hidden from candidates...
    let candidates = sql(
        &h,
        "decider",
        &format!(
            "SELECT composed_digest FROM decision_proven_paths \
             WHERE task_class = '{TASK_CLASS}' AND composed_digest = 'sha256:schema-b'"
        ),
    )
    .await;
    assert_eq!(
        candidates,
        vec![vec![json!("sha256:schema-b")]],
        "a new schema digest hides earlier templates from candidates"
    );
    // ...without deleting it: it stays readable.
    let still = sql(
        &h,
        "decider",
        "SELECT composed_digest FROM decision_proven_paths WHERE composed_digest = 'sha256:schema-a'",
    )
    .await;
    assert_eq!(still, vec![vec![json!("sha256:schema-a")]]);

    // Cross-tenant negative: a write for another tenant is refused at the
    // served boundary, and a foreign tenant's proven template never enters
    // this tenant's candidate source.
    let foreign_learn = medulla_decision_types::decision::statistical::log::DecisionLogOp::Learn {
        tenant_id: "tenant-foreign".to_string(),
        write: medulla_decision_types::decision::statistical::retrieval::LearningWrite::RecordOutcome {
            outcome: Box::new(outcome("rec-foreign", &["z"], &["z"])),
        },
    };
    let refused = decode::<String>(log_op(&h, "decider", foreign_learn).await);
    assert!(
        refused.unwrap_err().starts_with("ACCESS_DENIED"),
        "a template write for another tenant is refused"
    );
    let foreign = outcome("rec-foreign", &["z"], &["z"]);
    let stored_outcome = StoredRetrievalOutcome {
        outcome: foreign,
        producer: "decider".to_string(),
        recorded_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    };
    h.store
        .put_decision_artifacts(
            "tenant-foreign",
            &[(
                "retrieval:rec-foreign".to_string(),
                medulla_persistence_records::decision_jobs::encode_artifact(&stored_outcome)
                    .unwrap(),
            )],
        )
        .unwrap();
    assert!(
        h.store
            .decision_artifact("tenant-foreign", "retrieval:rec-foreign")
            .unwrap()
            .is_some(),
        "the foreign fact is durable under its own tenant"
    );
    let all = sql(
        &h,
        "decider",
        "SELECT composed_digest FROM decision_proven_paths ORDER BY composed_digest",
    )
    .await;
    assert_eq!(
        all,
        vec![vec![json!("sha256:schema-a")], vec![json!("sha256:schema-b")]],
        "another tenant's proven templates never enter this tenant's candidates"
    );
}
