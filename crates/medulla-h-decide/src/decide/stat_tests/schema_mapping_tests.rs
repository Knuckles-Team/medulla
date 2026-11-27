//! MEDULLA-DECISION-ENGINE-R033.2.2: the served `SchemaMapping` family maps a
//! column to the declared ontology property its score names, with the
//! premise `mapping:<column>=<property>` at the rule's evidence class.

use super::*;
use medulla_decision_types::decision::statistical::declared::{DeclaredOption, DeclaredText};
use medulla_decision_types::decision::{PremiseClass, PremiseRef};

const BASE: &str = "https://knuckles-team.github.io/ontology/customer#";

fn property(local: &str, datatype: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: format!("{BASE}{local}"),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "datatype".to_string(),
            text: datatype.to_string(),
        }])
        .unwrap(),
    }
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn mapping_request(
    schema: &ComponentDependency,
    column: &str,
    sql_type: &str,
    samples: &[&str],
) -> DecideRequest {
    let mut params = vec![
        text("column.name", column),
        text("column.sql_type", sql_type),
    ];
    if !samples.is_empty() {
        params.push(TypedParam {
            name: "column.samples".to_string(),
            value: TypedValue::IriList(
                BoundedVec::new(samples.iter().map(|s| s.to_string()).collect()).unwrap(),
            ),
        });
    }
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "mapping.column".to_string(),
            kind: QuestionKind::SchemaMapping,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![
                property("createdAt", "dateTime"),
                property("customerAge", "integer"),
                property("customerName", "string"),
            ])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: BoundedVec::default(),
    }
}

fn proposed(batch: &DecisionBatch) -> (String, Vec<PremiseRef>) {
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Advisory {
        scores, calibrated, ..
    } = &record.outcome
    else {
        panic!("expected a proposal, got {:?}", record.outcome)
    };
    assert!(!calibrated);
    let premises = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("mapping:"))
        .cloned()
        .collect();
    (scores.as_slice()[0].option_id.clone(), premises)
}

// spec: MEDULLA-DECISION-ENGINE-R033.2.2
#[tokio::test]
async fn columns_map_to_expected_properties_with_evidence_class() {
    let h = Harness::new().await;
    let (schema_pin, _digest) = publish_route_schema(&h);

    // An exact name and datatype match is a proof.
    let batch = decide(
        &h,
        mapping_request(&schema_pin, "customer_age", "INT", &["31", "44"]),
    )
    .await
    .unwrap();
    let (top, premises) = proposed(&batch);
    assert_eq!(top, format!("{BASE}customerAge"));
    assert_eq!(premises.len(), 1);
    assert_eq!(
        premises[0].fact,
        format!("mapping:customer_age={BASE}customerAge")
    );
    assert_eq!(premises[0].class, PremiseClass::Proof);
    assert_eq!(
        batch.records.as_slice()[0].resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Statistical
    );

    // A partial name with every sample valid is an observation.
    let batch = decide(
        &h,
        mapping_request(&schema_pin, "customer_age_years", "BIGINT", &["31", "44"]),
    )
    .await
    .unwrap();
    let (top, premises) = proposed(&batch);
    assert_eq!(top, format!("{BASE}customerAge"));
    assert_eq!(premises[0].class, PremiseClass::Observation);

    // Below `schema_mapping.accept_min` the decision abstains.
    let batch = decide(
        &h,
        mapping_request(&schema_pin, "years_old", "INT", &["31"]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome)
    };
    assert_eq!(
        reasons.as_slice(),
        &[medulla_decision_types::decision::AbstainReason::InsufficientConfidence]
    );
    assert!(record
        .premises
        .iter()
        .all(|p| !p.fact.starts_with("mapping:")));
}

// spec: MEDULLA-DECISION-ENGINE-R033.2.2
#[tokio::test]
async fn schema_mapping_refuses_a_missing_column_an_unknown_type_and_another_tenant() {
    let h = Harness::new().await;
    let (schema_pin, _digest) = publish_route_schema(&h);

    let mut request = mapping_request(&schema_pin, "customer_age", "INT", &[]);
    request.params = BoundedVec::new(vec![text("column.sql_type", "INT")]).unwrap();
    let refused = decide(&h, request).await.unwrap_err();
    assert!(
        refused.starts_with("SCHEMA_MAPPING_COLUMN_MISSING"),
        "{refused}"
    );

    let refused = decide(
        &h,
        mapping_request(&schema_pin, "customer_age", "geometry", &[]),
    )
    .await
    .unwrap_err();
    assert!(
        refused.starts_with("SCHEMA_MAPPING_SQL_TYPE_UNKNOWN"),
        "{refused}"
    );

    let mut request = mapping_request(&schema_pin, "customer_age", "INT", &[]);
    request.tenant_id = "foreign-tenant".into();
    let refused = decide(&h, request).await.unwrap_err();
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}
