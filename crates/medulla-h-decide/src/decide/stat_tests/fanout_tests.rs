//! MEDULLA-DECISION-ENGINE-R134.100: a served `FanoutAggregate` question
//! applies the plan's operator to the complete receipt set, names failure
//! gaps, refuses every incomplete or foreign set with a typed code, and reads
//! back through `DecisionLog` `Get` with a recomputable aggregate digest.

use super::log_tests::log_op;
use super::*;
use medulla_decision_types::decision::task_plan::fanout::{
    aggregate_digest, ChildSummary, FanoutAggregateRequest, FanoutBudget, FanoutEntry,
    FanoutStatus, FanoutSubtaskResult, FinalArtifactReceipt,
};
use medulla_decision_types::decision::task_plan::{
    plan_explicit_tasks, AggregationOp, FinalArtifactDecl, TaskPlanAggregation, TaskPlanProposal,
    TaskPlanTask,
};

const GRAPH: &str = "kg-fanout";
const OTHER_TENANT: &str = "tenant-other";

fn digest(c: char) -> String {
    c.to_string().repeat(64)
}

fn plan() -> String {
    digest('9')
}

fn ok(task: &str, c: char) -> FanoutSubtaskResult {
    FanoutSubtaskResult {
        task_id: task.to_string(),
        status: FanoutStatus::Ok,
        result_digest: Some(digest(c)),
        receipt_id: format!("receipt-{task}"),
        receipt_digest: digest('7'),
        failure_gap: None,
        summary: None,
    }
}

fn failed(task: &str, gap: &str) -> FanoutSubtaskResult {
    FanoutSubtaskResult {
        task_id: task.to_string(),
        status: FanoutStatus::Failed,
        result_digest: None,
        receipt_id: format!("receipt-{task}"),
        receipt_digest: digest('7'),
        failure_gap: Some(gap.to_string()),
        summary: None,
    }
}

/// The receipts of tasks t1..t4 in the caller's tenant, one foreign receipt
/// and nothing else.
async fn seed(h: &Harness) {
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
    for task in ["t1", "t2", "t3", "t4"] {
        let props = serde_json::json!({"type": "Receipt", "tenant_id": TENANT});
        core.add_node(
            format!("receipt-{task}"),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }
    let props = serde_json::json!({"type": "Receipt", "tenant_id": OTHER_TENANT});
    core.add_node(
        "receipt-foreign".to_string(),
        rmp_serde::to_vec_named(&props).unwrap(),
    );
    // The stored final artifact the coordinator's delivery is read back from,
    // and one stored in another tenant.
    for (id, tenant) in [("artifact-1", TENANT), ("artifact-foreign", OTHER_TENANT)] {
        let props = serde_json::json!({
            "type": "Artifact", "tenant_id": tenant, "digest": digest('d')
        });
        core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    }
}

fn fanout_request(
    h: &Harness,
    op: AggregationOp,
    reducer: Option<&str>,
    plan_digest: String,
    expected: &[&str],
    results: Vec<FanoutSubtaskResult>,
) -> DecideRequest {
    fanout_request_with(
        h,
        op,
        reducer,
        plan_digest,
        expected,
        results,
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn fanout_request_with(
    h: &Harness,
    op: AggregationOp,
    reducer: Option<&str>,
    plan_digest: String,
    expected: &[&str],
    results: Vec<FanoutSubtaskResult>,
    budget: Option<FanoutBudget>,
    plan: Option<TaskPlanProposal>,
    final_artifact_receipt: Option<FinalArtifactReceipt>,
) -> DecideRequest {
    let (schema, _) = publish_route_schema(h);
    declared_request(
        &schema,
        TENANT,
        QuestionKind::FanoutAggregate,
        "aggregate",
        DecisionPolicyRef::Default,
        vec![
            TypedParam {
                name: "fanout".to_string(),
                value: TypedValue::FanoutResults(Box::new(FanoutAggregateRequest {
                    plan_digest,
                    aggregation: TaskPlanAggregation {
                        op,
                        reducer_id: reducer.map(str::to_string),
                    },
                    expected_task_ids: BoundedVec::new(
                        expected.iter().map(|id| id.to_string()).collect(),
                    )
                    .unwrap(),
                    results: BoundedVec::new(results).unwrap(),
                    budget,
                    plan,
                    final_artifact_receipt,
                })),
            },
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Text(GRAPH.to_string()),
            },
        ],
    )
}

fn options(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an Advisory outcome: {:?}", record.outcome)
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

fn result_options(chars: &[char]) -> Vec<String> {
    chars
        .iter()
        .map(|c| format!("result:{}", digest(*c)))
        .collect()
}

fn fact<'a>(record: &'a StatisticalDecisionRecord, prefix: &str) -> Vec<&'a str> {
    record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with(prefix))
        .map(|p| p.fact.as_str())
        .collect()
}

fn abstained_with(record: &StatisticalDecisionRecord, code: &str) {
    assert!(
        matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
        "{:?}",
        record.outcome
    );
    assert!(
        format!("{:?}", record.outcome).contains(code),
        "{:?}",
        record.outcome
    );
}

async fn matrix(h: &Harness) {
    let tasks = ["t1", "t2", "t3"];
    // (operator, reducer, all-success digests, expected outputs, outputs with t3 failed)
    let matrix: Vec<(AggregationOp, Option<&str>, [char; 3], Vec<char>, Vec<char>)> = vec![
        (
            AggregationOp::Concat,
            None,
            ['a', 'b', 'c'],
            vec!['a', 'b', 'c'],
            vec!['a', 'b'],
        ),
        // a appears twice: the majority; with t3 failed, a and b tie and the lowest task id wins.
        (
            AggregationOp::MajorityVote,
            None,
            ['a', 'b', 'a'],
            vec!['a'],
            vec!['a'],
        ),
        (
            AggregationOp::FirstValid,
            None,
            ['a', 'b', 'c'],
            vec!['a'],
            vec!['a'],
        ),
        (
            AggregationOp::Reduce,
            Some("sum-reducer"),
            ['a', 'b', 'c'],
            vec!['a', 'b', 'c'],
            vec!['a', 'b'],
        ),
    ];
    for (op, reducer, digests, all_ok, with_gap) in matrix {
        let success: Vec<FanoutSubtaskResult> = tasks
            .iter()
            .zip(digests)
            .map(|(task, c)| ok(task, c))
            .collect();
        let batch = decide(&h, fanout_request(&h, op, reducer, plan(), &tasks, success))
            .await
            .unwrap();
        let record = &batch.records.as_slice()[0];
        assert_eq!(options(record), result_options(&all_ok), "{op:?}");
        assert_eq!(fact(record, "fanout_result:").len(), 3);
        assert!(fact(record, "fanout_result:")[0]
            .starts_with("fanout_result:task=t1;status=ok;receipt=receipt-t1"));

        let mut gapped: Vec<FanoutSubtaskResult> = vec![ok("t1", digests[0]), ok("t2", digests[1])];
        gapped.push(failed("t3", "tool timed out"));
        let batch = decide(&h, fanout_request(&h, op, reducer, plan(), &tasks, gapped))
            .await
            .unwrap();
        let record = &batch.records.as_slice()[0];
        assert_eq!(
            options(record),
            result_options(&with_gap),
            "{op:?} with a gap"
        );
        assert!(
            fact(record, "fanout_entry:")
                .contains(&"fanout_entry:task=t3;status=failed;value=tool timed out"),
            "the failure gap is named: {:?}",
            fact(record, "fanout_entry:")
        );
    }

    // A tie goes to the lowest task id carrying the digest, not the smaller digest.
    let tie = vec![ok("t1", 'b'), ok("t2", 'a')];
    let batch = decide(
        &h,
        fanout_request(
            &h,
            AggregationOp::MajorityVote,
            None,
            plan(),
            &["t1", "t2"],
            tie,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        options(&batch.records.as_slice()[0]),
        result_options(&['b'])
    );
}

async fn refusals(h: &Harness) {
    let run = |results: Vec<FanoutSubtaskResult>, plan_digest: String, tasks: &[&str]| {
        fanout_request(&h, AggregationOp::Concat, None, plan_digest, tasks, results)
    };
    let tasks = ["t1", "t2", "t3"];
    let record = decide(&h, run(vec![ok("t1", 'a'), ok("t2", 'b')], plan(), &tasks))
        .await
        .unwrap();
    abstained_with(&record.records.as_slice()[0], "FANOUT_CHILD_MISSING");
    let record = decide(
        &h,
        run(
            vec![ok("t1", 'a'), ok("t1", 'a'), ok("t2", 'b'), ok("t3", 'c')],
            plan(),
            &tasks,
        ),
    )
    .await
    .unwrap();
    abstained_with(&record.records.as_slice()[0], "FANOUT_CHILD_DUPLICATE");
    let record = decide(
        &h,
        run(
            vec![ok("t1", 'a'), ok("t2", 'b'), ok("t4", 'c')],
            plan(),
            &tasks,
        ),
    )
    .await
    .unwrap();
    abstained_with(&record.records.as_slice()[0], "FANOUT_CHILD_UNKNOWN");
    let record = decide(
        &h,
        run(
            vec![ok("t1", 'a'), ok("t2", 'b'), ok("t3", 'c')],
            "not-a-digest".to_string(),
            &tasks,
        ),
    )
    .await
    .unwrap();
    abstained_with(&record.records.as_slice()[0], "FANOUT_PLAN_MISMATCH");
    let record = decide(
        &h,
        run(
            vec![failed("t1", "x"), failed("t2", "y"), failed("t3", "z")],
            plan(),
            &tasks,
        ),
    )
    .await
    .unwrap();
    abstained_with(&record.records.as_slice()[0], "FANOUT_NO_VALID_RESULT");

    // A receipt in another tenant refuses exactly as a receipt that does not exist.
    let mut foreign = ok("t3", 'c');
    foreign.receipt_id = "receipt-foreign".to_string();
    let mut missing = ok("t3", 'c');
    missing.receipt_id = "receipt-none".to_string();
    let mut texts = Vec::new();
    for bad in [foreign, missing] {
        let refused = decide(
            &h,
            run(vec![ok("t1", 'a'), ok("t2", 'b'), bad], plan(), &tasks),
        )
        .await
        .unwrap_err();
        assert!(refused.contains("FANOUT_CHILD_UNKNOWN"), "{refused}");
        texts.push(refused);
    }
    assert_eq!(
        texts[0], texts[1],
        "the refusal never confirms the row exists"
    );
}

async fn readback(h: &Harness) {
    let tasks = ["t1", "t2", "t3"];
    let results = vec![ok("t1", 'a'), ok("t2", 'b'), failed("t3", "no route")];
    let batch = decide(
        &h,
        fanout_request(&h, AggregationOp::Concat, None, plan(), &tasks, results),
    )
    .await
    .unwrap();
    let record = batch.records.as_slice()[0].clone();
    let committed: DecisionLogCommitted = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            },
        )
        .await,
    )
    .unwrap();
    let get = |tenant: &str| DecisionLogOp::Get {
        tenant_id: tenant.to_string(),
        record_id: committed.record_id.clone(),
    };
    let entry: DecisionLogEntry =
        decode::<Option<DecisionLogEntry>>(log_op(&h, "decider", get(TENANT)).await)
            .unwrap()
            .expect("the committed record");
    let stored = entry.record;
    let entries: Vec<FanoutEntry> = stored
        .premises
        .iter()
        .filter_map(|p| p.fact.strip_prefix("fanout_entry:"))
        .map(|rest| {
            let field = |name: &str| {
                rest.split(';')
                    .find_map(|part| part.strip_prefix(name))
                    .unwrap()
                    .to_string()
            };
            FanoutEntry {
                task_id: field("task="),
                status: if field("status=") == "ok" {
                    FanoutStatus::Ok
                } else {
                    FanoutStatus::Failed
                },
                value: field("value="),
            }
        })
        .collect();
    let line = stored
        .premises
        .iter()
        .find_map(|p| p.fact.strip_prefix("fanout_aggregate:"))
        .expect("the aggregate premise");
    let recorded = line.rsplit("aggregate_digest=").next().unwrap();
    assert_eq!(recorded.len(), 64);
    assert_eq!(
        aggregate_digest(AggregationOp::Concat, &entries, &plan()),
        recorded,
        "readback recomputes the digest"
    );
    // Changing one entry changes the digest.
    let mut changed = entries.clone();
    changed[0].value = digest('f');
    assert_ne!(
        aggregate_digest(AggregationOp::Concat, &changed, &plan()),
        recorded
    );

    let refused =
        decode::<Option<DecisionLogEntry>>(log_op(&h, "decider", get(OTHER_TENANT)).await);
    assert!(refused.unwrap_err().starts_with("ACCESS_DENIED"));
}

/// One served journey: the operator matrix with named gaps, every typed
/// refusal including a foreign receipt, and the readback that recomputes the
/// aggregate digest.
// spec: MEDULLA-DECISION-ENGINE-R134.100
#[tokio::test]
async fn fanout_aggregate_applies_the_plan_operator_and_names_gaps() {
    let h = Harness::new().await;
    seed(&h).await;
    matrix(&h).await;
    refusals(&h).await;
    readback(&h).await;
}

const TOKENIZER: &str = "tok-1";

fn summarised(task: &str, c: char, tokens: u64) -> FanoutSubtaskResult {
    let mut result = ok(task, c);
    result.summary = Some(ChildSummary {
        artifact_kind: "summary".to_string(),
        schema_id: "child-summary".to_string(),
        schema_version: 1,
        summary_digest: digest(c),
        summary_tokens: tokens,
        tokenizer_revision: TOKENIZER.to_string(),
    });
    result
}

fn budget(remaining: u64, max: Option<u64>) -> FanoutBudget {
    FanoutBudget {
        parent_remaining_tokens: remaining,
        max_summary_tokens: max,
        tokenizer_revision: TOKENIZER.to_string(),
    }
}

async fn budgeted(
    h: &Harness,
    results: Vec<FanoutSubtaskResult>,
    budget: FanoutBudget,
) -> Result<DecisionBatch, String> {
    let request = fanout_request_with(
        h,
        AggregationOp::Concat,
        None,
        plan(),
        &["t1", "t2", "t3"],
        results,
        Some(budget),
        None,
        None,
    );
    decide(h, request).await
}

/// One served journey: summaries that fit the budget link decision, child and
/// summary digests; raw, oversized, over-budget and tokenizer-mismatched input
/// each refuse with their own code; a foreign receipt refuses like a missing one.
// spec: MEDULLA-DECISION-ENGINE-R134.10
#[tokio::test]
async fn summary_only_children_fit_the_parent_budget() {
    let h = Harness::new().await;
    seed(&h).await;
    let fits = vec![
        summarised("t1", 'a', 100),
        summarised("t2", 'b', 200),
        summarised("t3", 'c', 300),
    ];
    let batch = budgeted(&h, fits.clone(), budget(1000, None))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(options(record), result_options(&['a', 'b', 'c']));
    assert_eq!(
        fact(record, "child_summary:"),
        vec![
            format!("child_summary:child=t1;digest={};tokens=100", digest('a')),
            format!("child_summary:child=t2;digest={};tokens=200", digest('b')),
            format!("child_summary:child=t3;digest={};tokens=300", digest('c')),
        ]
    );
    assert_eq!(
        fact(record, "summary_budget:"),
        vec![format!(
            "summary_budget:plan={};total=600;limit=1000;remaining=400",
            plan()
        )]
    );
    assert_eq!(
        fact(record, "fanout.knobs:"),
        vec!["fanout.knobs:max_child_summary_tokens=2048;max_summary_tokens=4096"]
    );
    assert!(!fact(record, "fanout_aggregate:").is_empty());

    // The request's own cap tightens the default one; the total is checked.
    let refused = budgeted(&h, fits.clone(), budget(1000, Some(500))).await;
    abstained_with(
        &refused.unwrap().records.as_slice()[0],
        "SUMMARY_BUDGET_EXCEEDED",
    );
    let refused = budgeted(&h, fits.clone(), budget(599, None)).await;
    abstained_with(
        &refused.unwrap().records.as_slice()[0],
        "SUMMARY_BUDGET_EXCEEDED",
    );
    let mut raw = fits.clone();
    raw[1].summary.as_mut().unwrap().artifact_kind = "raw_output".to_string();
    let refused = budgeted(&h, raw, budget(1000, None)).await.unwrap();
    abstained_with(&refused.records.as_slice()[0], "RAW_RETURN_REFUSED");
    let mut absent = fits.clone();
    absent[2].summary = None;
    let refused = budgeted(&h, absent, budget(1000, None)).await.unwrap();
    abstained_with(&refused.records.as_slice()[0], "RAW_RETURN_REFUSED");
    let mut big = fits.clone();
    big[0].summary.as_mut().unwrap().summary_tokens = 2049;
    let refused = budgeted(&h, big, budget(100_000, Some(60_000)))
        .await
        .unwrap();
    abstained_with(&refused.records.as_slice()[0], "SUMMARY_OVERSIZED");
    let mut other = fits.clone();
    other[0].summary.as_mut().unwrap().tokenizer_revision = "tok-2".to_string();
    let refused = budgeted(&h, other, budget(1000, None)).await.unwrap();
    abstained_with(&refused.records.as_slice()[0], "TOKENIZER_MISMATCH");

    // A failed child needs a named gap, not a summary; the rest still fit.
    let gapped = vec![
        summarised("t1", 'a', 100),
        summarised("t2", 'b', 200),
        failed("t3", "tool timed out"),
    ];
    let batch = budgeted(&h, gapped, budget(1000, None)).await.unwrap();
    assert_eq!(
        fact(&batch.records.as_slice()[0], "summary_budget:"),
        vec![format!(
            "summary_budget:plan={};total=300;limit=1000;remaining=700",
            plan()
        )]
    );

    // Missing, duplicate and unknown children refuse before any summary is counted.
    let refused = budgeted(&h, fits[..2].to_vec(), budget(1000, None))
        .await
        .unwrap();
    abstained_with(&refused.records.as_slice()[0], "FANOUT_CHILD_MISSING");
    let mut dup = fits.clone();
    dup.push(fits[0].clone());
    let refused = budgeted(&h, dup, budget(1000, None)).await.unwrap();
    abstained_with(&refused.records.as_slice()[0], "FANOUT_CHILD_DUPLICATE");

    // A receipt in another tenant refuses exactly as a receipt that does not exist.
    let mut foreign = fits.clone();
    foreign[2].receipt_id = "receipt-foreign".to_string();
    let mut missing = fits.clone();
    missing[2].receipt_id = "receipt-none".to_string();
    let mut texts = Vec::new();
    for bad in [foreign, missing] {
        let refused = budgeted(&h, bad, budget(1000, None)).await.unwrap_err();
        assert!(refused.contains("FANOUT_CHILD_UNKNOWN"), "{refused}");
        texts.push(refused);
    }
    assert_eq!(texts[0], texts[1]);

    // The sealed record reads back through the decision log with its digests.
    let committed: DecisionLogCommitted = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            },
        )
        .await,
    )
    .unwrap();
    let entry: DecisionLogEntry = decode::<Option<DecisionLogEntry>>(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Get {
                tenant_id: TENANT.to_string(),
                record_id: committed.record_id.clone(),
            },
        )
        .await,
    )
    .unwrap()
    .expect("the committed record");
    assert_eq!(fact(&entry.record, "child_summary:").len(), 3);
    assert_eq!(entry.record.premises.len(), record.premises.len());
}

fn plan_task(id: &str, deps: &[&str]) -> TaskPlanTask {
    serde_json::from_value(serde_json::json!({
        "task_id": id, "prompt": format!("do {id}"), "depends_on": deps
    }))
    .unwrap()
}

fn coordinator_plan(artifact: Option<FinalArtifactDecl>) -> TaskPlanProposal {
    TaskPlanProposal {
        tasks: BoundedVec::new(vec![
            plan_task("t1", &[]),
            plan_task("t2", &[]),
            plan_task("t3", &[]),
            plan_task("t4", &[]),
            plan_task("out", &["t1", "t2", "t3", "t4"]),
        ])
        .unwrap(),
        output_task_id: "out".to_string(),
        max_concurrency: 2,
        max_steps: 8,
        final_artifact: artifact,
    }
}

fn declared() -> FinalArtifactDecl {
    FinalArtifactDecl {
        schema_id: "report".to_string(),
        schema_version: 2,
        kind: "markdown".to_string(),
    }
}

fn delivery_receipt() -> FinalArtifactReceipt {
    FinalArtifactReceipt {
        finalizer_task_id: "out".to_string(),
        schema_id: "report".to_string(),
        schema_version: 2,
        kind: "markdown".to_string(),
        artifact_digest: digest('d'),
        readback_id: "artifact-1".to_string(),
    }
}

async fn deliver(
    h: &Harness,
    proposal: TaskPlanProposal,
    receipt: FinalArtifactReceipt,
) -> Result<DecisionBatch, String> {
    let plan_digest = plan_explicit_tasks(&proposal).unwrap().plan_digest;
    let results = vec![
        ok("t1", 'a'),
        ok("t2", 'b'),
        ok("t3", 'c'),
        failed("t4", "no route"),
    ];
    let request = fanout_request_with(
        h,
        AggregationOp::Concat,
        None,
        plan_digest,
        &["t1", "t2", "t3", "t4"],
        results,
        None,
        Some(proposal),
        Some(receipt),
    );
    decide(h, request).await
}

/// One served journey: three outputs and one named failed gap through the
/// coordinator read back as one delivery record; every refusal; a foreign
/// readback and a plan digest that is not the carried plan's.
// spec: MEDULLA-DECISION-ENGINE-R134.101
#[tokio::test]
async fn coordinator_delivers_one_complete_artifact() {
    let h = Harness::new().await;
    seed(&h).await;
    let batch = deliver(&h, coordinator_plan(Some(declared())), delivery_receipt())
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(options(&record), result_options(&['a', 'b', 'c']));
    assert_eq!(
        fact(&record, "final_artifact:"),
        vec![format!(
            "final_artifact:schema=report@2;kind=markdown;digest={}",
            digest('d')
        )]
    );
    let delivery = fact(&record, "fanout_delivery:");
    assert_eq!(delivery.len(), 1);
    assert!(delivery[0].contains(";coordinator=out;receipts=receipt-t1,receipt-t2,receipt-t3,receipt-t4;readback=artifact-1;digest="));
    assert!(fact(&record, "fanout_entry:")
        .contains(&"fanout_entry:task=t4;status=failed;value=no route"));

    // One record reads back through the decision log with the same delivery.
    let committed: DecisionLogCommitted = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            },
        )
        .await,
    )
    .unwrap();
    let get = |tenant: &str| DecisionLogOp::Get {
        tenant_id: tenant.to_string(),
        record_id: committed.record_id.clone(),
    };
    let stored: DecisionLogEntry =
        decode::<Option<DecisionLogEntry>>(log_op(&h, "decider", get(TENANT)).await)
            .unwrap()
            .expect("the committed record");
    assert_eq!(
        fact(&stored.record, "fanout_delivery:"),
        fact(&record, "fanout_delivery:")
    );
    let denied = decode::<Option<DecisionLogEntry>>(log_op(&h, "decider", get(OTHER_TENANT)).await);
    assert!(denied.unwrap_err().starts_with("ACCESS_DENIED"));

    let refused = |batch: Result<DecisionBatch, String>, code: &str| match batch {
        Ok(batch) => abstained_with(&batch.records.as_slice()[0], code),
        Err(text) => assert!(text.contains(code), "{text}"),
    };
    refused(
        deliver(&h, coordinator_plan(None), delivery_receipt()).await,
        "COORDINATOR_ABSENT",
    );
    let mut receipt = delivery_receipt();
    receipt.finalizer_task_id = "t1".to_string();
    refused(
        deliver(&h, coordinator_plan(Some(declared())), receipt).await,
        "NON_COORDINATOR_FINALIZE",
    );
    for edit in [
        (|r: &mut FinalArtifactReceipt| r.schema_id = "other".to_string())
            as fn(&mut FinalArtifactReceipt),
        |r| r.schema_version = 3,
        |r| r.kind = "json".to_string(),
    ] {
        let mut receipt = delivery_receipt();
        edit(&mut receipt);
        refused(
            deliver(&h, coordinator_plan(Some(declared())), receipt).await,
            "ARTIFACT_SCHEMA_MISMATCH",
        );
    }
    // A digest that is not the stored readback's, a malformed one, a missing
    // readback and another tenant's readback all refuse the same way.
    let mut texts = Vec::new();
    for edit in [
        (|r: &mut FinalArtifactReceipt| r.artifact_digest = digest('e'))
            as fn(&mut FinalArtifactReceipt),
        |r| r.artifact_digest = "short".to_string(),
        |r| r.readback_id = "artifact-none".to_string(),
        |r| r.readback_id = "artifact-foreign".to_string(),
    ] {
        let mut receipt = delivery_receipt();
        edit(&mut receipt);
        let outcome = deliver(&h, coordinator_plan(Some(declared())), receipt).await;
        texts.push(match outcome {
            Ok(batch) => {
                abstained_with(&batch.records.as_slice()[0], "ARTIFACT_DIGEST_MISMATCH");
                "abstained".to_string()
            }
            Err(text) => {
                assert!(text.contains("ARTIFACT_DIGEST_MISMATCH"), "{text}");
                text
            }
        });
    }
    assert_eq!(
        texts[2].replace("artifact-none", "x"),
        texts[3].replace("artifact-foreign", "x")
    );

    // A plan digest that is not the carried plan's, and a plan that is another
    // plan's, refuse as a plan mismatch; accounting refusals still apply.
    let proposal = coordinator_plan(Some(declared()));
    let request = fanout_request_with(
        &h,
        AggregationOp::Concat,
        None,
        digest('9'),
        &["t1", "t2", "t3", "t4"],
        vec![ok("t1", 'a'), ok("t2", 'b'), ok("t3", 'c'), ok("t4", 'd')],
        None,
        Some(proposal.clone()),
        Some(delivery_receipt()),
    );
    refused(decide(&h, request).await, "FANOUT_PLAN_MISMATCH");
    let digest_of = plan_explicit_tasks(&proposal).unwrap().plan_digest;
    let request = fanout_request_with(
        &h,
        AggregationOp::Concat,
        None,
        digest_of.clone(),
        &["t1", "t2", "t3", "t4"],
        vec![ok("t1", 'a'), ok("t2", 'b'), ok("t3", 'c')],
        None,
        Some(proposal.clone()),
        Some(delivery_receipt()),
    );
    refused(decide(&h, request).await, "FANOUT_CHILD_MISSING");
    let request = fanout_request_with(
        &h,
        AggregationOp::Concat,
        None,
        digest_of,
        &["t1", "t2", "t3", "t4"],
        vec![
            ok("t1", 'a'),
            ok("t1", 'a'),
            ok("t2", 'b'),
            ok("t3", 'c'),
            ok("t4", 'd'),
        ],
        None,
        Some(proposal),
        Some(delivery_receipt()),
    );
    refused(decide(&h, request).await, "FANOUT_CHILD_DUPLICATE");
    // A delivery receipt with no plan has nothing to bind to.
    let request = fanout_request_with(
        &h,
        AggregationOp::Concat,
        None,
        plan(),
        &["t1"],
        vec![ok("t1", 'a')],
        None,
        None,
        Some(delivery_receipt()),
    );
    refused(decide(&h, request).await, "FANOUT_PLAN_MISMATCH");
}
