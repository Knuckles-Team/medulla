//! The `skill_upgrade` family through the actual Decide path
//! (MEDULLA-DECISION-ENGINE-R167.3, R167.4, R167.5, R167.6): additive upgrades and breaking
//! upgrades with a declared migration admit as a constraint with a receipt; a
//! breaking upgrade without a migration and a digest that does not match
//! abstain with their registered codes and write nothing.

use super::*;
use medulla_decision_types::decision::skill_upgrade_request::{
    ConsumerContractSchema, ContractField, SkillUpgradeReceipt, SkillUpgradeRequest,
    UpgradeSubjectKind, SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION, SKILL_UPGRADE_DIGEST_MISMATCH,
    SKILL_UPGRADE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::{AbstainReason, PremiseClass, ResolutionKind};
use medulla_types::contract::{Digest256, OpaqueId};

fn contract(fields: &[(&str, &str, bool)]) -> ConsumerContractSchema {
    ConsumerContractSchema::new(
        fields
            .iter()
            .map(|(name, type_name, required)| ContractField {
                name: (*name).into(),
                type_name: (*type_name).into(),
                required: *required,
            })
            .collect(),
    )
    .unwrap()
}

fn upgrade(
    previous: ConsumerContractSchema,
    new: ConsumerContractSchema,
    migration: Option<&str>,
) -> SkillUpgradeRequest {
    SkillUpgradeRequest {
        schema_version: SKILL_UPGRADE_SCHEMA_VERSION,
        subject_kind: UpgradeSubjectKind::Skill,
        subject_id: OpaqueId::new("skill:summarize").unwrap(),
        previous,
        new,
        declared_migration_id: migration.map(|id| OpaqueId::new(id).unwrap()),
        claimed_previous_digest: None,
        claimed_new_digest: None,
    }
}

fn upgrade_request(pin: &ComponentDependency, upgrade: SkillUpgradeRequest) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "skill.upgrade".into(),
            kind: QuestionKind::SkillUpgrade,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "admit_upgrade".into(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "skill_upgrade".into(),
            value: TypedValue::SkillUpgrade(Box::new(upgrade)),
        }])
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

fn abstain_code(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome);
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family { family, code, .. } => {
            assert_eq!(*family, QuestionKind::SkillUpgrade);
            code.as_str().to_string()
        }
        other => panic!("expected a family abstention, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R167.3
#[tokio::test]
async fn compatible_and_migrated_upgrades_admit() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let before = artifact_count(&h);
    let previous = contract(&[("a", "int", true), ("b", "text", false)]);
    let cases = [
        // Additive only: a new optional field and a required field made optional.
        (
            contract(&[
                ("a", "int", false),
                ("b", "text", false),
                ("c", "int", false),
            ]),
            None,
        ),
        // Breaking (a type change), with a declared migration.
        (
            contract(&[("a", "text", true), ("b", "text", false)]),
            Some("migration:skill-v2"),
        ),
    ];
    for (new, migration) in cases {
        let request = upgrade(previous.clone(), new, migration);
        let expected = SkillUpgradeReceipt::of_request(&request).unwrap();
        let batch = decide(&h, upgrade_request(&pin, request)).await.unwrap();
        let record = &batch.records.as_slice()[0];
        let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
            panic!("expected an admit, got {:?}", record.outcome);
        };
        assert_eq!(scores.as_slice()[0].option_id, "admit_upgrade");
        assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
        assert_eq!(record.evidence_class, EvidenceClass::Claim);
        let Some(FamilyAnswer::SkillUpgrade(receipt)) = &record.answer else {
            panic!("the record carries the receipt");
        };
        assert_eq!(**receipt, expected);
        assert_eq!(receipt.declared_migration_id.is_some(), migration.is_some());
        assert_ne!(
            receipt.previous_consumer_contract_digest,
            receipt.new_consumer_contract_digest
        );
        assert_eq!(
            medulla_decision_types::decision::digest::statistical_record_digest(record),
            record.record_digest
        );
    }
    assert_eq!(artifact_count(&h), before, "Decide writes nothing");

    // A foreign tenant is refused before any read.
    let mut foreign = upgrade_request(&pin, upgrade(previous.clone(), previous.clone(), None));
    foreign.tenant_id = "foreign-tenant".into();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R167.4
#[tokio::test]
async fn breaking_upgrade_without_migration_refuses() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let before = artifact_count(&h);
    let previous = contract(&[("a", "int", true), ("b", "text", false)]);

    let breaking = upgrade(previous.clone(), contract(&[("a", "int", true)]), None);
    let batch = decide(&h, upgrade_request(&pin, breaking)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(
        abstain_code(record),
        SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION.as_str()
    );
    assert!(record.answer.is_none(), "a refusal returns no receipt");
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);

    let mut mismatched = upgrade(previous.clone(), previous.clone(), None);
    mismatched.claimed_new_digest = Some(Digest256::from_bytes([9; 32]));
    let batch = decide(&h, upgrade_request(&pin, mismatched)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(abstain_code(record), SKILL_UPGRADE_DIGEST_MISMATCH.as_str());
    assert!(record.answer.is_none());

    // A matching claimed digest is accepted.
    let mut claimed = upgrade(previous.clone(), previous.clone(), None);
    claimed.claimed_previous_digest = Some(previous.digest().unwrap());
    claimed.claimed_new_digest = Some(previous.digest().unwrap());
    let batch = decide(&h, upgrade_request(&pin, claimed)).await.unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Advisory { .. }
    ));

    // Request refusals: a wrong option and a missing upgrade param.
    let mut wrong_option = upgrade_request(&pin, upgrade(previous.clone(), previous.clone(), None));
    wrong_option.candidates = CandidateSource::Declared {
        options: BoundedVec::default(),
    };
    let response =
        super::super::statistical::handle_decide(&h.state, 1, &verified(), wrong_option).await;
    assert_eq!(response.error.as_deref(), Some("PARAMETER_INVALID"));
    let mut missing = upgrade_request(&pin, upgrade(previous.clone(), previous, None));
    missing.params = BoundedVec::default();
    let response =
        super::super::statistical::handle_decide(&h.state, 1, &verified(), missing).await;
    assert_eq!(response.error.as_deref(), Some("PARAMETER_INVALID"));

    assert_eq!(artifact_count(&h), before, "a refusal writes nothing");
}

fn upgrade_facts(record: &StatisticalDecisionRecord) -> Vec<String> {
    record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("upgrade:"))
        .map(|p| p.fact.clone())
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R167.5
#[tokio::test]
async fn upgrade_evidence_names_digests_and_migration() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let previous = contract(&[("a", "int", true)]);
    let new = contract(&[("a", "int", true), ("c", "int", false)]);
    let breaking = contract(&[("a", "text", true)]);
    for (target, migration, diff) in [
        (new.clone(), None, "additive"),
        (breaking, Some("migration:skill-v2"), "breaking"),
        (previous.clone(), None, "identical"),
    ] {
        let request = upgrade(previous.clone(), target.clone(), migration);
        let batch = decide(&h, upgrade_request(&pin, request)).await.unwrap();
        let record = &batch.records.as_slice()[0];
        let facts = upgrade_facts(record);
        assert_eq!(
            facts,
            vec![
                format!(
                    "upgrade:contract_comparison previous={} new={} diff={diff}",
                    previous.digest().unwrap().to_hex(),
                    target.digest().unwrap().to_hex()
                ),
                format!("upgrade:migration={}", migration.unwrap_or("none")),
            ]
        );
        assert!(record
            .premises
            .iter()
            .filter(|p| p.fact.starts_with("upgrade:"))
            .all(|p| p.class == PremiseClass::Claim));
        assert_eq!(record.evidence_class, EvidenceClass::Claim);
        // The premises are sealed into the record digest.
        assert_eq!(
            medulla_decision_types::decision::digest::statistical_record_digest(record),
            record.record_digest
        );
    }
}

// spec: MEDULLA-DECISION-ENGINE-R167.6
#[tokio::test]
async fn upgrade_admission_journey() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let previous = contract(&[("a", "int", true), ("b", "text", false)]);
    let before = artifact_count(&h);
    // (new contract, migration, admitted)
    let cases = [
        (
            contract(&[
                ("a", "int", true),
                ("b", "text", false),
                ("c", "int", false),
            ]),
            None,
            true,
        ),
        (contract(&[("a", "int", true)]), None, false),
        (
            contract(&[("a", "int", true)]),
            Some("migration:drop-b"),
            true,
        ),
    ];
    for (new, migration, admitted) in cases {
        let request = upgrade(previous.clone(), new, migration);
        let batch = decide(&h, upgrade_request(&pin, request)).await.unwrap();
        let record = &batch.records.as_slice()[0];
        if admitted {
            assert!(matches!(
                record.outcome,
                StatisticalOutcome::Advisory { .. }
            ));
            assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
            assert!(matches!(record.answer, Some(FamilyAnswer::SkillUpgrade(_))));
        } else {
            assert_eq!(
                abstain_code(record),
                SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION.as_str()
            );
            assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
            assert!(record.answer.is_none());
        }
        assert_eq!(record.evidence_class, EvidenceClass::Claim);
        assert_eq!(
            upgrade_facts(record).len(),
            2,
            "every case seals its evidence"
        );
    }
    assert_eq!(artifact_count(&h), before, "Decide writes nothing");
}
