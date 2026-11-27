//! Shape policies over the result of a query (MEDULLA-FEDERATED-QUERY-R706.9, R706.10,
//! R706.11): each result node gets one of three outcomes, an invalid shape or a violation
//! refuses without a node value, and one evaluation appends one record with counts only.
#![cfg(all(feature = "query", feature = "owl"))]

use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::policy_mask::{
    apply_shape_policies, load_policy_set, policy_digest, PolicyEvaluationLog, PolicySet,
    ShapeEffect, ShapePolicy,
};
use medulla_exec::rowset::RowSet;
use medulla_graph_core::graph::{GraphCore, GraphView};
use medulla_query_types::wire::{Op, Plan};
use medulla_semantic_index::semantic::SemanticStore;
use medulla_types::ontology_base::CORE_NAMESPACE;
use serde_json::json;

/// A value no refusal or record may carry.
const SECRET_VALUE: &str = "ssn-078-05-1120";

fn blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

/// Three claims: a published one, a draft one and one marked secret.
fn graph() -> GraphView {
    let core = GraphCore::new();
    for (id, status) in [
        ("claim-ok", "published"),
        ("claim-draft", "draft"),
        ("claim-secret", "secret"),
    ] {
        core.add_node(
            id.into(),
            blob(json!({ "type": "Claim", "status": status, "ssn": SECRET_VALUE })),
        );
    }
    core.analysis_snapshot()
}

/// A shape every `:Claim` whose `:status` is `status` fails.
fn not_status(status: &str) -> String {
    format!(
        r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix : <{CORE_NAMESPACE}> .
:Not{status} a sh:NodeShape ;
    sh:targetClass :Claim ;
    sh:not [ sh:property [ sh:path :status ; sh:hasValue "{status}" ] ] .
"#
    )
}

fn shape(policy_id: &str, status: &str, effect: ShapeEffect) -> ShapePolicy {
    ShapePolicy {
        policy_id: policy_id.to_string(),
        shape_turtle: not_status(status),
        effect,
    }
}

fn set(shapes: Vec<ShapePolicy>) -> PolicySet {
    PolicySet {
        shapes,
        ..PolicySet::default()
    }
}

fn all_claims() -> RowSet {
    RowSet::from_ids(["claim-ok", "claim-draft", "claim-secret"].map(str::to_string))
}

// spec: MEDULLA-FEDERATED-QUERY-R706.9
#[test]
fn three_outcomes_on_a_fixed_graph() {
    let view = graph();
    let policies = set(vec![
        shape("policy:no-drafts", "draft", ShapeEffect::Filter),
        shape("policy:no-secrets", "secret", ShapeEffect::Refuse),
    ]);
    let evaluation = apply_shape_policies(all_claims(), &policies, &view).unwrap();
    // One node of each outcome: allowed stays, filtered leaves, violation refuses.
    assert_eq!(evaluation.counts.allowed, 1);
    assert_eq!(evaluation.counts.filtered, 1);
    assert_eq!(evaluation.counts.violation, 1);
    assert_eq!(evaluation.rows.ids(), vec!["claim-ok".to_string()]);
    assert_eq!(
        evaluation.violated_policy.as_deref(),
        Some("policy:no-secrets")
    );

    // With only the filter shape the draft leaves, the rest stay, and nothing refuses.
    let filter_only = set(vec![shape(
        "policy:no-drafts",
        "draft",
        ShapeEffect::Filter,
    )]);
    let evaluation = apply_shape_policies(all_claims(), &filter_only, &view).unwrap();
    assert_eq!(evaluation.violated_policy, None);
    assert_eq!(
        evaluation.rows.ids(),
        vec!["claim-ok".to_string(), "claim-secret".to_string()]
    );

    // A set with no shape returns every row, untouched.
    let untouched = apply_shape_policies(all_claims(), &PolicySet::default(), &view).unwrap();
    assert_eq!(untouched.rows.ids(), all_claims().ids());
    assert_eq!(untouched.counts.allowed, 3);
}

fn shape_record(id: &str, turtle: &str, effect: &str) -> (String, Vec<u8>) {
    (
        id.to_string(),
        blob(json!({
            "type": "QueryPolicy",
            "kind": "shape",
            "shape_turtle": turtle,
            "effect": effect,
        })),
    )
}

fn view_of(records: Vec<(String, Vec<u8>)>) -> GraphView {
    let core = GraphCore::new();
    for (id, properties) in records {
        core.add_node(id, properties);
    }
    core.analysis_snapshot()
}

// spec: MEDULLA-FEDERATED-QUERY-R706.10
#[test]
fn invalid_shape_and_violation_refuse_without_values() {
    // A shape that does not parse refuses at load, naming the policy and the field.
    let broken = view_of(vec![shape_record(
        "policy:broken",
        "this is not turtle {{{",
        "refuse",
    )]);
    let error = load_policy_set("acme", &broken).unwrap_err();
    assert_eq!(error.code, "POLICY_SHAPE_INVALID");
    assert_eq!(error.record, "policy:broken");
    assert_eq!(error.field, "shape_turtle");

    // A shape that parses loads, and so does the effect it names.
    let good = view_of(vec![shape_record(
        "policy:no-secrets",
        &not_status("secret"),
        "refuse",
    )]);
    let loaded = load_policy_set("acme", &good).unwrap();
    assert_eq!(loaded.shapes.len(), 1);
    assert_eq!(loaded.shapes[0].effect, ShapeEffect::Refuse);

    // An effect outside the closed set is a malformed record.
    let unknown_effect = view_of(vec![shape_record(
        "policy:odd",
        &not_status("secret"),
        "delete",
    )]);
    assert_eq!(
        load_policy_set("acme", &unknown_effect).unwrap_err().code,
        "POLICY_RECORD_INVALID"
    );

    // A violation refuses the whole request: no rows, the policy id, no node value.
    let view = graph();
    let semantic = SemanticStore::new();
    let policies = set(vec![shape(
        "policy:no-secrets",
        "secret",
        ShapeEffect::Refuse,
    )]);
    let ctx = PlanCtx::new(&view, &semantic)
        .with_policies(&policies)
        .with_principal("role:analyst");
    let plan = Plan::new(vec![Op::Scan {
        label: "Claim".into(),
    }]);
    let refusal = execute(&plan, &ctx).unwrap_err();
    assert!(refusal.starts_with("POLICY_VIOLATION"), "{refusal}");
    assert!(refusal.contains("policy:no-secrets"), "{refusal}");
    assert!(!refusal.contains(SECRET_VALUE), "{refusal}");
    assert!(!refusal.contains("claim-secret"), "{refusal}");
}

// spec: MEDULLA-FEDERATED-QUERY-R706.11
#[test]
fn evaluation_appends_one_audit_record_with_counts_only() {
    let view = graph();
    let semantic = SemanticStore::new();
    let policies = set(vec![
        shape("policy:no-drafts", "draft", ShapeEffect::Filter),
        shape("policy:no-secrets", "secret", ShapeEffect::Refuse),
    ]);
    let log = PolicyEvaluationLog::default();
    let ctx = PlanCtx::new(&view, &semantic)
        .with_policies(&policies)
        .with_principal("role:analyst")
        .with_policy_audit(&log);
    let plan = Plan::new(vec![Op::Scan {
        label: "Claim".into(),
    }]);
    // The request is refused, and its evaluation is still recorded, once.
    assert!(execute(&plan, &ctx).is_err());
    let records = log.take();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.policy_digest, policy_digest(&policies));
    assert_eq!(record.principal, "role:analyst");
    assert_eq!(
        (
            record.counts.allowed,
            record.counts.filtered,
            record.counts.violation
        ),
        (1, 1, 1)
    );
    let text = format!("{record:?}");
    assert!(!text.contains(SECRET_VALUE), "{text}");
    assert!(!text.contains("claim-"), "{text}");

    // A request under a set with no shape records nothing.
    let empty = PolicySet::default();
    let plain = PlanCtx::new(&view, &semantic)
        .with_policies(&empty)
        .with_policy_audit(&log);
    assert!(execute(&plan, &plain).is_ok());
    assert!(log.take().is_empty());
}

// spec: MEDULLA-FEDERATED-QUERY-R706.12
#[test]
fn rows_of_any_language_take_the_worst_outcome_of_their_nodes() {
    use medulla_exec::policy_mask::judge_rows;

    let view = graph();
    let policies = set(vec![
        shape("policy:no-drafts", "draft", ShapeEffect::Filter),
        shape("policy:no-secrets", "secret", ShapeEffect::Refuse),
    ]);
    let nodes = |ids: &[&str]| ids.iter().map(ToString::to_string).collect::<Vec<_>>();

    // A SPARQL-style row binds several nodes and takes the worst verdict among them; a row
    // that binds no node (a SQL row of an unmapped table, a solution of literals) stays.
    let rows = vec![
        nodes(&["claim-ok"]),
        nodes(&["claim-ok", "claim-draft"]),
        nodes(&[]),
        nodes(&["claim-ok", "claim-draft", "claim-secret"]),
    ];
    let verdicts = judge_rows(&policies, &view, &rows).unwrap();
    assert_eq!(verdicts.keep, vec![true, false, true, false]);
    assert_eq!(verdicts.counts.allowed, 2);
    assert_eq!(verdicts.counts.filtered, 1);
    assert_eq!(verdicts.counts.violation, 1);
    assert_eq!(
        verdicts.violated_policy.as_deref(),
        Some("policy:no-secrets")
    );

    // The same nodes through UQL's row set give the same verdicts: one function judges both.
    let uql = apply_shape_policies(all_claims(), &policies, &view).unwrap();
    let single: Vec<Vec<String>> = all_claims().ids().into_iter().map(|id| vec![id]).collect();
    let by_rows = judge_rows(&policies, &view, &single).unwrap();
    assert_eq!(uql.counts, by_rows.counts);
    assert_eq!(uql.violated_policy, by_rows.violated_policy);

    // No shape judges every row allowed without reading the graph.
    let none = judge_rows(&PolicySet::default(), &view, &rows).unwrap();
    assert_eq!(none.keep, vec![true; 4]);
    assert_eq!(none.counts.allowed, 4);
}
