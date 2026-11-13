//! Field shielding served through `Decide` (MEDULLA-DECISION-ENGINE-R162.3,
//! R162.4): the family answers which fields a step may read, as a constraint,
//! and abstains with a registered code naming the path when a read is shielded
//! or undeclared, a required input is missing or a path is too deep.

use super::*;
use medulla_decision_types::decision::field_shielding::{
    DeclaredInput, FieldPath, FieldShieldingRequest, ShieldedField, StepProjection, REFUSALS,
};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::{DecisionPolicy, ResolutionKind};

fn path(value: &str) -> FieldPath {
    FieldPath::new(value).unwrap()
}

fn input(value: &str, required: bool, depth: u8) -> DeclaredInput {
    DeclaredInput {
        path: path(value),
        required,
        depth,
    }
}

/// A customer object whose `ssn` is shielded; the step declares `customer`
/// one level deep and the notes subtree.
fn shielding() -> FieldShieldingRequest {
    FieldShieldingRequest {
        object_paths: vec![
            path("customer.name"),
            path("customer.ssn"),
            path("customer.address.city"),
            path("billing.card"),
        ],
        shielded: vec![ShieldedField {
            path: path("customer.ssn"),
        }],
        step: StepProjection {
            step_id: "step:1".into(),
            declared: vec![input("customer", true, 1)],
        },
        requested_reads: vec![path("customer.name")],
    }
}

fn request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    shielding: FieldShieldingRequest,
) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::FieldShielding,
        "release_projection",
        policy,
        vec![TypedParam {
            name: "field_shielding".to_string(),
            value: TypedValue::FieldShielding(Box::new(shielding)),
        }],
    )
}

fn facts(record: &StatisticalDecisionRecord) -> Vec<&str> {
    record.premises.iter().map(|p| p.fact.as_str()).collect()
}

/// A request for another tenant by a caller of this one is refused.
async fn foreign_tenant_is_refused(h: &Harness, schema: &ComponentDependency) {
    let mut foreign = request(schema, DecisionPolicyRef::Default, shielding());
    foreign.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R162.3: the served answer carries the
/// allowed and withheld sets as a constraint.
// spec: MEDULLA-DECISION-ENGINE-R162.3
#[tokio::test]
async fn shielding_routes() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let batch = decide(
        &h,
        request(&schema, DecisionPolicyRef::Default, shielding()),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::FieldShielding);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!(
            "a releasable projection answers advisory, got {:?}",
            record.outcome
        );
    };
    assert_eq!(scores.as_slice().len(), 1);
    assert_eq!(scores.as_slice()[0].option_id, "release_projection");

    // customer.name sits one level below the declared `customer`; ssn is
    // shielded even though declared; address.city is two levels down; billing
    // is undeclared.
    let facts = facts(record);
    assert!(facts.contains(&"shielding:step=step:1"));
    assert!(facts.contains(&"shielding:allowed=customer.name"));
    for withheld in ["customer.ssn", "customer.address.city", "billing.card"] {
        assert!(
            facts.contains(&format!("shielding:withheld={withheld}").as_str()),
            "{withheld} is withheld: {facts:?}"
        );
    }
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.starts_with("shielding:allowed="))
            .count(),
        1
    );

    foreign_tenant_is_refused(&h, &schema).await;
}

fn code_of(batch: &DecisionBatch) -> (String, String) {
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome);
    };
    let reason = &reasons.as_slice()[0];
    (reason.code().as_str().to_string(), format!("{reason:?}"))
}

/// Verifies MEDULLA-DECISION-ENGINE-R162.4: each refusal code is returned with
/// the path it names, and nothing is stored.
// spec: MEDULLA-DECISION-ENGINE-R162.4
#[tokio::test]
async fn undeclared_fields_refuse() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    decide(
        &h,
        request(&schema, DecisionPolicyRef::Default, shielding()),
    )
    .await
    .unwrap();
    let dir = std::path::PathBuf::from(h.state.read().await.persist_dir.clone().unwrap());
    let before = stored_files(&dir);

    let refused = |shielding: FieldShieldingRequest, policy: DecisionPolicyRef| {
        let request = request(&schema, policy, shielding);
        let h = &h;
        async move { code_of(&decide(h, request).await.unwrap()) }
    };

    // A declared sibling the step never asked for is fine; an undeclared read refuses.
    let mut undeclared = shielding();
    undeclared.requested_reads = vec![path("billing.card")];
    let (code, detail) = refused(undeclared, DecisionPolicyRef::Default).await;
    assert_eq!(code, "FIELD_UNDECLARED");
    assert!(detail.contains("billing.card"), "{detail}");

    // Too deep below its declared input: withheld, not shielded, so undeclared.
    let mut deep = shielding();
    deep.requested_reads = vec![path("customer.address.city")];
    assert_eq!(
        refused(deep, DecisionPolicyRef::Default).await.0,
        "FIELD_UNDECLARED"
    );

    // A shielded read, and a read below a shielded path.
    let mut shielded = shielding();
    shielded.requested_reads = vec![path("customer.ssn")];
    let (code, detail) = refused(shielded, DecisionPolicyRef::Default).await;
    assert_eq!(code, "FIELD_SHIELDED");
    assert!(detail.contains("customer.ssn"), "{detail}");
    let mut below = shielding();
    below.object_paths.push(path("customer.ssn.last4"));
    below.requested_reads = vec![path("customer.ssn.last4")];
    assert_eq!(
        refused(below, DecisionPolicyRef::Default).await.0,
        "FIELD_SHIELDED"
    );

    // A required declared input the object does not hold.
    let mut missing = shielding();
    missing.step.declared.push(input("order", true, 2));
    let (code, detail) = refused(missing, DecisionPolicyRef::Default).await;
    assert_eq!(code, "FIELD_REQUIRED_MISSING");
    assert!(detail.contains("order"), "{detail}");
    let mut optional = shielding();
    optional.step.declared.push(input("order", false, 2));
    assert!(matches!(
        decide(&h, request(&schema, DecisionPolicyRef::Default, optional))
            .await
            .unwrap()
            .records
            .as_slice()[0]
            .outcome,
        StatisticalOutcome::Advisory { .. }
    ));

    // A path deeper than the pinned shielding.max_depth.
    let mut policy = DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KnobName::new("shielding.max_depth"), KnobValue::Integer(2));
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("shield-depth", &policy),
    };
    let (code, detail) = refused(shielding(), pinned).await;
    assert_eq!(code, "FIELD_PATH_INVALID");
    assert!(detail.contains("customer.address.city"), "{detail}");

    // A refusal wrote nothing, and every code is registered.
    assert_eq!(stored_files(&dir), before);
    for code in REFUSALS {
        assert_eq!(
            medulla_decision_types::decision::codes::resolve(code.as_str()),
            Some(*code)
        );
    }
    let mut no_param = request(&schema, DecisionPolicyRef::Default, shielding());
    no_param.params = BoundedVec::default();
    let error = decide(&h, no_param).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    foreign_tenant_is_refused(&h, &schema).await;
}

/// Every file under the server's persist directory with its length.
fn stored_files(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                out.push((entry.path().display().to_string(), meta.len()));
            }
        }
    }
    out.sort();
    out
}
