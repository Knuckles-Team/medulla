//! `DecisionEval` `CertifyLocalSkill` (MEDULLA-DECISION-ENGINE-R142.101), served: an atomic skill
//! is attested and stored by digest, a workflow is gated on a current attestation of every atomic
//! dependency, and a refusal stores nothing.

use super::*;
use medulla_agent_types::agent_component::{AgentComponentKind, ComponentDependency};
use medulla_decision_types::decision::local_certification::{
    CriterionResult, LocalCertificationRequest, LocalSkillPins, RunReceipt, SkillClassification,
    LOCAL_CERTIFICATION_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::ranking_eval::PinnedSet;

const FOREIGN: &str = "tenant-foreign";
const PREFIX: &str = "local_certification/";

fn pin(id: &str, kind: AgentComponentKind, digest: &str) -> ComponentDependency {
    ComponentDependency {
        component_id: id.into(),
        kind,
        definition_digest: digest.into(),
    }
}

fn pins(model_digest: &str) -> LocalSkillPins {
    LocalSkillPins {
        model: pin("model-a", AgentComponentKind::ModelProfile, model_digest),
        dataset: PinnedSet {
            id: "dataset-a".into(),
            revision: 1,
            digest: "c".repeat(64),
        },
        criteria_policy: pin("criteria-a", AgentComponentKind::DecisionPolicy, "sha256:cc"),
    }
}

fn request(
    tenant: &str,
    key: &str,
    skill: &ComponentDependency,
    dependencies: Vec<ComponentDependency>,
    pins: LocalSkillPins,
) -> LocalCertificationRequest {
    LocalCertificationRequest {
        schema_version: LOCAL_CERTIFICATION_SCHEMA_VERSION,
        tenant_id: tenant.into(),
        idempotency_key: key.into(),
        skill: skill.clone(),
        classification: if dependencies.is_empty() {
            SkillClassification::Atomic
        } else {
            SkillClassification::Workflow
        },
        dependencies,
        pins: pins.clone(),
        criterion_ids: vec!["accuracy".into()],
        receipt: RunReceipt {
            run_id: format!("run-{key}"),
            skill: skill.clone(),
            pins,
            results: vec![CriterionResult {
                criterion_id: "accuracy".into(),
                passed: true,
                evidence_digest: "e".repeat(64),
            }],
            completion_revision: 1,
        },
    }
}

async fn certify(h: &Harness, who: &str, request: LocalCertificationRequest) -> DecisionJobRecord {
    let tenant = request.tenant_id.clone();
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, &tenant);
    let op = DecisionEvalOp::CertifyLocalSkill {
        request: Box::new(request),
    };
    decode(super::super::jobs::handle_decision_eval(&h.state, 71, &verified, op).await).unwrap()
}

fn stored_count(h: &Harness, tenant: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(tenant, PREFIX, 100)
        .unwrap()
        .len()
}

fn code_of(job: &DecisionJobRecord) -> &str {
    match &job.outcome {
        DecisionJobOutcome::Failed { code } => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R142.101
#[tokio::test]
async fn attests_atomic_and_gates_workflow() {
    let h = Harness::new().await;
    let atomic = h
        .publish_test_component("skill-atomic", AgentComponentKind::Skill, "atomic skill", None::<&String>, None)
        .unwrap();
    let other = h
        .publish_test_component("skill-other", AgentComponentKind::Skill, "unattested skill", None::<&String>, None)
        .unwrap();
    let workflow_skill = pin("skill-workflow", AgentComponentKind::Skill, "sha256:dd");

    // A workflow before its dependency is attested refuses CERT_DEPENDENCY_MISSING.
    let early = certify(
        &h,
        "decider",
        request(TENANT, "wf-0", &workflow_skill, vec![atomic.clone()], pins("sha256:aa")),
    )
    .await;
    assert!(code_of(&early).starts_with("CERT_DEPENDENCY_MISSING"), "{}", code_of(&early));
    assert_eq!(stored_count(&h, TENANT), 0, "a refusal stores nothing");

    // The atomic skill is attested, listing its evidence, and stored by digest.
    let job = certify(&h, "decider", request(TENANT, "atomic-1", &atomic, vec![], pins("sha256:aa"))).await;
    let DecisionJobOutput::Certification { attestation } = succeeded(&job).clone() else {
        panic!("a certification output");
    };
    assert!(attestation.premises.iter().any(|p| p == "criterion:accuracy"));
    assert!(attestation.premises.iter().any(|p| p.starts_with("skill:skill-atomic@")));
    assert_eq!(stored_count(&h, TENANT), 1);
    // The same request again replays the stored job and writes nothing more.
    let again = certify(&h, "decider", request(TENANT, "atomic-1", &atomic, vec![], pins("sha256:aa"))).await;
    assert_eq!(again.job_id, job.job_id);
    assert_eq!(stored_count(&h, TENANT), 1);

    // The workflow now passes; another model pin makes the attestation stale; a dependency with
    // no attestation is still missing.
    let ok = certify(
        &h,
        "decider",
        request(TENANT, "wf-1", &workflow_skill, vec![atomic.clone()], pins("sha256:aa")),
    )
    .await;
    succeeded(&ok);
    let stale = certify(
        &h,
        "decider",
        request(TENANT, "wf-2", &workflow_skill, vec![atomic.clone()], pins("sha256:bb")),
    )
    .await;
    assert!(code_of(&stale).starts_with("CERT_DEPENDENCY_STALE"), "{}", code_of(&stale));
    let missing = certify(
        &h,
        "decider",
        request(TENANT, "wf-3", &workflow_skill, vec![other], pins("sha256:aa")),
    )
    .await;
    assert!(code_of(&missing).starts_with("CERT_DEPENDENCY_MISSING"));
    assert_eq!(stored_count(&h, TENANT), 2, "the atomic and the passing workflow only");

    // A receipt that fails a criterion is refused by the pure rule before any store check.
    let mut failing = request(TENANT, "atomic-2", &atomic, vec![], pins("sha256:aa"));
    failing.receipt.results[0].passed = false;
    let refused = certify(&h, "decider", failing).await;
    assert!(code_of(&refused).starts_with("CERT_CRITERION_FAILED"));

    // Another tenant does not see this tenant's attestation.
    let foreign = certify(
        &h,
        "decider",
        request(FOREIGN, "wf-f", &workflow_skill, vec![atomic.clone()], pins("sha256:aa")),
    )
    .await;
    assert!(code_of(&foreign).starts_with("CERT_DEPENDENCY_MISSING"));
    assert_eq!(stored_count(&h, FOREIGN), 0);
}
