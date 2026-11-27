//! Skill documents compile to programs without a workflow file (MEDULLA-DECISION-ENGINE-R156.3
//! to R156.5): a typed draft is compiled by `Decide`, the committed record stores the program
//! under `skill:<document digest>`, `ProgramLookup` reads it back, an incomplete draft refuses
//! by name and writes nothing, and the sealed record carries the four premises.

use super::kernel_family_support::kernel_request;
use super::log_tests::log_op;
use super::path_preference_tests::{pins, OTHER};
use super::*;
use medulla_decision_types::decision::program_store::{program_key, CompiledProgram};
use medulla_decision_types::decision::skill_workflow::{
    DraftStep, SkillWorkflowDraft, StepEdge, TypeRef,
};
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::TypedValue;
use medulla_decision_types::decision::{PremiseClass, ResolutionKind};
use medulla_types::contract::Digest256;

fn step(id: &str, action: &str, next: &[&str], on_error: Option<&str>) -> DraftStep {
    DraftStep {
        step_id: id.into(),
        action: action.into(),
        input_types: BoundedVec::new(vec![TypeRef::Text]).unwrap(),
        output_types: BoundedVec::new(vec![TypeRef::Json]).unwrap(),
        next: BoundedVec::new(
            next.iter()
                .map(|target| StepEdge {
                    step_id: (*target).into(),
                    label: None,
                })
                .collect(),
        )
        .unwrap(),
        on_error: on_error.map(Into::into),
    }
}

fn draft_of(document: &[u8], steps: Vec<DraftStep>) -> SkillWorkflowDraft {
    SkillWorkflowDraft {
        skill_document_digest: Digest256::sha256(document),
        steps: BoundedVec::new(steps).unwrap(),
    }
}

/// fetch -> parse -> store, each external step falling back to `recover`.
fn golden(document: &[u8]) -> SkillWorkflowDraft {
    draft_of(
        document,
        vec![
            step("fetch", "tool.fetch", &["parse"], Some("recover")),
            step("parse", "tool.parse", &["store"], Some("recover")),
            step("store", "tool.store", &[], Some("recover")),
            step("recover", "internal.recover", &[], None),
        ],
    )
}

fn skill_request(schema: &ComponentDependency, draft: SkillWorkflowDraft) -> DecideRequest {
    kernel_request(
        schema,
        QuestionKind::SkillWorkflowCompile,
        "skill_workflow",
        TypedValue::SkillWorkflowDraft(Box::new(draft)),
        &["workflow"],
    )
}

fn lookup_request(schema: &ComponentDependency, tenant: &str, signature: &str) -> DecideRequest {
    let mut request = kernel_request(
        schema,
        QuestionKind::ProgramLookup,
        "task_signature",
        TypedValue::Text(signature.to_owned()),
        &["program"],
    );
    request.tenant_id = tenant.to_owned();
    request
}

fn program_of(record: &StatisticalDecisionRecord) -> CompiledProgram {
    match &record.answer {
        Some(FamilyAnswer::SkillWorkflow(program)) => (**program).clone(),
        other => panic!("a compiled skill program, got {other:?}"),
    }
}

fn looked_up(record: &StatisticalDecisionRecord) -> Option<CompiledProgram> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let id = &scores.as_slice()[0].option_id;
            let body = id.strip_prefix("program:").expect("a program answer");
            Some(serde_json::from_str(body).expect("the answer carries the program"))
        }
        _ => None,
    }
}

fn commit(record: StatisticalDecisionRecord) -> DecisionLogOp {
    DecisionLogOp::Commit {
        record: Box::new(record),
        evaluator: None,
        approval: None,
    }
}

fn stored(h: &Harness, signature: &str) -> bool {
    h.store
        .decision_artifact(TENANT, &program_key(signature))
        .unwrap()
        .is_some()
}

// spec: MEDULLA-DECISION-ENGINE-R156.3
#[tokio::test]
async fn served_compile_then_lookup_by_skill_digest() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let schema = pins(&h, TENANT).0;
    let draft = golden(b"deploy skill");
    let signature = format!("skill:{}", draft.skill_document_digest.to_hex());

    // Decide compiles and writes nothing.
    let batch = decide(&h, skill_request(&schema, draft.clone()))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    let program = program_of(&record);
    assert_eq!(program.task_signature, signature);
    let order: Vec<&str> = program
        .steps
        .iter()
        .map(|step| step.node.as_str())
        .collect();
    assert_eq!(order, ["fetch", "parse", "store", "recover"]);
    assert_eq!(program.source_run_id, None);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert!(!stored(&h, &signature), "Decide wrote nothing");
    let before = decide(&h, lookup_request(&schema, TENANT, &signature))
        .await
        .unwrap();
    assert!(
        looked_up(&before.records.as_slice()[0]).is_none(),
        "nothing to look up yet"
    );

    // The commit is the one write; the program is stored under the skill digest.
    let committed: DecisionLogCommitted =
        decode(log_op(&h, "decider", commit(record.clone())).await).unwrap();
    assert!(!committed.replayed);
    assert!(stored(&h, &signature));
    let again: DecisionLogCommitted = decode(log_op(&h, "decider", commit(record)).await).unwrap();
    assert!(
        again.replayed,
        "a repeat commit replays and stores nothing new"
    );

    // ProgramLookup returns the program by the skill digest, field for field.
    let found = decide(&h, lookup_request(&schema, TENANT, &signature))
        .await
        .unwrap();
    assert_eq!(looked_up(&found.records.as_slice()[0]), Some(program));
}

// spec: MEDULLA-DECISION-ENGINE-R156.3
#[tokio::test]
async fn other_tenant_cannot_look_up_the_program() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let schema = pins(&h, TENANT).0;
    let theirs = pins(&h, OTHER).0;
    let draft = golden(b"private skill");
    let signature = format!("skill:{}", draft.skill_document_digest.to_hex());
    let batch = decide(&h, skill_request(&schema, draft)).await.unwrap();
    let record = batch.records.as_slice()[0].clone();
    let _: DecisionLogCommitted = decode(log_op(&h, "decider", commit(record)).await).unwrap();

    // The other tenant's principal looks up the same signature and finds no program.
    let outsider = VerifiedRequestContext::verified_for_test_in_tenant("outsider", OTHER);
    let batch = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &outsider,
            lookup_request(&theirs, OTHER, &signature),
        )
        .await,
    )
    .unwrap();
    assert!(looked_up(&batch.records.as_slice()[0]).is_none());
    assert!(h
        .store
        .decision_artifact(OTHER, &program_key(&signature))
        .unwrap()
        .is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R156.4
#[tokio::test]
async fn incomplete_drafts_refuse() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let schema = pins(&h, TENANT).0;
    let rows = |h: &Harness| {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 100_000)
            .unwrap()
            .len()
    };
    let before = rows(&h);
    let mut unlabelled = step("a", "tool.a", &["b", "c"], Some("recover"));
    unlabelled.next = BoundedVec::new(vec![
        StepEdge {
            step_id: "b".into(),
            label: Some("yes".into()),
        },
        StepEdge {
            step_id: "c".into(),
            label: None,
        },
    ])
    .unwrap();
    let mut untyped = step("a", "tool.a", &[], Some("recover"));
    untyped.input_types = BoundedVec::new(Vec::new()).unwrap();
    let recover = || step("recover", "internal.recover", &[], None);
    let cases: Vec<(&str, &str, SkillWorkflowDraft)> = vec![
        (
            "SKILL_WORKFLOW_AMBIGUOUS",
            "step a",
            draft_of(
                b"d1",
                vec![
                    step("a", "tool.a", &[], Some("recover")),
                    step("a", "tool.b", &[], Some("recover")),
                    recover(),
                ],
            ),
        ),
        (
            "SKILL_WORKFLOW_AMBIGUOUS",
            "absent step ghost",
            draft_of(
                b"d2",
                vec![step("a", "tool.a", &["ghost"], Some("recover")), recover()],
            ),
        ),
        (
            "SKILL_WORKFLOW_AMBIGUOUS",
            "a branch has no label",
            draft_of(
                b"d3",
                vec![
                    unlabelled,
                    step("b", "tool.b", &[], Some("recover")),
                    step("c", "tool.c", &[], Some("recover")),
                    recover(),
                ],
            ),
        ),
        (
            "SKILL_WORKFLOW_MISSING_ERROR_HANDLER",
            "step a",
            draft_of(b"d4", vec![step("a", "tool.a", &[], None), recover()]),
        ),
        (
            "SKILL_WORKFLOW_UNTYPED_STEP",
            "step a",
            draft_of(b"d5", vec![untyped, recover()]),
        ),
        (
            "SKILL_WORKFLOW_CYCLE",
            "step a",
            draft_of(
                b"d6",
                vec![
                    step("a", "tool.a", &["b"], Some("recover")),
                    step("b", "tool.b", &["a"], Some("recover")),
                    recover(),
                ],
            ),
        ),
    ];
    for (code, named, draft) in cases {
        let signature = format!("skill:{}", draft.skill_document_digest.to_hex());
        let refusal = decide(&h, skill_request(&schema, draft)).await.unwrap_err();
        assert!(refusal.contains(code), "{code}: {refusal}");
        assert!(refusal.contains(named), "{code} names {named}: {refusal}");
        assert!(!stored(&h, &signature), "{code} stored a program");
    }
    // A draft past the step limit does not decode at all, so no program can follow.
    let too_long = serde_json::json!({
        "skill_document_digest": Digest256::sha256(b"d7").to_hex(),
        "steps": (0..41).map(|index| serde_json::json!({
            "step_id": format!("s{index}"), "action": "internal.noop",
            "input_types": ["text"], "output_types": ["json"],
        })).collect::<Vec<_>>(),
    });
    assert!(serde_json::from_value::<SkillWorkflowDraft>(too_long).is_err());
    assert_eq!(rows(&h), before, "no refusal wrote anything");
}

// spec: MEDULLA-DECISION-ENGINE-R156.5
#[tokio::test]
async fn golden_skill_drafts_have_fixed_program_digests() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let schema = pins(&h, TENANT).0;
    let first = decide(&h, skill_request(&schema, golden(b"golden skill")))
        .await
        .unwrap();
    let again = decide(&h, skill_request(&schema, golden(b"golden skill")))
        .await
        .unwrap();
    let record = first.records.as_slice()[0].clone();
    let program = program_of(&record);
    assert_eq!(
        program,
        program_of(&again.records.as_slice()[0]),
        "the compile is deterministic"
    );
    assert_eq!(program.program_digest, program.compute_digest());

    // The error branch, the types and the step order survive the compile.
    let fetch = &program.steps.as_slice()[0];
    assert_eq!(fetch.next_node.as_deref(), Some("parse"));
    assert_eq!(fetch.edges.last().unwrap().label.as_deref(), Some("error"));
    assert_eq!(fetch.input_types.to_vec(), [TypeRef::Text]);

    // Four premises: the document and the step count are claims, the program is a proof, the
    // compiler is a definition. The record class is the weakest one.
    let premises = &record.premises;
    let find = |fact: &str| {
        premises
            .iter()
            .find(|premise| premise.fact == fact)
            .unwrap_or_else(|| panic!("premise {fact} missing from {premises:?}"))
    };
    let document = find(&format!(
        "skill_workflow:document={}",
        Digest256::sha256(b"golden skill").to_hex()
    ));
    assert_eq!(document.class, PremiseClass::Claim);
    let compiled = find(&format!(
        "skill_workflow:program={}",
        program.program_digest
    ));
    assert_eq!(compiled.class, PremiseClass::Proof);
    assert_eq!(find("skill_workflow:steps=4").class, PremiseClass::Claim);
    assert_eq!(
        find("skill_workflow:compiler=skill_workflow_compile@1").class,
        PremiseClass::Definition
    );
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
}
