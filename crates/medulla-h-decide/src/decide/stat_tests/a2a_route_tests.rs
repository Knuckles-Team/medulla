//! MEDULLA-DECISION-ENGINE-R044.2.2: a served `A2aRoute` question routes an
//! inbound task to the tenant's published graph it names, through a recorded
//! decision; an unknown or missing request abstains under its registered code,
//! and a foreign tenant's graph is never visible.

use super::*;
use medulla_decision_types::decision::{AbstainReason, EvidenceClass, PremiseProvenance};

const OTHER_TENANT: &str = "tenant-other";

fn publish_graph(h: &Harness, tenant: &str, id: &str) {
    let draft = test_component_draft(tenant, id);
    h.commit_in(tenant, draft, None).unwrap();
}

fn route_request(schema: &ComponentDependency, requested: Option<&str>) -> DecideRequest {
    let params = requested
        .into_iter()
        .map(|id| TypedParam {
            name: "a2a.requested_graph_id".to_string(),
            value: TypedValue::Text(id.to_string()),
        })
        .collect::<Vec<_>>();
    let mut request = declared_request(
        schema,
        TENANT,
        QuestionKind::A2aRoute,
        "unused",
        DecisionPolicyRef::Default,
        params,
    );
    request.candidates = CandidateSource::AgentLibrary {
        scope: LibraryCandidateScope {
            kinds: BoundedVec::new(vec![AgentComponentKind::Tool]).unwrap(),
            classification_under: None,
        },
    };
    request
}

fn abstained_code(batch: &DecisionBatch) -> String {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Abstained { reasons } => match &reasons.as_slice()[0] {
            AbstainReason::Family { code, .. } => code.as_str().to_string(),
            other => panic!("unexpected abstention {other:?}"),
        },
        other => panic!("expected an abstention, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R044.2.2
#[tokio::test]
async fn inbound_task_routes_through_a_recorded_decision() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_graph(&h, TENANT, "graph-1");
    publish_graph(&h, TENANT, "graph-2");
    publish_graph(&h, OTHER_TENANT, "graph-foreign");

    // An exact match routes to that one graph, as a constraint, observed.
    let batch = decide(&h, route_request(&schema, Some("graph-2")))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            assert_eq!(scores.len(), 1);
            assert_eq!(scores.as_slice()[0].option_id, "graph-2");
        }
        other => panic!("expected an advisory route, got {other:?}"),
    }
    assert_eq!(
        record.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Constraint
    );
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    let premise = record
        .premises
        .iter()
        .find(|p| p.fact == "a2a_route:graph-2")
        .expect("the route is a named premise");
    assert!(matches!(
        premise.provenance,
        PremiseProvenance::Publisher { ref component_id, .. } if component_id == "graph-2"
    ));

    // No match abstains under the registered code with claim-grade evidence.
    let batch = decide(&h, route_request(&schema, Some("graph-9")))
        .await
        .unwrap();
    assert_eq!(abstained_code(&batch), "A2A_GRAPH_UNKNOWN");
    assert_eq!(
        batch.records.as_slice()[0].evidence_class,
        EvidenceClass::Claim
    );

    // A task that names no graph is refused under its own code.
    let batch = decide(&h, route_request(&schema, None)).await.unwrap();
    assert_eq!(abstained_code(&batch), "A2A_REQUEST_MISSING");

    // Another tenant's published graph is invisible: it routes nowhere.
    let batch = decide(&h, route_request(&schema, Some("graph-foreign")))
        .await
        .unwrap();
    assert_eq!(abstained_code(&batch), "A2A_GRAPH_UNKNOWN");
}
