//! Served write-back preflight and proposals (MEDULLA-DECISION-ENGINE-R615.2.1,
//! R615.2.2): `Method::DecisionLog` `Writeback` reaches the preflight rule and
//! the creation and ticket proposals through the real dispatch, under the
//! verified request tenant.

use super::log_tests::log_op;
use super::*;
use medulla_decision_types::decision::statistical::writeback::{
    InventoryItemInput, PreflightVerdict, ProposalInput, ProposalKind, RiskFindingInput, RiskTier,
    WritebackOp, WritebackOutcome,
};
use std::collections::BTreeMap;

async fn writeback(h: &Harness, op: WritebackOp) -> Result<WritebackOutcome, String> {
    decode(
        log_op(
            h,
            "writer",
            DecisionLogOp::Writeback {
                tenant_id: TENANT.to_string(),
                op,
            },
        )
        .await,
    )
}

fn proposal(tier: RiskTier) -> ProposalInput {
    ProposalInput {
        id: "wp:1".to_string(),
        external_id: "ext:1".to_string(),
        system_of_record: "sor:ledger".to_string(),
        risk_tier: tier,
        as_of_ms: 1_700_000_000_000,
    }
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R615.2.1
#[tokio::test]
async fn served_preflight_queues_high_tier_and_refuses_conflicts() {
    let h = Harness::new().await;
    let verdict = |outcome: WritebackOutcome| match outcome {
        WritebackOutcome::Preflight { verdict } => verdict,
        other => panic!("unexpected {other:?}"),
    };
    let high = writeback(
        &h,
        WritebackOp::Preflight {
            proposal: proposal(RiskTier::High),
            source_values: values(&[("a", "d1"), ("b", "d1")]),
        },
    )
    .await
    .unwrap();
    assert_eq!(verdict(high), PreflightVerdict::Queue);
    let low = writeback(
        &h,
        WritebackOp::Preflight {
            proposal: proposal(RiskTier::Low),
            source_values: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(verdict(low), PreflightVerdict::Proceed);
    let conflict = writeback(
        &h,
        WritebackOp::Preflight {
            proposal: proposal(RiskTier::Low),
            source_values: values(&[("a", "d1"), ("b", "d2")]),
        },
    )
    .await
    .unwrap();
    match verdict(conflict) {
        PreflightVerdict::Refuse { code, .. } => assert_eq!(code, "WRITEBACK_SOURCE_CONFLICT"),
        other => panic!("a conflict refuses, got {other:?}"),
    }
    // A foreign tenant is refused before any computation.
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("writer", TENANT);
    let foreign = DecisionLogOp::Writeback {
        tenant_id: "tenant-other".to_string(),
        op: WritebackOp::Propose {
            inventory: vec![],
            enabled_systems: vec![],
            findings: vec![],
        },
    };
    let response = super::super::log::handle_decision_log(&h.state, 9, &verified, foreign).await;
    assert!(response.error.unwrap().contains("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R615.2.2
#[tokio::test]
async fn served_proposals_are_sorted_and_carry_the_source_as_of() {
    let h = Harness::new().await;
    let outcome = writeback(
        &h,
        WritebackOp::Propose {
            inventory: vec![InventoryItemInput {
                external_id: "ext:a".to_string(),
                present_in: vec!["sor:ledger".to_string()],
                as_of_ms: 10,
            }],
            enabled_systems: vec!["sor:ledger".to_string(), "sor:crm".to_string()],
            findings: vec![RiskFindingInput {
                finding_id: "f:1".to_string(),
                external_id: "ext:z".to_string(),
                system_of_record: "sor:tickets".to_string(),
                risk_tier: RiskTier::High,
                as_of_ms: 20,
            }],
        },
    )
    .await
    .unwrap();
    let WritebackOutcome::Proposals { proposals } = outcome else {
        panic!("proposals expected")
    };
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0].kind, ProposalKind::InventoryCreation);
    assert_eq!(proposals[0].system_of_record, "sor:crm");
    assert_eq!(proposals[0].as_of_ms, 10);
    assert_eq!(proposals[1].kind, ProposalKind::FindingTicket);
    assert_eq!(proposals[1].risk_tier, Some(RiskTier::High));
    assert_eq!(proposals[1].as_of_ms, 20);
}
