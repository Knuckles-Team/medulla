//! Served process decisions (MEDULLA-DECISION-ENGINE-R180.4, R180.5, R180.6): a stored
//! DMN decision table is evaluated and an exclusive branch of a sealed plan selects
//! its arm through that table, over the graph the question names.

use super::kernel_family_support::kernel_request;
use super::path_preference_tests::pins;
use super::promotion_tests::commit;
use super::task_plan_tests::{request_with, step};
use super::*;
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::decision::PremiseClass;

const GRAPH: &str = "dmn-graph";
const DECISION: &str = "https://example.org/dmn#discount";
const TABLE: &str = "https://example.org/dmn#t1";

fn node(label: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), label.into());
    for (key, value) in fields {
        object.insert((*key).into(), (*value).into());
    }
    rmp_serde::to_vec_named(&serde_json::Value::Object(object)).unwrap()
}

/// One table over `tier`: rule `r0` maps gold to `first`, rule `r1` maps any tier to `second`.
async fn seed_table(h: &Harness, hit_policy: &str, first: &str, second: &str) {
    let mut guard = h.state.write().await;
    guard
        .registry
        .create_graph(
            GRAPH,
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    let core = guard.registry.get(GRAPH).unwrap().core.clone();
    let rows: Vec<(String, Vec<u8>)> = vec![
        (
            DECISION.into(),
            node("DmnDecision", &[("name", "discount")]),
        ),
        (
            TABLE.into(),
            node(
                "DmnDecisionTable",
                &[("dmnDecision", DECISION), ("dmnHitPolicy", hit_policy)],
            ),
        ),
        (
            "in0".into(),
            node(
                "DmnInputClause",
                &[
                    ("dmnTable", TABLE),
                    ("dmnExpression", "tier"),
                    ("dmnClauseIndex", "0"),
                ],
            ),
        ),
        (
            "out0".into(),
            node(
                "DmnOutputClause",
                &[
                    ("dmnTable", TABLE),
                    ("name", "discount"),
                    ("dmnClauseIndex", "0"),
                ],
            ),
        ),
        (
            "r0".into(),
            node(
                "DmnRule",
                &[
                    ("dmnTable", TABLE),
                    ("dmnRuleIndex", "0"),
                    ("dmnInputEntry:0", "\"gold\""),
                    ("dmnOutputEntry:0", first),
                ],
            ),
        ),
        (
            "r1".into(),
            node(
                "DmnRule",
                &[
                    ("dmnTable", TABLE),
                    ("dmnRuleIndex", "1"),
                    ("dmnInputEntry:0", "-"),
                    ("dmnOutputEntry:0", second),
                ],
            ),
        ),
    ];
    for (id, blob) in rows {
        core.add_node(id, blob);
    }
}

fn request(
    schema: &ComponentDependency,
    kind: QuestionKind,
    body: serde_json::Value,
) -> DecideRequest {
    let mut request = kernel_request(
        schema,
        kind,
        "process_decision",
        TypedValue::Text(body.to_string()),
        &["decision"],
    );
    request.params = BoundedVec::new(vec![
        TypedParam {
            name: "graph".into(),
            value: TypedValue::Text(GRAPH.into()),
        },
        TypedParam {
            name: "process_decision".into(),
            value: TypedValue::Text(body.to_string()),
        },
    ])
    .unwrap();
    request
}

fn table_request(schema: &ComponentDependency, tier: Option<&str>) -> DecideRequest {
    let inputs = match tier {
        Some(tier) => serde_json::json!({"tier": {"type": "string", "value": tier}}),
        None => serde_json::json!({}),
    };
    request(
        schema,
        QuestionKind::DecisionTableEval,
        serde_json::json!({"decision_iri": DECISION, "inputs": inputs}),
    )
}

fn options(record: &StatisticalDecisionRecord) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            scores.iter().map(|score| score.option_id.clone()).collect()
        }
        other => panic!("an advisory answer, got {other:?}"),
    }
}

fn abstention(record: &StatisticalDecisionRecord) -> String {
    format!("{:?}", record.outcome)
}

/// A sealed plan whose exclusive branch `fork` takes `x` when the table holds, else `y`.
async fn seal_branch_plan(h: &Harness) -> String {
    let (pin, _) = publish_route_schema(h);
    let none = serde_json::json!({});
    let proposal = serde_json::json!({
        "tasks": [
            step("start", &[], none.clone()),
            step("fork", &["start"], serde_json::json!({"branch": {
                "kind": "exclusive",
                "arms": [
                    {"target_task_id": "x", "condition": {"decision_iri": DECISION}},
                    {"target_task_id": "y"}
                ],
                "default_arm": "y"}})),
            step("x", &["fork"], none.clone()),
            step("y", &["fork"], none.clone()),
            step("end", &["x", "y"], none),
        ],
        "output_task_id": "end",
        "max_concurrency": 4,
        "max_steps": 8
    });
    let batch = decide(h, request_with(&pin, proposal)).await.unwrap();
    let record = batch.records.as_slice()[0].clone();
    let digest = record.task_plan().unwrap().plan_digest.clone();
    commit(h, &record).await;
    digest
}

fn branch_request(schema: &ComponentDependency, plan_digest: &str, tier: &str) -> DecideRequest {
    request(
        schema,
        QuestionKind::BranchSelect,
        serde_json::json!({
            "plan_digest": plan_digest,
            "branch_task_id": "fork",
            "inputs": {"tier": {"type": "string", "value": tier}},
        }),
    )
}

// proves R180: Process decisions: DMN tables and gateway branches evaluate as Decide families
// spec: MEDULLA-DECISION-ENGINE-R180
// spec: MEDULLA-DECISION-ENGINE-R180.4
#[tokio::test]
async fn decide_routes_table_and_branch() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    seed_table(&h, "FIRST", "true", "false").await;

    let gold = decide(&h, table_request(&pins.0, Some("gold")))
        .await
        .unwrap();
    let chosen = options(&gold.records.as_slice()[0]);
    assert!(chosen.contains(&"rule:r0".to_string()), "{chosen:?}");
    assert!(chosen.iter().any(|option| option.starts_with("outputs:")));

    let digest = seal_branch_plan(&h).await;
    let taken = decide(&h, branch_request(&pins.0, &digest, "gold"))
        .await
        .unwrap();
    assert_eq!(
        options(&taken.records.as_slice()[0]),
        vec!["taken:x".to_string(), "default:false".to_string()]
    );
    let defaulted = decide(&h, branch_request(&pins.0, &digest, "bronze"))
        .await
        .unwrap();
    assert_eq!(
        options(&defaulted.records.as_slice()[0]),
        vec!["taken:y".to_string(), "default:true".to_string()]
    );
}

// spec: MEDULLA-DECISION-ENGINE-R180.5
#[tokio::test]
async fn decision_refusals_name_the_rule() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    // UNIQUE with two matching rules for gold.
    seed_table(&h, "UNIQUE", "1", "2").await;
    let batch = decide(&h, table_request(&pins.0, Some("gold")))
        .await
        .unwrap();
    let text = abstention(&batch.records.as_slice()[0]);
    assert!(
        text.contains("DMN_UNIQUE_VIOLATED") && text.contains("r0"),
        "{text}"
    );
    // A missing input names the input.
    let batch = decide(&h, table_request(&pins.0, None)).await.unwrap();
    let text = abstention(&batch.records.as_slice()[0]);
    assert!(
        text.contains("DMN_INPUT_MISSING") && text.contains("tier"),
        "{text}"
    );
    // An unknown plan digest.
    let batch = decide(&h, branch_request(&pins.0, "sha256:none", "gold"))
        .await
        .unwrap();
    assert!(abstention(&batch.records.as_slice()[0]).contains("PLAN_UNKNOWN"));
}

// spec: MEDULLA-DECISION-ENGINE-R180.6
#[tokio::test]
async fn premises_cite_rules_and_inputs() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    seed_table(&h, "FIRST", "true", "false").await;
    let batch = decide(&h, table_request(&pins.0, Some("gold")))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(
        facts.contains(&format!("dmn:rule=r0;table={DECISION};hit_policy=FIRST").as_str()),
        "{facts:?}"
    );
    assert!(facts.contains(&"input:tier=gold"), "{facts:?}");
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("dmn:rule="))
        .all(|p| p.class == PremiseClass::Observation));

    let digest = seal_branch_plan(&h).await;
    let branch = decide(&h, branch_request(&pins.0, &digest, "gold"))
        .await
        .unwrap();
    let facts: Vec<String> = branch.records.as_slice()[0]
        .premises
        .iter()
        .map(|p| p.fact.clone())
        .collect();
    assert!(
        facts.contains(&"branch:task=fork;taken=x;default=false".to_string()),
        "{facts:?}"
    );
}
