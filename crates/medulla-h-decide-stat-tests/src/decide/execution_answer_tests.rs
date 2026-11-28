//! Served acceptance of `Method::AgentExecutionAnswer`
//! (MEDULLA-CQ-ORCH-R008.2): the seven-part answer over a fixture graph that
//! holds one task, and the tenant refusal that runs before any read.

use medulla_decision_outcome_types::decision::execution_answer::{
    AgentExecutionAnswerRequest, ExecutionAnswer, ExecutionAnswerPart,
};
use medulla_decision_types::decision::record::DecisionOutcome;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::ontology_iri;

use super::stat_tests::assemble_tests::{
    assemble, harness, prepare_request_graph, request, seed, Harness, ASKER, TENANT,
};

const CORE: &str = ontology_iri!("core#");
const SOURCE_PREDICATE: &str = "http://purl.org/dc/terms/source";
const TASK: &str = "urn:task:xyz";

/// Add `(subject, predicate IRI, object)` edges between IRI-keyed nodes.
async fn state_edges(h: &Harness, edges: &[(&str, String, &str)]) {
    let state = h.state.read().await;
    let core = &state.registry.get("default").expect("request graph").core;
    let empty = rmp_serde::to_vec_named(&serde_json::json!({})).unwrap();
    for (subject, predicate, object) in edges {
        for node in [subject, object] {
            core.add_node(format!("<{node}>"), empty.clone());
        }
        core.add_edge(
            format!("<{subject}>"),
            format!("<{object}>"),
            rmp_serde::to_vec_named(&serde_json::json!({ "relationship": predicate })).unwrap(),
        )
        .expect("add the edge");
    }
}

fn core(predicate: &str) -> String {
    format!("{CORE}{predicate}")
}

async fn ask(h: &Harness, request: AgentExecutionAnswerRequest) -> Result<ExecutionAnswer, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, TENANT);
    let response = medulla_h_decide::decide::execution_answer::handle_agent_execution_answer(
        &h.state,
        1,
        ("default", &verified),
        request,
    )
    .await;
    if let Some(error) = response.error {
        return Err(error);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => Ok(rmp_serde::from_slice(&bytes).unwrap()),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn graph_version(h: &Harness) -> u64 {
    h.state
        .read()
        .await
        .registry
        .get("default")
        .expect("request graph")
        .core
        .version()
}

// spec: MEDULLA-CQ-ORCH-R008.2
#[tokio::test]
async fn served_answer_fills_seven_parts() {
    let h = harness().await;
    seed(&h.store, None);
    prepare_request_graph(&h).await;
    state_edges(
        &h,
        &[
            (TASK, core("hasStep"), "urn:step:gather"),
            (TASK, core("hasStep"), "urn:step:report"),
            ("urn:step:gather", core("precedes"), "urn:step:report"),
            ("urn:step:gather", core("handsOffTo"), "urn:step:report"),
            (
                "urn:step:gather",
                SOURCE_PREDICATE.to_string(),
                "urn:source:spec",
            ),
        ],
    )
    .await;
    let before = graph_version(&h).await;

    let assembly = request(Vec::new());
    let answer = ask(
        &h,
        AgentExecutionAnswerRequest {
            task_iri: TASK.to_string(),
            assembly: assembly.clone(),
        },
    )
    .await
    .expect("answers");

    assert_eq!(answer.task_iri, TASK);
    assert_eq!(answer.assembled_by, "graph.assemble");
    let parts = &answer.parts;
    for (name, part) in [
        ("steps", &parts.steps),
        ("staffing", &parts.staffing),
        ("guardrails", &parts.guardrails),
        ("bindings", &parts.bindings),
        ("workflow", &parts.workflow),
        ("dag", &parts.dag),
    ] {
        assert!(
            matches!(part, ExecutionAnswerPart::Answer { .. }),
            "{name} is answered: {part:?}"
        );
    }
    // The steps part names the source of the step that states one.
    let ExecutionAnswerPart::Answer {
        value, provenance, ..
    } = &parts.steps
    else {
        unreachable!()
    };
    assert_eq!(value["vars"], serde_json::json!(["step", "next", "source"]));
    assert_eq!(provenance.len(), 1);
    assert_eq!(provenance.as_slice()[0].fact, "dc:source <urn:source:spec>");

    // The certificate digest is the digest of the sealed record AgentAssemble returns.
    let solved = assemble(&h, assembly).await.expect("assembles");
    assert!(matches!(
        solved.record.outcome,
        DecisionOutcome::Solved { .. }
    ));
    assert_eq!(answer.certificate_digest, solved.record.record_digest);
    assert!(!answer.certificate_digest.is_empty());

    // Nothing was written.
    assert_eq!(graph_version(&h).await, before);
}

// spec: MEDULLA-CQ-ORCH-R008.2
#[tokio::test]
async fn other_tenant_assembly_is_access_denied() {
    let h = harness().await;
    seed(&h.store, None);
    prepare_request_graph(&h).await;
    let before = graph_version(&h).await;
    let mut assembly = request(Vec::new());
    assembly.tenant_id = "tenant-other".to_string();
    let error = ask(
        &h,
        AgentExecutionAnswerRequest {
            task_iri: TASK.to_string(),
            assembly,
        },
    )
    .await
    .expect_err("refuses");
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
    assert_eq!(graph_version(&h).await, before);

    // A task IRI outside the admitted shape refuses before any read.
    let error = ask(
        &h,
        AgentExecutionAnswerRequest {
            task_iri: "urn:task:<x>".to_string(),
            assembly: request(Vec::new()),
        },
    )
    .await
    .expect_err("refuses");
    assert!(error.starts_with("INVALID_ARGUMENT"), "{error}");
}

/// The `Family` abstention an abstained part carries: its code, and the premise the part cites.
fn family_abstention(part: &ExecutionAnswerPart, name: &str) -> String {
    use medulla_decision_types::decision::record::AbstainReason;
    use medulla_types::evidence_class::EvidenceClass;
    let ExecutionAnswerPart::Abstain {
        reason: AbstainReason::Family { code, .. },
        provenance,
        evidence_class,
    } = part
    else {
        panic!("{name} abstains with a family reason: {part:?}");
    };
    assert_eq!(*evidence_class, EvidenceClass::Claim, "{name}");
    assert_eq!(provenance.len(), 1, "{name}");
    assert_eq!(
        provenance.as_slice()[0].fact,
        format!("orch:part={name}/{}", code.as_str())
    );
    code.as_str().to_string()
}

// spec: MEDULLA-CQ-ORCH-R008.3
/// Verifies MEDULLA-CQ-ORCH-R008.3: a part whose own read failed abstains with the
/// `ORCH_PART_NO_SOURCE` family code, the failure text as its detail, evidence class `Claim`
/// and the premise `orch:part=<name>/<code>`.
#[test]
fn no_source_fault_abstains_one_part_by_code() {
    use medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind;
    use medulla_decision_types::decision::record::AbstainReason;
    use {
        medulla_h_decide::decide::execution_answer::part_or_abstain,
        medulla_h_decide::decide::execution_answer::PartFault,
    };

    let part = part_or_abstain(
        "coverage",
        "urn:task:xyz",
        Err(PartFault::NoSource("read failed".into())),
    );
    let ExecutionAnswerPart::Abstain {
        reason:
            AbstainReason::Family {
                family,
                code,
                detail,
            },
        ..
    } = &part
    else {
        panic!("a part with no source abstains: {part:?}");
    };
    assert_eq!(family, &QuestionKind::TaskPlan);
    assert_eq!(code.as_str(), "ORCH_PART_NO_SOURCE");
    assert_eq!(detail, "read failed");
    assert_eq!(family_abstention(&part, "coverage"), "ORCH_PART_NO_SOURCE");
}

// spec: MEDULLA-CQ-ORCH-R008.3
#[tokio::test]
async fn task_without_rows_abstains_steps_and_workflow_by_part() {
    let h = harness().await;
    seed(&h.store, None);
    prepare_request_graph(&h).await;
    let answer = ask(
        &h,
        AgentExecutionAnswerRequest {
            task_iri: "urn:task:nobody".to_string(),
            assembly: request(Vec::new()),
        },
    )
    .await
    .expect("a part without data never fails the response");
    assert_eq!(
        family_abstention(&answer.parts.steps, "steps"),
        "ORCH_PART_EMPTY"
    );
    assert_eq!(
        family_abstention(&answer.parts.workflow, "workflow"),
        "ORCH_PART_EMPTY"
    );
    // The parts with data still answer.
    for (name, part) in [
        ("staffing", &answer.parts.staffing),
        ("bindings", &answer.parts.bindings),
        ("dag", &answer.parts.dag),
    ] {
        assert!(
            matches!(part, ExecutionAnswerPart::Answer { .. }),
            "{name} is answered: {part:?}"
        );
    }
}

// spec: MEDULLA-CQ-ORCH-R008.3
#[tokio::test]
async fn abstained_assembly_abstains_its_three_parts_and_keeps_the_others() {
    let h = harness().await;
    // No library is seeded, so the assembly cannot staff the task.
    prepare_request_graph(&h).await;
    let answer = ask(
        &h,
        AgentExecutionAnswerRequest {
            task_iri: TASK.to_string(),
            assembly: request(Vec::new()),
        },
    )
    .await
    .expect("an abstained assembly never fails the response");
    for (name, part) in [
        ("staffing", &answer.parts.staffing),
        ("bindings", &answer.parts.bindings),
        ("dag", &answer.parts.dag),
    ] {
        assert_eq!(
            family_abstention(part, name),
            "ORCH_PART_ASSEMBLY_ABSTAINED"
        );
    }
    assert!(answer.certificate_digest.is_empty());
}
