//! The `planning_admission` family through the actual Decide path
//! (MEDULLA-DECISION-ENGINE-R171.3, R171.4): a plan is admitted only when the
//! architecture view derived from the caller's visible repository graph wires
//! every feature, another tenant's repository reads as unknown, and each
//! refusal carries its registered code and admits nothing.

use medulla_core::architecture_view::{derive_architecture_view_of_snapshot, ViewRules};
use medulla_decision_types::decision::planning_admission::{
    AttachPoint, PlanFeature, PlanningAdmissionRequest, ADMISSION_FEATURE_UNWIRED,
    ADMISSION_GAP_UNCOVERED, ADMISSION_NO_COMPARISON, ADMISSION_STALE_VIEW,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::{AbstainReason, PremiseClass, ResolutionKind};
use medulla_types::contract::Digest256;

use super::super::family::planning_admission::view_digest;
use super::*;

const GRAPH: &str = "code-graph";
const REPO: &str = "repo:orders";
const FOREIGN_REPO: &str = "repo:foreign";

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: serde_json::Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

/// A harness whose code graph holds this tenant's repository (one Rust
/// package with two source files) and another tenant's repository.
async fn harness_with_repositories() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
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
        for (tenant, repo, dir) in [
            (TENANT, REPO, "services/orders"),
            ("tenant-other", FOREIGN_REPO, "services/secret"),
        ] {
            put(
                &core,
                &format!("manifest:{repo}"),
                serde_json::json!({
                    "type": "FILE", "file": format!("{dir}/Cargo.toml"),
                    "repository_id": repo, "tenant_id": tenant,
                }),
            );
            for file in ["main.rs", "lib.rs"] {
                put(
                    &core,
                    &format!("symbol:{repo}:{file}"),
                    serde_json::json!({
                        "type": "SYMBOL", "file": format!("{dir}/src/{file}"),
                        "repository_id": repo, "tenant_id": tenant,
                    }),
                );
            }
        }
    }
    h
}

/// The digest of the view the repository derives now, computed without the
/// family so the test is an independent oracle.
async fn current_digest(h: &Harness, repo: &str, tenant: &str) -> Digest256 {
    let guard = h.state.read().await;
    let view = guard.registry.get(GRAPH).unwrap().core.analysis_snapshot();
    let scope = medulla_catalog_types::architecture_view::ViewScope {
        repository_id: repo.to_string(),
        revision: None,
    };
    let own = |row: &serde_json::Map<String, serde_json::Value>| {
        row.get("tenant_id").and_then(serde_json::Value::as_str) == Some(tenant)
    };
    let derived =
        derive_architecture_view_of_snapshot(&view, &scope, &ViewRules::default(), &own).unwrap();
    view_digest(&derived).unwrap()
}

fn gap(name: &str) -> Digest256 {
    Digest256::sha256(name.as_bytes())
}

fn feature(id: &str, container: &str, file: &str, gaps: &[&str]) -> PlanFeature {
    PlanFeature {
        feature_id: id.into(),
        description_digest: gap(id),
        container: container.into(),
        attach_point: AttachPoint {
            file: file.into(),
            symbol: "run".into(),
            operation: "call".into(),
        },
        gap_digests: BoundedVec::new(gaps.iter().map(|g| gap(g)).collect()).unwrap(),
    }
}

fn plan(
    repo: &str,
    view: Digest256,
    gaps: &[&str],
    features: Vec<PlanFeature>,
) -> PlanningAdmissionRequest {
    PlanningAdmissionRequest {
        repository_id: repo.into(),
        repository_revision: "rev-1".into(),
        architecture_view_digest: view,
        gap_proposal_digests: BoundedVec::new(gaps.iter().map(|g| gap(g)).collect()).unwrap(),
        features: BoundedVec::new(features).unwrap(),
    }
}

fn admission_request(pin: &ComponentDependency, plan: PlanningAdmissionRequest) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "plan.admission".into(),
            kind: QuestionKind::PlanningAdmission,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "admit_plan".into(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "planning_admission".into(),
                value: TypedValue::PlanningAdmission(Box::new(plan)),
            },
            TypedParam {
                name: "graph".into(),
                value: TypedValue::Iri(GRAPH.into()),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn artifact_count(h: &Harness) -> usize {
    h.store
        .decision_artifacts_with_prefix(TENANT, "", 100_000)
        .unwrap()
        .len()
}

fn held(record: &StatisticalDecisionRecord) -> (String, String) {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome);
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family {
            family,
            code,
            detail,
        } => {
            assert_eq!(*family, QuestionKind::PlanningAdmission);
            (code.as_str().to_string(), detail.clone())
        }
        other => panic!("expected a family abstention, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R171.3
#[tokio::test]
async fn served_decide_admits_only_wired_features() {
    let h = harness_with_repositories().await;
    let (pin, _) = publish_route_schema(&h);
    let view = current_digest(&h, REPO, TENANT).await;
    let stored = artifact_count(&h);

    // Every feature wired and the gap covered: admitted as a constraint on
    // observed evidence, with the answer and the view premise.
    let wired = plan(
        REPO,
        view,
        &["gap-a"],
        vec![feature(
            "f1",
            "orders",
            "services/orders/src/main.rs",
            &["gap-a"],
        )],
    );
    let batch = decide(&h, admission_request(&pin, wired.clone()))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an admission, got {:?}", record.outcome);
    };
    assert_eq!(scores.as_slice()[0].option_id, "admit_plan");
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    let Some(FamilyAnswer::PlanningAdmission(answer)) = &record.answer else {
        panic!("the admitted record carries the answer");
    };
    assert!(answer.differences.is_empty());
    assert_eq!(answer.architecture_view_digest, view);
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("admission:view ") && p.class == PremiseClass::Observation));

    // A feature whose attach file is not a support path of its container, and
    // one whose container the view does not have, are not wired: the plan is
    // held, each feature named, and no answer is sealed.
    let unwired = plan(
        REPO,
        view,
        &["gap-a", "gap-b"],
        vec![
            feature(
                "f-file",
                "orders",
                "services/orders/src/ghost.rs",
                &["gap-a"],
            ),
            feature("f-box", "ghost", "src/lib.rs", &["gap-b"]),
        ],
    );
    let batch = decide(&h, admission_request(&pin, unwired)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let (code, detail) = held(record);
    assert_eq!(code, ADMISSION_FEATURE_UNWIRED.as_str());
    assert!(
        detail.contains("f-file") && detail.contains("f-box"),
        "{detail}"
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    assert!(record.answer.is_none(), "a held plan carries no answer");

    assert_eq!(artifact_count(&h), stored, "Decide writes nothing");
}

// spec: MEDULLA-DECISION-ENGINE-R171.3
#[tokio::test]
async fn other_tenant_repository_is_invisible() {
    let h = harness_with_repositories().await;
    let (pin, _) = publish_route_schema(&h);
    // Another tenant's repository reads exactly like one that does not exist:
    // the same refusal, never a "forbidden".
    let foreign_view = current_digest(&h, FOREIGN_REPO, "tenant-other").await;
    let mut errors = Vec::new();
    for repo in [FOREIGN_REPO, "repo:does-not-exist"] {
        let request = admission_request(
            &pin,
            plan(
                repo,
                foreign_view,
                &["gap-a"],
                vec![feature(
                    "f1",
                    "secret",
                    "services/secret/src/main.rs",
                    &["gap-a"],
                )],
            ),
        );
        errors.push(decide(&h, request).await.unwrap_err());
    }
    assert!(
        errors[0].starts_with("VIEW_UNKNOWN_REPOSITORY"),
        "{}",
        errors[0]
    );
    assert_eq!(errors[0], errors[1], "no existence leak");
}

// spec: MEDULLA-DECISION-ENGINE-R171.3
#[tokio::test]
async fn task_plan_is_returned_only_when_admitted() {
    let h = harness_with_repositories().await;
    let (pin, _) = publish_route_schema(&h);
    let view = current_digest(&h, REPO, TENANT).await;
    let with_plan = |plan: PlanningAdmissionRequest| {
        let mut request = super::registry_tests::task_request(&pin);
        let mut params = request.params.as_slice().to_vec();
        params.push(TypedParam {
            name: "planning_admission".into(),
            value: TypedValue::PlanningAdmission(Box::new(plan)),
        });
        params.push(TypedParam {
            name: "graph".into(),
            value: TypedValue::Iri(GRAPH.into()),
        });
        request.params = BoundedVec::new(params).unwrap();
        request
    };
    let wired = plan(
        REPO,
        view,
        &["gap-a"],
        vec![feature(
            "f1",
            "orders",
            "services/orders/src/lib.rs",
            &["gap-a"],
        )],
    );
    let batch = decide(&h, with_plan(wired)).await.unwrap();
    assert!(
        batch.records.as_slice()[0].answer.is_some(),
        "the plan is returned"
    );

    let unwired = plan(
        REPO,
        view,
        &["gap-a"],
        vec![feature(
            "f1",
            "orders",
            "services/orders/src/ghost.rs",
            &["gap-a"],
        )],
    );
    let error = decide(&h, with_plan(unwired)).await.unwrap_err();
    assert!(
        error.starts_with(ADMISSION_FEATURE_UNWIRED.as_str()),
        "{error}"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R171.4
#[tokio::test]
async fn admission_refusals() {
    let h = harness_with_repositories().await;
    let (pin, _) = publish_route_schema(&h);
    let view = current_digest(&h, REPO, TENANT).await;
    let stored = artifact_count(&h);
    let wired = |gaps: &[&str], covered: &[&str]| {
        plan(
            REPO,
            view,
            gaps,
            vec![feature(
                "f1",
                "orders",
                "services/orders/src/main.rs",
                covered,
            )],
        )
    };

    // ADMISSION_NO_COMPARISON: no gap digests, and no view digest.
    for request in [
        wired(&[], &[]),
        plan(
            REPO,
            Digest256::from_bytes([0u8; 32]),
            &["gap-a"],
            vec![feature(
                "f1",
                "orders",
                "services/orders/src/main.rs",
                &["gap-a"],
            )],
        ),
    ] {
        let error = decide(&h, admission_request(&pin, request))
            .await
            .unwrap_err();
        assert!(
            error.starts_with(ADMISSION_NO_COMPARISON.as_str()),
            "{error}"
        );
    }

    // ADMISSION_STALE_VIEW: the plan was made against another view.
    let stale = plan(
        REPO,
        gap("an older view"),
        &["gap-a"],
        vec![feature(
            "f1",
            "orders",
            "services/orders/src/main.rs",
            &["gap-a"],
        )],
    );
    let error = decide(&h, admission_request(&pin, stale))
        .await
        .unwrap_err();
    assert!(error.starts_with(ADMISSION_STALE_VIEW.as_str()), "{error}");

    // ADMISSION_FEATURE_UNWIRED names the feature.
    let unwired = plan(
        REPO,
        view,
        &["gap-a"],
        vec![feature("f-unwired", "orders", "", &["gap-a"])],
    );
    let batch = decide(&h, admission_request(&pin, unwired)).await.unwrap();
    let (code, detail) = held(&batch.records.as_slice()[0]);
    assert_eq!(code, ADMISSION_FEATURE_UNWIRED.as_str());
    assert!(detail.contains("f-unwired"), "{detail}");
    assert!(batch.records.as_slice()[0].answer.is_none());

    // ADMISSION_GAP_UNCOVERED names the gap digest.
    let batch = decide(
        &h,
        admission_request(&pin, wired(&["gap-a", "gap-orphan"], &["gap-a"])),
    )
    .await
    .unwrap();
    let (code, detail) = held(&batch.records.as_slice()[0]);
    assert_eq!(code, ADMISSION_GAP_UNCOVERED.as_str());
    assert!(detail.contains(&gap("gap-orphan").to_hex()), "{detail}");
    assert!(batch.records.as_slice()[0].answer.is_none());

    assert_eq!(
        artifact_count(&h),
        stored,
        "a refusal admits and writes nothing"
    );
}
