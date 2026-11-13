use super::*;

fn task(id: &str, deps: &[&str]) -> TaskPlanTask {
    TaskPlanTask {
        task_id: id.into(),
        prompt: format!("Analyze {id} with cited evidence"),
        depends_on: BoundedVec::new(deps.iter().map(|s| s.to_string()).collect()).unwrap(),
        call: None,
        aggregation: None,
        assignee: None,
        branch: None,
        join: None,
        decision: None,
        approval: None,
        duration_ms: None,
        priority_weight: None,
    }
}

fn proposal() -> TaskPlanProposal {
    TaskPlanProposal {
        tasks: BoundedVec::new(vec![
            task("join", &["c", "a"]),
            task("c", &["b"]),
            task("b", &[]),
            task("a", &[]),
        ])
        .unwrap(),
        output_task_id: "join".into(),
        max_concurrency: 2,
        max_steps: 4,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R134
// spec: MEDULLA-DECISION-ENGINE-R134.111.1
#[test]
fn task_plan_round_trips() {
    let plan = plan_explicit_tasks(&proposal()).unwrap();
    let value = serde_json::to_value(&plan).unwrap();
    assert_eq!(
        serde_json::from_value::<TaskPlan>(value.clone()).unwrap(),
        plan
    );
    let mut unknown = value;
    unknown["authorize"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TaskPlan>(unknown).is_err());
    let mut value = serde_json::to_value(proposal()).unwrap();
    value["tasks"][0]["model"] = serde_json::json!("untrusted");
    assert!(serde_json::from_value::<TaskPlanProposal>(value).is_err());
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.2
#[test]
fn plan_respects_dependencies() {
    let original = proposal();
    let plan = plan_explicit_tasks(&original).unwrap();
    assert_eq!(
        plan.tasks
            .iter()
            .map(|t| t.task_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "join"]
    );
    assert_eq!(plan.tasks.as_slice()[2].depends_on.as_slice(), ["b"]);
    assert_eq!(plan.tasks.as_slice()[3].depends_on.as_slice(), ["a", "c"]);
    assert_eq!(
        (plan.total_tasks, plan.max_steps, plan.max_concurrency),
        (4, 4, 2)
    );
    let mut reordered = original.clone();
    reordered.tasks = BoundedVec::new(original.tasks.iter().cloned().rev().collect()).unwrap();
    let other = plan_explicit_tasks(&reordered).unwrap();
    assert_eq!(plan.plan_digest, other.plan_digest);
    assert_ne!(plan.input_digest, other.input_digest);
    let mut changed = original.clone();
    let mut tasks = changed.tasks.as_slice().to_vec();
    tasks[0].prompt.push('!');
    changed.tasks = BoundedVec::new(tasks).unwrap();
    assert_ne!(
        plan.plan_digest,
        plan_explicit_tasks(&changed).unwrap().plan_digest
    );
}

fn refuses(mutator: impl FnOnce(&mut TaskPlanProposal), reason: &str) {
    let mut input = proposal();
    mutator(&mut input);
    assert!(plan_explicit_tasks(&input).unwrap_err().starts_with(reason));
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.4
#[test]
fn invalid_graphs_refuse() {
    refuses(|p| p.tasks = BoundedVec::default(), "EMPTY_PLAN");
    refuses(
        |p| p.tasks = BoundedVec::new(vec![task("a", &[]), task("a", &[])]).unwrap(),
        "DUPLICATE_TASK_ID",
    );
    refuses(
        |p| p.tasks = BoundedVec::new(vec![task("bad id", &[])]).unwrap(),
        "INVALID_TASK_ID",
    );
    refuses(
        |p| p.tasks = BoundedVec::new(vec![task("a", &["missing"])]).unwrap(),
        "MISSING_DEPENDENCY",
    );
    refuses(
        |p| p.tasks = BoundedVec::new(vec![task("a", &["a"])]).unwrap(),
        "PLAN_CYCLE",
    );
    refuses(
        |p| p.tasks = BoundedVec::new(vec![task("a", &["b"]), task("b", &["a"])]).unwrap(),
        "PLAN_CYCLE",
    );
    refuses(
        |p| p.tasks = BoundedVec::new(vec![task("a", &["b", "b"]), task("b", &[])]).unwrap(),
        "DUPLICATE_DEPENDENCY",
    );
    refuses(
        |p| p.output_task_id = "missing".into(),
        "INVALID_OUTPUT_TASK",
    );
    refuses(|p| p.output_task_id = "c".into(), "DISCONNECTED_OUTPUT");
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.4
#[test]
fn work_and_text_bounds_refuse() {
    for value in [0, 3, 65] {
        refuses(|p| p.max_steps = value, "PLAN_BOUNDS");
    }
    for value in [0, 5, 9] {
        refuses(|p| p.max_concurrency = value, "PLAN_BOUNDS");
    }
    for prompt in [" ".into(), "x".repeat(MAX_PROMPT_BYTES + 1)] {
        refuses(
            |p| {
                p.tasks = BoundedVec::new(vec![TaskPlanTask {
                    prompt,
                    ..task("join", &[])
                }])
                .unwrap()
            },
            "PLAN_TEXT_BOUNDS",
        );
    }
    let mut input = proposal();
    input.tasks = BoundedVec::new(
        (0..5)
            .map(|i| TaskPlanTask {
                prompt: "x".repeat(MAX_PROMPT_BYTES),
                ..task(&format!("t{i}"), &[])
            })
            .collect(),
    )
    .unwrap();
    input.max_steps = 5;
    assert!(plan_explicit_tasks(&input)
        .unwrap_err()
        .starts_with("PLAN_TEXT_BOUNDS"));
    let input = TaskPlanProposal {
        tasks: BoundedVec::new(
            (0..64)
                .map(|i| task(&format!("t{i:02}"), if i == 0 { &[] } else { &["t00"] }))
                .collect(),
        )
        .unwrap(),
        output_task_id: "t63".into(),
        max_concurrency: 8,
        max_steps: 64,
    };
    // All tasks are bounded, but most do not feed the output sink.
    assert!(plan_explicit_tasks(&input)
        .unwrap_err()
        .starts_with("DISCONNECTED_OUTPUT"));
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.2
#[test]
fn maximum_bounded_chain_is_valid() {
    let tasks = (0..64)
        .map(|i| TaskPlanTask {
            task_id: format!("t{i:02}"),
            prompt: "Bounded work".to_string(),
            depends_on: BoundedVec::new(if i == 0 {
                vec![]
            } else {
                vec![format!("t{:02}", i - 1)]
            })
            .unwrap(),
            call: None,
            aggregation: None,
            assignee: None,
            branch: None,
            join: None,
            decision: None,
            approval: None,
            duration_ms: None,
            priority_weight: None,
        })
        .collect();
    let input = TaskPlanProposal {
        tasks: BoundedVec::new(tasks).unwrap(),
        output_task_id: "t63".into(),
        max_concurrency: 8,
        max_steps: 64,
    };
    let plan = plan_explicit_tasks(&input).unwrap();
    assert_eq!(plan.total_tasks, 64);
    assert_eq!(plan.tasks.as_slice()[63].task_id, "t63");
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.4
#[test]
fn deserialization_enforces_collection_bounds() {
    let mut value = serde_json::to_value(proposal()).unwrap();
    value["tasks"] = serde_json::json!((0..65)
        .map(|i| task(&format!("t{i}"), &[]))
        .collect::<Vec<_>>());
    assert!(serde_json::from_value::<TaskPlanProposal>(value).is_err());
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.4
#[test]
fn index_refusal_precedence_is_stable() {
    let mut input = proposal();
    input.max_steps = 0;
    input.tasks = BoundedVec::default();
    assert_eq!(
        index(&input).unwrap_err(),
        "EMPTY_PLAN: at least one task is required"
    );
    input.tasks = BoundedVec::new(vec![task("bad id", &[])]).unwrap();
    assert_eq!(
        index(&input).unwrap_err(),
        "PLAN_BOUNDS: invalid task or concurrency ceiling"
    );
    input.max_steps = 4;
    let mut invalid = task("bad id", &["missing", "missing"]);
    invalid.prompt = " ".into();
    input.tasks = BoundedVec::new(vec![invalid.clone()]).unwrap();
    assert_eq!(index(&input).unwrap_err(), "INVALID_TASK_ID: bad id");
    invalid.task_id = "a".into();
    input.tasks = BoundedVec::new(vec![invalid.clone()]).unwrap();
    assert_eq!(index(&input).unwrap_err(), "PLAN_TEXT_BOUNDS: a");
    invalid.prompt = "Valid prompt".into();
    input.tasks = BoundedVec::new(vec![task("a", &[]), invalid]).unwrap();
    assert_eq!(index(&input).unwrap_err(), "DUPLICATE_DEPENDENCY: a");
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.4
#[test]
fn index_accepts_exact_total_prompt_byte_limit() {
    let mut input = proposal();
    let mut tasks = input.tasks.as_slice().to_vec();
    for task in &mut tasks {
        task.prompt = "é".repeat(MAX_PROMPT_BYTES / 2);
    }
    input.tasks = BoundedVec::new(tasks).unwrap();
    assert!(index(&input).is_ok());
    let mut tasks = input.tasks.as_slice().to_vec();
    tasks.push(task("extra", &[]));
    input.tasks = BoundedVec::new(tasks).unwrap();
    input.max_steps = 5;
    assert_eq!(index(&input).unwrap_err(), "PLAN_TEXT_BOUNDS: extra");
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.7
#[test]
fn the_canonical_task_plan_feature_schema_is_valid_and_stable() {
    let body = task_plan_feature_schema().checked().unwrap();
    assert_eq!(
        body.names(),
        [
            "task_position",
            "task_call",
            "task_call_bindings",
            "task_step_kind"
        ]
    );
    let digest = crate::decision::statistical::body::encode_body(&body)
        .unwrap()
        .content_digest;
    assert_eq!(
        digest,
        crate::decision::statistical::body::encode_body(&task_plan_feature_schema())
            .unwrap()
            .content_digest
    );
}

fn tool(id: &str, deps: &[&str], arguments: serde_json::Value) -> TaskPlanTask {
    TaskPlanTask {
        call: Some(TaskPlanCall {
            server: "github".into(),
            tool: "get_issue".into(),
            arguments,
        }),
        ..task(id, deps)
    }
}

fn mixed(arguments: serde_json::Value) -> TaskPlanProposal {
    TaskPlanProposal {
        tasks: BoundedVec::new(vec![task("a", &[]), tool("b", &["a"], arguments)]).unwrap(),
        output_task_id: "b".into(),
        max_concurrency: 1,
        max_steps: 2,
    }
}

fn reference(text: &str) -> serde_json::Value {
    serde_json::json!({ "$ref": text })
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.8
#[test]
fn tool_step_round_trips_with_literals_and_bindings() {
    let arguments = serde_json::json!({
        "repo": "medulla",
        "limit": 3,
        "issue": reference("steps.a.output.items.0.number"),
        "labels": ["bug", reference("steps.a.output.label")],
    });
    let proposal = mixed(arguments.clone());
    let wire = serde_json::to_value(&proposal).unwrap();
    assert!(
        wire["tasks"][0].get("call").is_none(),
        "prompt step keeps its form"
    );
    assert_eq!(
        serde_json::from_value::<TaskPlanProposal>(wire).unwrap(),
        proposal
    );
    let plan = plan_explicit_tasks(&proposal).unwrap();
    let call = plan.tasks.as_slice()[1].call.as_ref().unwrap();
    assert_eq!(call.arguments, arguments);
    assert_eq!(
        serde_json::from_value::<TaskPlan>(serde_json::to_value(&plan).unwrap()).unwrap(),
        plan
    );
    assert!(plan.tasks.as_slice()[0].call.is_none());
    let bindings = task_bindings(&plan.tasks.as_slice()[1]);
    assert_eq!(
        bindings
            .iter()
            .map(|b| (b.pointer.as_str(), b.source_task.as_str()))
            .collect::<Vec<_>>(),
        [("/issue", "a"), ("/labels/1", "a")]
    );
    let mut unknown = serde_json::to_value(mixed(arguments)).unwrap();
    unknown["tasks"][1]["call"]["credential"] = serde_json::json!("secret");
    assert!(serde_json::from_value::<TaskPlanProposal>(unknown).is_err());
    let extra = serde_json::json!({ "x": { "$ref": "steps.a.output", "default": 1 } });
    assert!(plan_explicit_tasks(&mixed(extra))
        .unwrap_err()
        .starts_with("BINDING_BAD_PATH"));
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.9
#[test]
fn binding_canonicalization_is_order_independent() {
    let first: serde_json::Value = serde_json::from_str(
        r#"{"b": 1, "a": {"y": [1, 2], "x": {"$ref": " steps.a.output.items.007 "}}}"#,
    )
    .unwrap();
    let second: serde_json::Value = serde_json::from_str(
        r#"{ "a" : { "x" : { "$ref" : "steps.a.output.items.7" }, "y" : [1,2] }, "b" : 1 }"#,
    )
    .unwrap();
    let one = plan_explicit_tasks(&mixed(first)).unwrap();
    let two = plan_explicit_tasks(&mixed(second)).unwrap();
    assert_eq!(one.plan_digest, two.plan_digest);
    assert_eq!(
        one.tasks.as_slice()[1].call.as_ref().unwrap().arguments["a"]["x"],
        reference("steps.a.output.items.7")
    );
    // A binding never adds an edge: the referenced task is still only `a`.
    assert_eq!(one.tasks.as_slice()[1].depends_on.as_slice(), ["a"]);
}

fn bound_arguments(bindings: usize, padding: usize) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for i in 0..bindings {
        map.insert(format!("k{i:02}"), reference("steps.a.output"));
    }
    map.insert("pad".into(), serde_json::Value::String("x".repeat(padding)));
    serde_json::Value::Object(map)
}

fn argument_bytes(arguments: &serde_json::Value) -> usize {
    serde_json::to_vec(arguments).unwrap().len()
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.9
#[test]
fn binding_bounds() {
    assert!(plan_explicit_tasks(&mixed(bound_arguments(MAX_BINDINGS_PER_CALL, 0))).is_ok());
    assert!(
        plan_explicit_tasks(&mixed(bound_arguments(MAX_BINDINGS_PER_CALL + 1, 0)))
            .unwrap_err()
            .starts_with("CALL_BOUNDS")
    );
    let base = argument_bytes(&bound_arguments(0, 0));
    let exact = bound_arguments(0, MAX_CALL_ARGUMENT_BYTES - base);
    assert_eq!(argument_bytes(&exact), MAX_CALL_ARGUMENT_BYTES);
    assert!(plan_explicit_tasks(&mixed(exact)).is_ok());
    let over = bound_arguments(0, MAX_CALL_ARGUMENT_BYTES - base + 1);
    assert!(plan_explicit_tasks(&mixed(over))
        .unwrap_err()
        .starts_with("CALL_BOUNDS"));
    let path = |n: usize| {
        let segments = vec!["k"; n].join(".");
        serde_json::json!({ "x": reference(&format!("steps.a.output.{segments}")) })
    };
    assert!(plan_explicit_tasks(&mixed(path(MAX_BINDING_PATH_SEGMENTS))).is_ok());
    assert!(
        plan_explicit_tasks(&mixed(path(MAX_BINDING_PATH_SEGMENTS + 1)))
            .unwrap_err()
            .starts_with("BINDING_BAD_PATH")
    );
    let long = |len: usize| {
        let key = "k".repeat(len - "steps.a.output.".len());
        serde_json::json!({ "x": reference(&format!("steps.a.output.{key}")) })
    };
    assert!(plan_explicit_tasks(&mixed(long(MAX_BINDING_PATH_BYTES))).is_ok());
    assert!(
        plan_explicit_tasks(&mixed(long(MAX_BINDING_PATH_BYTES + 1)))
            .unwrap_err()
            .starts_with("BINDING_BAD_PATH")
    );
    let tools = TaskPlanProposal {
        tasks: BoundedVec::new(
            (0..MAX_TOOL_STEPS)
                .map(|i| {
                    let deps: Vec<String> = (i > 0)
                        .then(|| format!("t{:02}", i - 1))
                        .into_iter()
                        .collect();
                    let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
                    tool(&format!("t{i:02}"), &deps, serde_json::json!({}))
                })
                .collect(),
        )
        .unwrap(),
        output_task_id: "t63".into(),
        max_concurrency: 8,
        max_steps: 64,
    };
    assert_eq!(plan_explicit_tasks(&tools).unwrap().total_tasks, 64);
    let mut over_total = mixed(bound_arguments(0, MAX_CALL_ARGUMENT_BYTES - base));
    let mut tasks = over_total.tasks.as_slice().to_vec();
    for task in &mut tasks {
        task.prompt = "x".repeat(MAX_PROMPT_BYTES);
    }
    for i in 0..2 {
        tasks.push(TaskPlanTask {
            prompt: "x".repeat(MAX_PROMPT_BYTES),
            ..task(&format!("p{i}"), &[])
        });
    }
    tasks[1].depends_on = BoundedVec::new(vec!["a".into(), "p0".into(), "p1".into()]).unwrap();
    over_total.tasks = BoundedVec::new(tasks).unwrap();
    over_total.max_steps = 4;
    // Four full prompts fit; the call text pushes the plan past 64 KiB.
    assert!(plan_explicit_tasks(&over_total)
        .unwrap_err()
        .starts_with("PLAN_TEXT_BOUNDS"));
}

fn binding_refusal(proposal: TaskPlanProposal) -> String {
    plan_explicit_tasks(&proposal).unwrap_err()
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.10
#[test]
fn refuses_each_binding_defect() {
    let cases = [
        (
            serde_json::json!({ "x": reference("steps.zz.output") }),
            "BINDING_UNKNOWN_TASK: task=b pointer=/x",
        ),
        (
            serde_json::json!({ "n": { "x": reference("steps.a.output.a b") } }),
            "BINDING_BAD_PATH: task=b pointer=/n/x",
        ),
        (
            serde_json::json!({ "x": [reference("steps.b.output")] }),
            "BINDING_CYCLE: task=b pointer=/x/0",
        ),
    ];
    for (arguments, expected) in cases {
        assert_eq!(binding_refusal(mixed(arguments)), expected);
    }
    let mut not_dependency = mixed(serde_json::json!({ "x": reference("steps.c.output") }));
    let mut tasks = not_dependency.tasks.as_slice().to_vec();
    tasks.push(task("c", &[]));
    tasks[1].depends_on = BoundedVec::new(vec!["a".into()]).unwrap();
    tasks[0].depends_on = BoundedVec::new(vec!["c".into()]).unwrap();
    not_dependency.tasks = BoundedVec::new(tasks).unwrap();
    not_dependency.max_steps = 3;
    assert_eq!(
        binding_refusal(not_dependency),
        "BINDING_NOT_DEPENDENCY: task=b pointer=/x"
    );
    // A reference to a downstream task closes a cycle through `depends_on`.
    let mut downstream = mixed(serde_json::json!({}));
    let mut tasks = downstream.tasks.as_slice().to_vec();
    tasks[0] = tool(
        "a",
        &[],
        serde_json::json!({ "y": reference("steps.b.output") }),
    );
    downstream.tasks = BoundedVec::new(tasks).unwrap();
    assert_eq!(
        binding_refusal(downstream),
        "BINDING_CYCLE: task=a pointer=/y"
    );
    for call in [
        TaskPlanCall {
            server: String::new(),
            tool: "t".into(),
            arguments: serde_json::json!({}),
        },
        TaskPlanCall {
            server: "s".into(),
            tool: "bad tool".into(),
            arguments: serde_json::json!({}),
        },
        TaskPlanCall {
            server: "s".into(),
            tool: "t".into(),
            arguments: serde_json::json!([1]),
        },
        TaskPlanCall {
            server: "s".into(),
            tool: "t".into(),
            arguments: reference("steps.a.output"),
        },
    ] {
        let mut input = mixed(serde_json::json!({}));
        let mut tasks = input.tasks.as_slice().to_vec();
        tasks[1].call = Some(call);
        input.tasks = BoundedVec::new(tasks).unwrap();
        assert_eq!(binding_refusal(input), "CALL_INVALID: task=b pointer=");
    }
    assert_eq!(
        binding_refusal(mixed(bound_arguments(MAX_BINDINGS_PER_CALL + 1, 0))),
        "CALL_BOUNDS: task=b pointer=/k00"
    );
    // A plain dependency cycle still refuses as a plan cycle.
    let mut cyclic = mixed(serde_json::json!({ "x": reference("steps.a.output") }));
    let mut tasks = cyclic.tasks.as_slice().to_vec();
    tasks[0].depends_on = BoundedVec::new(vec!["b".into()]).unwrap();
    cyclic.tasks = BoundedVec::new(tasks).unwrap();
    assert!(binding_refusal(cyclic).starts_with("PLAN_CYCLE"));
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.11
#[test]
fn digest_covers_calls_and_bindings() {
    let arguments = serde_json::json!({ "repo": "medulla", "x": reference("steps.a.output.id") });
    let base = plan_explicit_tasks(&mixed(arguments.clone())).unwrap();
    let digest = |edit: &dyn Fn(&mut TaskPlanCall)| {
        let mut input = mixed(arguments.clone());
        let mut tasks = input.tasks.as_slice().to_vec();
        edit(tasks[1].call.as_mut().unwrap());
        input.tasks = BoundedVec::new(tasks).unwrap();
        plan_explicit_tasks(&input).unwrap().plan_digest
    };
    assert_eq!(digest(&|_| {}), base.plan_digest);
    assert_ne!(digest(&|c| c.tool = "list_issues".into()), base.plan_digest);
    assert_ne!(digest(&|c| c.server = "gitlab".into()), base.plan_digest);
    assert_ne!(
        digest(&|c| c.arguments["repo"] = serde_json::json!("other")),
        base.plan_digest
    );
    assert_ne!(
        digest(&|c| c.arguments["x"] = reference("steps.a.output.name")),
        base.plan_digest
    );
    let prompt_only = plan_explicit_tasks(&proposal()).unwrap();
    assert_ne!(prompt_only.policy_digest, "");
    assert_eq!(base.policy_digest, task_plan_policy_digest());
}

fn aggregated(op: AggregationOp, reducer_id: Option<&str>) -> TaskPlanProposal {
    let mut input = proposal();
    let mut tasks = input.tasks.as_slice().to_vec();
    tasks[0].aggregation = Some(TaskPlanAggregation {
        op,
        reducer_id: reducer_id.map(str::to_string),
    });
    tasks[1].assignee = Some("skill:summarize".into());
    input.tasks = BoundedVec::new(tasks).unwrap();
    input
}

// spec: MEDULLA-DECISION-ENGINE-R134.1
#[test]
fn aggregation_round_trips() {
    let plan = plan_explicit_tasks(&aggregated(AggregationOp::MajorityVote, None)).unwrap();
    let value = serde_json::to_value(&plan).unwrap();
    let back = serde_json::from_value::<TaskPlan>(value.clone()).unwrap();
    assert_eq!(back, plan);
    let join = back.tasks.iter().find(|t| t.task_id == "join").unwrap();
    assert_eq!(
        join.aggregation,
        Some(TaskPlanAggregation {
            op: AggregationOp::MajorityVote,
            reducer_id: None
        })
    );
    let c = back.tasks.iter().find(|t| t.task_id == "c").unwrap();
    assert_eq!(c.assignee.as_deref(), Some("skill:summarize"));
    for (op, wire) in [
        (AggregationOp::Concat, "concat"),
        (AggregationOp::MajorityVote, "majority_vote"),
        (AggregationOp::Reduce, "reduce"),
        (AggregationOp::FirstValid, "first_valid"),
    ] {
        assert_eq!(serde_json::to_value(op).unwrap(), serde_json::json!(wire));
    }
    assert!(
        serde_json::from_value::<TaskPlanAggregation>(serde_json::json!({"op": "average"}))
            .is_err()
    );
    // Plans without the fields keep their serialized form.
    let plain = serde_json::to_value(plan_explicit_tasks(&proposal()).unwrap()).unwrap();
    assert!(plain["tasks"][0].get("aggregation").is_none());
    assert!(plain["tasks"][0].get("assignee").is_none());
    assert_eq!(plan.policy_revision, "bounded-explicit-dag-v2");
}

// spec: MEDULLA-DECISION-ENGINE-R134.1
#[test]
fn aggregation_off_the_output_task_refuses() {
    let mut off = proposal();
    let mut tasks = off.tasks.as_slice().to_vec();
    tasks[1].aggregation = Some(TaskPlanAggregation {
        op: AggregationOp::Concat,
        reducer_id: None,
    });
    off.tasks = BoundedVec::new(tasks).unwrap();
    assert!(plan_explicit_tasks(&off)
        .unwrap_err()
        .starts_with(PLAN_AGGREGATION_NOT_OUTPUT));
    for (op, reducer) in [
        (AggregationOp::Reduce, None),
        (AggregationOp::Concat, Some("reducer:sum")),
        (AggregationOp::Reduce, Some("bad id")),
    ] {
        assert!(plan_explicit_tasks(&aggregated(op, reducer))
            .unwrap_err()
            .starts_with(PLAN_AGGREGATION_INVALID));
    }
    let base = plan_explicit_tasks(&proposal()).unwrap();
    let reduce =
        plan_explicit_tasks(&aggregated(AggregationOp::Reduce, Some("reducer:sum"))).unwrap();
    let concat = plan_explicit_tasks(&aggregated(AggregationOp::Concat, None)).unwrap();
    assert_ne!(base.plan_digest, reduce.plan_digest);
    assert_ne!(reduce.plan_digest, concat.plan_digest);
}

fn step(id: &str, deps: &[&str], extra: serde_json::Value) -> serde_json::Value {
    let mut value = serde_json::json!({
        "task_id": id,
        "prompt": format!("Do {id}"),
        "depends_on": deps,
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

/// A plan with all four control kinds: a parallel fork and join, a decision, an
/// exclusive branch with a default arm, and an approval wait on that arm.
fn control_json() -> serde_json::Value {
    let none = serde_json::json!({});
    serde_json::json!({
        "tasks": [
            step("start", &[], none.clone()),
            step("fork", &["start"], serde_json::json!({"branch": {
                "kind": "parallel",
                "arms": [{"target_task_id": "x"}, {"target_task_id": "y"}]
            }})),
            step("x", &["fork"], none.clone()),
            step("y", &["fork"], none.clone()),
            step("merge", &["x", "y"], serde_json::json!({"join": {"branch_task_id": "fork"}})),
            step("decide", &["merge"], serde_json::json!({"decision": {
                "decision_iri": "urn:decision:route",
                "inputs": ["steps.merge.output.score"]
            }})),
            step("route", &["decide"], serde_json::json!({"branch": {
                "kind": "exclusive",
                "arms": [{"target_task_id": "ok", "condition":
                    {"ref": "steps.decide.output.verdict", "op": "eq", "value": "approve"}}],
                "default_arm": "review"
            }})),
            step("ok", &["route"], none.clone()),
            step("review", &["route"], serde_json::json!({"approval": {
                "approver_role": "reviewer", "timeout_s": 3600
            }})),
            step("final", &["ok", "review"], none),
        ],
        "output_task_id": "final",
        "max_concurrency": 4,
        "max_steps": 12
    })
}

fn control_proposal(edit: impl FnOnce(&mut serde_json::Value)) -> TaskPlanProposal {
    let mut value = control_json();
    edit(&mut value);
    serde_json::from_value(value).unwrap()
}

fn task_mut<'a>(value: &'a mut serde_json::Value, id: &str) -> &'a mut serde_json::Value {
    value["tasks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["task_id"] == id)
        .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.14
#[test]
fn control_steps_round_trip_and_refuse_two_kinds() {
    let plan = plan_explicit_tasks(&control_proposal(|_| {})).unwrap();
    let value = serde_json::to_value(&plan).unwrap();
    assert_eq!(
        serde_json::from_value::<TaskPlan>(value.clone()).unwrap(),
        plan
    );
    let by_id = |id: &str| plan.tasks.iter().find(|t| t.task_id == id).unwrap();
    assert_eq!(
        by_id("fork").branch.as_ref().unwrap().kind,
        BranchKind::Parallel
    );
    assert_eq!(by_id("merge").join.as_ref().unwrap().branch_task_id, "fork");
    assert_eq!(
        by_id("decide").decision.as_ref().unwrap().decision_iri,
        "urn:decision:route"
    );
    assert_eq!(by_id("review").approval.as_ref().unwrap().timeout_s, 3600);
    assert!(matches!(
        by_id("route").branch.as_ref().unwrap().arms.as_slice()[0].condition,
        Some(BranchCondition::Compare(_))
    ));
    // An unset step is an absent key, never null.
    assert!(value["tasks"][0].get("branch").is_none());

    // A decision-IRI condition is the other condition shape.
    let decided = control_proposal(|v| {
        task_mut(v, "route")["branch"]["arms"][0]["condition"] =
            serde_json::json!({"decision_iri": "urn:decision:approved"});
    });
    assert!(plan_explicit_tasks(&decided).is_ok());

    // Unknown fields are rejected on every control type.
    for (id, key, field) in [
        ("fork", "branch", "extra"),
        ("merge", "join", "extra"),
        ("decide", "decision", "extra"),
        ("review", "approval", "extra"),
    ] {
        let mut v = control_json();
        task_mut(&mut v, id)[key][field] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<TaskPlanProposal>(v).is_err(),
            "{key}"
        );
    }

    // Two kinds on one task refuse STEP_KIND_CONFLICT and name it.
    for (id, extra) in [
        (
            "review",
            serde_json::json!({"join": {"branch_task_id": "fork"}}),
        ),
        (
            "x",
            serde_json::json!({"call": {"server": "s", "tool": "t", "arguments": {}},
                "approval": {"approver_role": "r", "timeout_s": 60}}),
        ),
    ] {
        let err = plan_explicit_tasks(&control_proposal(|v| {
            task_mut(v, id)
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
        }))
        .unwrap_err();
        assert_eq!(err, format!("{STEP_KIND_CONFLICT}: task={id}"));
    }
}

fn refused(edit: impl FnOnce(&mut serde_json::Value), code: &str, task: &str) {
    let err = plan_explicit_tasks(&control_proposal(edit)).unwrap_err();
    assert_eq!(err, format!("{code}: task={task}"));
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.15
#[test]
fn control_checks_refuse_planted_faults() {
    // Arm target missing, or not depending on its branch.
    refused(
        |v| task_mut(v, "fork")["branch"]["arms"][0]["target_task_id"] = "nope".into(),
        BRANCH_ARM_UNKNOWN,
        "fork",
    );
    refused(
        |v| task_mut(v, "x")["depends_on"] = serde_json::json!(["start"]),
        BRANCH_ARM_UNKNOWN,
        "fork",
    );
    refused(
        |v| task_mut(v, "route")["branch"]["default_arm"] = "ok".into(),
        BRANCH_ARM_UNKNOWN,
        "route",
    );
    // An exclusive branch needs a default arm or an exhaustive mark.
    let mut exhaustive = control_json();
    {
        let branch = task_mut(&mut exhaustive, "route")["branch"]
            .as_object_mut()
            .unwrap();
        branch.remove("default_arm");
        branch.insert("exhaustive".into(), true.into());
        task_mut(&mut exhaustive, "route")["branch"]["arms"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"target_task_id": "review"}));
    }
    assert!(plan_explicit_tasks(&serde_json::from_value(exhaustive).unwrap()).is_ok());
    refused(
        |v| {
            let branch = task_mut(v, "route")["branch"].as_object_mut().unwrap();
            branch.remove("default_arm");
            branch["arms"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"target_task_id": "review"}));
        },
        BRANCH_NO_DEFAULT,
        "route",
    );
    // A parallel branch needs exactly one join; a join must wait on every arm.
    refused(
        |v| {
            task_mut(v, "merge").as_object_mut().unwrap().remove("join");
        },
        BRANCH_JOIN_UNMATCHED,
        "fork",
    );
    refused(
        |v| task_mut(v, "merge")["join"]["branch_task_id"] = "start".into(),
        BRANCH_JOIN_UNMATCHED,
        "fork",
    );
    refused(
        |v| {
            task_mut(v, "merge")["depends_on"] = serde_json::json!(["x"]);
            task_mut(v, "final")["depends_on"] = serde_json::json!(["ok", "review", "y"]);
        },
        BRANCH_JOIN_UNMATCHED,
        "merge",
    );
    // The approval timeout lies in [60, 2,592,000] seconds.
    for bad in [0u64, 59, 2_592_001] {
        refused(
            |v| task_mut(v, "review")["approval"]["timeout_s"] = bad.into(),
            APPROVAL_TIMEOUT_OUT_OF_RANGE,
            "review",
        );
    }
    for good in [60u64, 2_592_000] {
        let p = control_proposal(|v| task_mut(v, "review")["approval"]["timeout_s"] = good.into());
        assert!(plan_explicit_tasks(&p).is_ok());
    }
    // A condition reads only a direct dependency; `in` takes an array.
    refused(
        |v| {
            task_mut(v, "route")["branch"]["arms"][0]["condition"]["ref"] =
                "steps.start.output".into()
        },
        "BINDING_NOT_DEPENDENCY",
        "route",
    );
    refused(
        |v| task_mut(v, "route")["branch"]["arms"][0]["condition"]["op"] = "in".into(),
        CONTROL_STEP_INVALID,
        "route",
    );
    // The plan stays acyclic.
    let err = plan_explicit_tasks(&control_proposal(|v| {
        task_mut(v, "start")["depends_on"] = serde_json::json!(["final"]);
    }))
    .unwrap_err();
    assert!(err.starts_with("PLAN_CYCLE"), "{err}");
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.17
#[test]
fn control_fields_change_the_digest() {
    let base = plan_explicit_tasks(&control_proposal(|_| {})).unwrap();
    assert_eq!(
        base.plan_digest,
        plan_plan_digest_again(),
        "the digest is stable"
    );
    let edits: [(&str, fn(&mut serde_json::Value)); 6] = [
        ("timeout", |v| {
            task_mut(v, "review")["approval"]["timeout_s"] = 3601.into()
        }),
        ("role", |v| {
            task_mut(v, "review")["approval"]["approver_role"] = "owner".into()
        }),
        ("iri", |v| {
            task_mut(v, "decide")["decision"]["decision_iri"] = "urn:decision:other".into()
        }),
        ("default", |v| {
            let branch = task_mut(v, "route")["branch"].as_object_mut().unwrap();
            branch.insert("exhaustive".into(), true.into());
        }),
        ("input", |v| {
            task_mut(v, "decide")["decision"]["inputs"]
                .as_array_mut()
                .unwrap()
                .push("steps.merge.output.other".into())
        }),
        ("value", |v| {
            task_mut(v, "route")["branch"]["arms"][0]["condition"]["value"] = "reject".into()
        }),
    ];
    let mut digests = BTreeSet::from([base.plan_digest.clone()]);
    for (name, edit) in edits {
        let plan = plan_explicit_tasks(&control_proposal(edit)).unwrap();
        assert!(
            digests.insert(plan.plan_digest),
            "{name} left the digest unchanged"
        );
    }
    // Reference text is canonical: leading zeros in an index do not matter.
    let a = control_proposal(|v| {
        task_mut(v, "route")["branch"]["arms"][0]["condition"]["ref"] =
            "steps.decide.output.items.07".into()
    });
    let b = control_proposal(|v| {
        task_mut(v, "route")["branch"]["arms"][0]["condition"]["ref"] =
            "steps.decide.output.items.7".into()
    });
    assert_eq!(
        plan_explicit_tasks(&a).unwrap().plan_digest,
        plan_explicit_tasks(&b).unwrap().plan_digest
    );
    // The canonical schema describes the step kind.
    assert!(task_plan_feature_schema()
        .names()
        .iter()
        .any(|n| n == "task_step_kind"));
}

fn plan_plan_digest_again() -> String {
    plan_explicit_tasks(&control_proposal(|_| {}))
        .unwrap()
        .plan_digest
}

fn specialists(scores: &[(&str, i64)]) -> BoundedVec<SpecialistScore, 64> {
    BoundedVec::new(
        scores
            .iter()
            .map(|(id, micros)| SpecialistScore {
                specialist_id: id.to_string(),
                // Q32 of micros / 1e6.
                score_q32: ((i128::from(*micros) << 32) / 1_000_000) as i64,
            })
            .collect(),
    )
    .unwrap()
}

fn subtask(
    id: &str,
    deps: &[&str],
    schema: Option<&str>,
    scores: &[(&str, i64)],
) -> DecomposeSubtask {
    DecomposeSubtask {
        id: id.into(),
        text: format!("Handle {id} with cited evidence"),
        depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
        answer_schema: schema.map(str::to_string),
        specialists: specialists(scores),
    }
}

fn facts(output: &str, subtasks: Vec<DecomposeSubtask>) -> DecomposeFacts {
    DecomposeFacts {
        output_task_id: output.into(),
        subtasks: BoundedVec::new(subtasks).unwrap(),
    }
}

fn knobs() -> DecomposeKnobs {
    DecomposeKnobs::from_policy(&crate::decision::policy::DecisionPolicy::engine_default())
}

// spec: MEDULLA-DECISION-ENGINE-R134.3
#[test]
fn independent_subtasks_yield_no_edges_and_operator() {
    let voting = facts(
        "out",
        vec![
            subtask("a", &[], Some("verdict"), &[("spec-x", 900_000)]),
            subtask("b", &[], Some("verdict"), &[("spec-x", 800_000)]),
            subtask("out", &["a", "b"], None, &[("spec-y", 500_000)]),
        ],
    );
    let proposal = decompose_task(&voting, &knobs()).unwrap();
    let plan = plan_explicit_tasks(&proposal).unwrap();
    let by_id = |id: &str| plan.tasks.iter().find(|t| t.task_id == id).unwrap();
    // No edge between the two independent subtasks, and no independence flag.
    assert!(by_id("a").depends_on.is_empty() && by_id("b").depends_on.is_empty());
    assert_eq!(by_id("out").depends_on.as_slice(), ["a", "b"]);
    assert_eq!(independent_count(&proposal), 2);
    assert_eq!(
        by_id("out").aggregation,
        Some(TaskPlanAggregation {
            op: AggregationOp::MajorityVote,
            reducer_id: None
        })
    );
    assert_eq!((plan.max_concurrency, plan.max_steps), (2, 3));
    // Differing or missing schemas fall back to concat.
    for schemas in [(Some("verdict"), Some("score")), (Some("verdict"), None)] {
        let mixed = facts(
            "out",
            vec![
                subtask("a", &[], schemas.0, &[("spec-x", 900_000)]),
                subtask("b", &[], schemas.1, &[("spec-x", 900_000)]),
                subtask("out", &["a", "b"], None, &[("spec-y", 500_000)]),
            ],
        );
        let plan = plan_explicit_tasks(&decompose_task(&mixed, &knobs()).unwrap()).unwrap();
        assert_eq!(
            plan.tasks
                .iter()
                .find(|t| t.task_id == "out")
                .unwrap()
                .aggregation
                .as_ref()
                .map(|a| a.op),
            Some(AggregationOp::Concat)
        );
    }
}

// spec: MEDULLA-DECISION-ENGINE-R134.3
#[test]
fn independence_is_the_empty_dependency_closure() {
    let chain = facts(
        "out",
        vec![
            subtask("a", &[], None, &[("s", 900_000)]),
            subtask("b", &["a"], None, &[("s", 900_000)]),
            subtask("c", &["b"], None, &[("s", 900_000)]),
            subtask("out", &["c"], None, &[("s", 900_000)]),
        ],
    );
    let proposal = decompose_task(&chain, &knobs()).unwrap();
    assert_eq!(independent_count(&proposal), 1);
    assert_eq!(proposal.max_concurrency, 1);
    assert_eq!(proposal.max_steps, 4);
    let wide: Vec<DecomposeSubtask> = (0..12)
        .map(|n| subtask(&format!("w{n:02}"), &[], None, &[("s", 900_000)]))
        .collect();
    let deps: Vec<String> = (0..12).map(|n| format!("w{n:02}")).collect();
    let mut all = wide;
    all.push(subtask(
        "out",
        &deps.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
        &[("s", 900_000)],
    ));
    let proposal = decompose_task(&facts("out", all), &knobs()).unwrap();
    assert_eq!(independent_count(&proposal), 12);
    // The concurrency ceiling caps the count; the topology question gets the count.
    assert_eq!(proposal.max_concurrency, MAX_CONCURRENCY);
    let requirements = topology_requirements(
        medulla_agent_types::agent_fixtures::topology::requirements(1),
        &proposal,
    );
    assert_eq!(requirements.subtasks, 12);
}

// spec: MEDULLA-DECISION-ENGINE-R134.3
#[test]
fn specialists_route_by_score_ties_and_the_floor_knob() {
    let f = facts(
        "out",
        vec![
            subtask(
                "a",
                &[],
                None,
                &[("zed", 700_000), ("alpha", 700_000), ("low", 100_000)],
            ),
            subtask("out", &["a"], None, &[("only", 300_000)]),
        ],
    );
    let plan = plan_explicit_tasks(&decompose_task(&f, &knobs()).unwrap()).unwrap();
    let assignee = |id: &str| {
        plan.tasks
            .iter()
            .find(|t| t.task_id == id)
            .unwrap()
            .assignee
            .clone()
    };
    // The tie goes to the lowest id, and a score below the floor never wins.
    assert_eq!(assignee("a").as_deref(), Some("alpha"));
    assert_eq!(assignee("out").as_deref(), Some("only"));
    // A pinned knob raises the floor: `only` (0.3) no longer qualifies.
    let mut policy = crate::decision::policy::DecisionPolicy::engine_default();
    policy.knobs.insert(
        crate::decision::knob_families::task_plan::SPECIALIST_MIN_SCORE.knob_name(),
        crate::decision::knobs::KnobValue::DecimalMicros(500_000),
    );
    assert_eq!(
        decompose_task(&f, &DecomposeKnobs::from_policy(&policy)).unwrap_err(),
        "NO_SPECIALIST_FOR_SUBTASK: out"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R134.3
#[test]
fn a_subtask_without_a_specialist_names_the_first_such_id() {
    let f = facts(
        "out",
        vec![
            subtask("m", &[], None, &[]),
            subtask("b", &[], None, &[("x", 50_000)]),
            subtask("out", &["b", "m"], None, &[("x", 900_000)]),
        ],
    );
    assert_eq!(
        decompose_task(&f, &knobs()).unwrap_err(),
        "NO_SPECIALIST_FOR_SUBTASK: b"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R134.3
#[test]
fn a_decomposition_cycle_is_left_to_the_shipped_cycle_check() {
    let f = facts(
        "b",
        vec![
            subtask("a", &["b"], None, &[("x", 900_000)]),
            subtask("b", &["a"], None, &[("x", 900_000)]),
        ],
    );
    let proposal = decompose_task(&f, &knobs()).unwrap();
    assert!(plan_explicit_tasks(&proposal)
        .unwrap_err()
        .starts_with("PLAN_CYCLE"));
}

// spec: MEDULLA-DECISION-ENGINE-R134.6
#[test]
fn every_planner_code_is_registered_upper_snake() {
    use crate::decision::codes::resolve;
    for code in super::TASK_PLAN_REFUSALS {
        assert_eq!(resolve(code.as_str()), Some(*code));
        assert!(code
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'_'));
    }
    for raised in [
        "EMPTY_PLAN",
        "PLAN_CYCLE",
        "DISCONNECTED_OUTPUT",
        "BINDING_NOT_DEPENDENCY",
        "PLAN_PARAMETER_CONFLICT",
    ] {
        assert!(resolve(raised).is_some(), "{raised} is registered");
    }
}
