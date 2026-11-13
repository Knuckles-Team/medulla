//! Served write-back preflight and proposals (MEDULLA-DECISION-ENGINE-R615.2.1,
//! R615.2.2): `Method::DecisionLog` `Writeback` reaches the preflight rule and
//! the creation and ticket proposals through the real dispatch, under the
//! verified request tenant.

use super::log_tests::log_op;
use super::*;
use medulla_decision_types::decision::statistical::writeback::{
    InventoryItemInput, PreflightVerdict, ProposalInput, ProposalKind, QueuedProposalInput,
    RiskFindingInput, RiskTier, WritebackAudit, WritebackAuditOutcome, WritebackAuditRows,
    WritebackOp, WritebackOutcome,
};
use std::collections::BTreeMap;

async fn writeback(h: &Harness, op: WritebackOp) -> Result<WritebackOutcome, String> {
    writeback_as(h, "writer", op).await
}

async fn writeback_as(h: &Harness, who: &str, op: WritebackOp) -> Result<WritebackOutcome, String> {
    decode(
        log_op(
            h,
            who,
            DecisionLogOp::Writeback {
                tenant_id: TENANT.to_string(),
                computation: op,
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
        computation: WritebackOp::Propose {
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

fn queued(id: &str, external: &str, tier: RiskTier) -> QueuedProposalInput {
    QueuedProposalInput {
        proposal: ProposalInput {
            id: id.to_string(),
            external_id: external.to_string(),
            system_of_record: "sor:ledger".to_string(),
            risk_tier: tier,
            as_of_ms: 1_700_000_000_000,
        },
        source_values: BTreeMap::new(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R615.3
#[tokio::test]
async fn high_tier_proposal_queues_and_releases_after_approval() {
    let h = Harness::new().await;
    let queue_id = "q:1";
    let release = || WritebackOp::Release {
        queue_id: queue_id.to_string(),
    };
    let approve = |proposal: &str| WritebackOp::Approve {
        queue_id: queue_id.to_string(),
        proposal_id: proposal.to_string(),
    };

    // Preflight first: a low-tier proposal never queues; a source conflict refuses.
    let low = writeback(
        &h,
        WritebackOp::Queue {
            queue_id: "q:low".to_string(),
            proposals: vec![queued("wp:l", "ext:l", RiskTier::Low)],
        },
    )
    .await
    .unwrap_err();
    assert!(low.starts_with("WRITEBACK_BELOW_QUEUE_TIER"), "{low}");
    let mut conflicting = queued("wp:c", "ext:c", RiskTier::High);
    conflicting.source_values = values(&[("a", "d1"), ("b", "d2")]);
    let conflict = writeback(
        &h,
        WritebackOp::Queue {
            queue_id: "q:conflict".to_string(),
            proposals: vec![conflicting],
        },
    )
    .await
    .unwrap_err();
    assert!(
        conflict.starts_with("WRITEBACK_SOURCE_CONFLICT"),
        "{conflict}"
    );
    assert!(writeback(
        &h,
        WritebackOp::Queue {
            queue_id: "q:none".into(),
            proposals: vec![]
        }
    )
    .await
    .unwrap_err()
    .starts_with("WRITEBACK_QUEUE_TOO_LARGE"));

    // Queue two high-tier proposals.
    let stored = writeback(
        &h,
        WritebackOp::Queue {
            queue_id: queue_id.to_string(),
            proposals: vec![
                queued("wp:1", "ext:1", RiskTier::High),
                queued("wp:2", "ext:2", RiskTier::High),
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        stored,
        WritebackOutcome::Queued {
            queue_id: queue_id.to_string(),
            proposal_ids: vec!["wp:1".to_string(), "wp:2".to_string()],
        }
    );
    let again = writeback(
        &h,
        WritebackOp::Queue {
            queue_id: queue_id.to_string(),
            proposals: vec![queued("wp:9", "ext:9", RiskTier::High)],
        },
    )
    .await
    .unwrap_err();
    assert!(again.starts_with("WRITEBACK_QUEUE_EXISTS"), "{again}");

    // Release before every approval refuses and builds no change set.
    let early = writeback(&h, release()).await.unwrap_err();
    assert!(early.starts_with("PROPOSAL_UNAPPROVED"), "{early}");
    let first = writeback_as(&h, "approver-1", approve("wp:1"))
        .await
        .unwrap();
    let WritebackOutcome::Approved { approval } = first else {
        panic!("approval expected")
    };
    assert_eq!(approval.proposal_id, "wp:1");
    assert!(
        approval.approver.contains("approver-1"),
        "{}",
        approval.approver
    );
    assert!(approval.approved_at_ms > 0);
    let unknown = writeback_as(&h, "approver-1", approve("wp:404"))
        .await
        .unwrap_err();
    assert!(
        unknown.starts_with("WRITEBACK_PROPOSAL_UNKNOWN"),
        "{unknown}"
    );
    assert!(writeback(&h, release())
        .await
        .unwrap_err()
        .starts_with("PROPOSAL_UNAPPROVED"));

    // A repeated approval keeps the first approver and instant.
    let repeat = writeback_as(&h, "approver-2", approve("wp:1"))
        .await
        .unwrap();
    assert_eq!(
        repeat,
        WritebackOutcome::Approved {
            approval: approval.clone()
        }
    );
    writeback_as(&h, "approver-2", approve("wp:2"))
        .await
        .unwrap();

    // Release returns the change set once; a second release returns the same one.
    let WritebackOutcome::Released { change_set } = writeback(&h, release()).await.unwrap() else {
        panic!("released change set expected")
    };
    assert_eq!(change_set.change_set_id, "changeset:q:1");
    let ids: Vec<_> = change_set.proposals.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["wp:1", "wp:2"]);
    assert_eq!(change_set.approvals.len(), 2);
    let replay = writeback(&h, release()).await.unwrap();
    assert_eq!(replay, WritebackOutcome::Released { change_set });

    // A released queue takes no further approval.
    let late = writeback_as(&h, "approver-3", approve("wp:2"))
        .await
        .unwrap_err();
    assert!(
        late.starts_with("WRITEBACK_QUEUE_TRANSITION_REFUSED"),
        "{late}"
    );
    assert!(
        late.contains("proposal_queue") && late.contains("released"),
        "{late}"
    );
    assert!(writeback(
        &h,
        WritebackOp::Release {
            queue_id: "q:404".into()
        }
    )
    .await
    .unwrap_err()
    .starts_with("WRITEBACK_QUEUE_UNKNOWN"));

    // Cross-tenant: the verified tenant must match the one the op names.
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("writer", TENANT);
    let foreign = DecisionLogOp::Writeback {
        tenant_id: "tenant-other".to_string(),
        computation: release(),
    };
    let response = super::super::log::handle_decision_log(&h.state, 9, &verified, foreign).await;
    assert!(response.error.unwrap().contains("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R615.4
#[tokio::test]
async fn unapproved_release_refuses_without_partial_write() {
    use medulla_decision_types::decision::codes::{
        self, CONNECTOR_WRITE_BACK, PROPOSAL_UNAPPROVED, WRITEBACK_SOURCE_CONFLICT,
    };
    // Both codes are registered in the connector_write_back family table.
    assert!(CONNECTOR_WRITE_BACK.contains(&PROPOSAL_UNAPPROVED));
    assert!(CONNECTOR_WRITE_BACK.contains(&WRITEBACK_SOURCE_CONFLICT));
    assert_eq!(
        codes::resolve("PROPOSAL_UNAPPROVED"),
        Some(PROPOSAL_UNAPPROVED)
    );

    let h = Harness::new().await;
    let queue_id = "q:unapproved";
    // The queue and change-set rows; the refused release's audit rows
    // (R615.5) are a separate family and are asserted by their own test.
    let stored_rows = || {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 100_000)
            .unwrap()
            .into_iter()
            .filter(|(key, _)| !key.starts_with("writeback_audit:"))
            .collect::<Vec<_>>()
    };
    writeback(
        &h,
        WritebackOp::Queue {
            queue_id: queue_id.to_string(),
            proposals: vec![
                queued("wp:1", "ext:1", RiskTier::High),
                queued("wp:2", "ext:2", RiskTier::High),
            ],
        },
    )
    .await
    .unwrap();
    writeback_as(
        &h,
        "approver-1",
        WritebackOp::Approve {
            queue_id: queue_id.to_string(),
            proposal_id: "wp:1".to_string(),
        },
    )
    .await
    .unwrap();
    let before = stored_rows();

    // One proposal holds no approval: the whole release refuses.
    let release = || WritebackOp::Release {
        queue_id: queue_id.to_string(),
    };
    let error = writeback(&h, release()).await.unwrap_err();
    assert!(error.starts_with(PROPOSAL_UNAPPROVED.as_str()), "{error}");

    // Nothing was written: the stored rows are byte-for-byte unchanged and no
    // change set was stored.
    let after = stored_rows();
    assert_eq!(before, after);
    assert!(after.iter().all(|(key, _)| !key.contains("changeset")));

    // The refused release left the queue pending: the last approval releases.
    writeback_as(
        &h,
        "approver-2",
        WritebackOp::Approve {
            queue_id: queue_id.to_string(),
            proposal_id: "wp:2".to_string(),
        },
    )
    .await
    .unwrap();
    let WritebackOutcome::Released { change_set } = writeback(&h, release()).await.unwrap() else {
        panic!("released change set expected")
    };
    assert_eq!(change_set.approvals.len(), 2);

    // A source conflict refuses at queue time with its registered code.
    let mut conflicting = queued("wp:c", "ext:c", RiskTier::High);
    conflicting.source_values = values(&[("a", "d1"), ("b", "d2")]);
    let conflict = writeback(
        &h,
        WritebackOp::Queue {
            queue_id: "q:conflict".to_string(),
            proposals: vec![conflicting],
        },
    )
    .await
    .unwrap_err();
    assert!(
        conflict.starts_with(WRITEBACK_SOURCE_CONFLICT.as_str()),
        "{conflict}"
    );
    assert_eq!(stored_rows().len(), after.len());
}

async fn audit_rows(h: &Harness, who: &str, change_set_id: &str) -> Vec<WritebackAudit> {
    let rows: WritebackAuditRows = decode(
        log_op(
            h,
            who,
            DecisionLogOp::WritebackAudit {
                tenant_id: TENANT.to_string(),
                change_set_id: change_set_id.to_string(),
            },
        )
        .await,
    )
    .unwrap();
    rows.rows
}

// spec: MEDULLA-DECISION-ENGINE-R615.5
#[tokio::test]
async fn release_and_refusal_leave_audit_rows() {
    let h = Harness::new().await;
    let queue_id = "q:audit";
    let change_set_id = "changeset:q:audit";
    let release = || WritebackOp::Release {
        queue_id: queue_id.to_string(),
    };
    writeback(
        &h,
        WritebackOp::Queue {
            queue_id: queue_id.to_string(),
            proposals: vec![
                queued("wp:1", "ext:1", RiskTier::High),
                queued("wp:2", "ext:2", RiskTier::High),
            ],
        },
    )
    .await
    .unwrap();
    // Nothing released yet: the audit reads empty.
    assert!(audit_rows(&h, "writer", change_set_id).await.is_empty());

    writeback_as(
        &h,
        "approver-1",
        WritebackOp::Approve {
            queue_id: queue_id.to_string(),
            proposal_id: "wp:1".to_string(),
        },
    )
    .await
    .unwrap();

    // A refused release leaves one refused row per proposal: the approved one
    // names its approver, the unapproved one names none.
    writeback(&h, release()).await.unwrap_err();
    let refused = audit_rows(&h, "writer", change_set_id).await;
    assert_eq!(refused.len(), 2);
    let code = medulla_decision_types::decision::codes::PROPOSAL_UNAPPROVED
        .as_str()
        .to_string();
    for row in &refused {
        assert_eq!(row.change_set_id, change_set_id);
        assert_eq!(row.risk_tier, RiskTier::High);
        assert_eq!(
            row.outcome,
            WritebackAuditOutcome::Refused { code: code.clone() }
        );
    }
    assert_eq!(refused[0].proposal_id, "wp:1");
    assert!(refused[0]
        .approver
        .as_deref()
        .unwrap()
        .contains("approver-1"));
    assert_eq!(refused[1].proposal_id, "wp:2");
    assert_eq!(refused[1].approver, None);

    // The release appends its released rows after the refused ones, in commit
    // order, with each proposal's approver.
    writeback_as(
        &h,
        "approver-2",
        WritebackOp::Approve {
            queue_id: queue_id.to_string(),
            proposal_id: "wp:2".to_string(),
        },
    )
    .await
    .unwrap();
    writeback(&h, release()).await.unwrap();
    let all = audit_rows(&h, "writer", change_set_id).await;
    assert_eq!(all.len(), 4);
    assert_eq!(all[..2], refused[..]);
    assert!(all[2..]
        .iter()
        .all(|r| r.outcome == WritebackAuditOutcome::Released));
    assert!(all[2].approver.as_deref().unwrap().contains("approver-1"));
    assert!(all[3].approver.as_deref().unwrap().contains("approver-2"));
    assert_eq!(all[2].as_of_ms, 1_700_000_000_000);

    // Releasing a released queue returns the same set and adds no rows.
    writeback(&h, release()).await.unwrap();
    assert_eq!(audit_rows(&h, "writer", change_set_id).await.len(), 4);

    // Another owner reads nothing of the releaser's rows, exactly as for a
    // change set nobody released.
    assert!(audit_rows(&h, "approver-1", change_set_id).await.is_empty());
    assert!(audit_rows(&h, "writer", "changeset:q:none")
        .await
        .is_empty());
}
