//! MEDULLA-DECISION-ENGINE-R134.14: branch outcomes are recorded through
//! `DecisionLog` (`RunRecord::BranchRecord`) and fused through `Decide`
//! (`QuestionKind::BranchOutcome`). `Decide` writes nothing; every refusal
//! stores nothing; another tenant's record is never read.

use super::*;
use medulla_decision_types::decision::branch_fusion::{
    branch_record_key, BranchRecord, BranchRef, BranchResult, BranchRetrievalReceipt,
    FusionReceipt, PlannedBranch,
};
use medulla_decision_types::decision::program_store::{RunRecord, RunRecordStored, StoredRun};
use medulla_decision_types::decision::PremiseClass;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};

const FOREIGN: &str = "tenant-foreign";
const PLAN: &str = "plan-digest-1";
const INPUT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OUTCOME: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn record(branch: &str, result: BranchResult) -> BranchRecord {
    BranchRecord {
        branch_id: branch.into(),
        plan_digest: PLAN.into(),
        input_digest: INPUT.into(),
        experience_revision: 7,
        fusion_position: None,
        result,
    }
}

fn outcome() -> BranchResult {
    BranchResult::Outcome {
        outcome_digest: OUTCOME.into(),
    }
}

async fn write(h: &Harness, record: RunRecord) -> Result<RunRecordStored, String> {
    let op = DecisionLogOp::RunRecord {
        tenant_id: TENANT.to_string(),
        record,
    };
    decode(super::log_tests::log_op(h, "brancher", op).await)
}

async fn write_branch(h: &Harness, record: BranchRecord) -> Result<RunRecordStored, String> {
    write(h, RunRecord::BranchRecord { record }).await
}

fn planned(ids: &[&str]) -> String {
    let planned: Vec<PlannedBranch> = ids
        .iter()
        .map(|id| PlannedBranch {
            branch_id: (*id).into(),
            input_digest: INPUT.into(),
        })
        .collect();
    serde_json::to_string(&planned).unwrap()
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn params(planned_json: &str, policy: bool) -> Vec<TypedParam> {
    let mut out = vec![
        text("plan_digest", PLAN),
        text("planned_branches", planned_json),
    ];
    if policy {
        out.push(text("fusion_policy_id", "fuse-by-id"));
        out.push(TypedParam {
            name: "fusion_policy_revision".to_string(),
            value: TypedValue::Int(2),
        });
        out.push(text("fusion_policy_digest", INPUT));
    }
    out
}

fn fuse_in(
    h: &Harness,
    tenant: &str,
    params: Vec<TypedParam>,
) -> Result<StatisticalDecisionRecord, String> {
    let schema = publish_route_schema_in(h, tenant);
    let request = declared_request(
        &schema,
        tenant,
        QuestionKind::BranchOutcome,
        "fusion",
        DecisionPolicyRef::Default,
        params,
    );
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        &request,
        None,
        "decider".to_string(),
        std::time::Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

fn receipt_of(record: &StatisticalDecisionRecord) -> FusionReceipt {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let id = &scores.as_slice()[0].option_id;
            serde_json::from_str(id.strip_prefix("receipt:").expect("a receipt answer")).unwrap()
        }
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn rows(h: &Harness, tenant: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(tenant, "run_record/", 100)
        .unwrap()
        .len()
}

// spec: MEDULLA-DECISION-ENGINE-R134.14
#[tokio::test]
async fn complete_branch_set_yields_ordered_fusion_receipt() {
    let h = Harness::new().await;
    let gap = BranchResult::Gap {
        failure_gap: "tool_timeout".into(),
    };
    // Written out of order; the fusion orders them by branch id.
    let stored_b = write_branch(&h, record("b", gap.clone())).await.unwrap();
    let stored_a = write_branch(&h, record("a", outcome())).await.unwrap();
    assert!(!stored_a.replayed);
    assert_eq!(stored_a.key, branch_record_key(PLAN, "a"));
    assert_eq!(rows(&h, TENANT), 2);

    let sealed = fuse_in(&h, TENANT, params(&planned(&["b", "a"]), true)).unwrap();
    let receipt = receipt_of(&sealed);
    assert_eq!(receipt.planned, ["a", "b"]);
    assert_eq!(receipt.policy.fusion_policy_id, "fuse-by-id");
    assert_eq!(receipt.policy.revision, 2);
    assert_eq!(receipt.branches[0].fusion_position, 0);
    assert_eq!(receipt.branches[1].fusion_position, 1);
    assert_eq!(receipt.branches[1].result, gap);
    assert_eq!(sealed.question.kind, QuestionKind::BranchOutcome);
    assert_eq!(rows(&h, TENANT), 2, "Decide wrote nothing");
    for fact in [
        format!("branch:id=a;position=0;result={OUTCOME}"),
        "branch:id=b;position=1;result=gap:tool_timeout".to_string(),
    ] {
        let found = sealed.premises.iter().find(|p| p.fact == fact);
        assert_eq!(found.map(|p| p.class), Some(PremiseClass::Claim), "{fact}");
    }

    // Independent readback of every outcome: each ref names the stored row.
    for (fused, stored) in receipt.branches.iter().zip([&stored_a, &stored_b]) {
        let StoredRun::BranchRecord { record } = &stored.stored else {
            panic!("a branch record");
        };
        let bytes = h
            .store
            .decision_artifact(TENANT, &fused.record.record_id)
            .unwrap()
            .expect("the cited row is stored");
        let read: BranchRecord = decode_artifact(&bytes, "branch record").unwrap();
        assert_eq!(&read, record);
        assert_eq!(fused.record, BranchRef::of(&read));
    }

    // A later branch cites the exact pinned prior ref and it is stored.
    let cited = BranchRetrievalReceipt {
        plan_digest: PLAN.into(),
        consumer_branch_id: "c".into(),
        refs: vec![receipt.branches[0].record.clone()],
    };
    let stored = write(
        &h,
        RunRecord::BranchRetrieval {
            receipt: cited.clone(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(stored.stored, StoredRun::BranchRetrieval { .. }));
    assert_eq!(rows(&h, TENANT), 3);

    // A ref that does not read back as cited is refused and stores nothing.
    let mut stale = cited.clone();
    stale.consumer_branch_id = "d".into();
    stale.refs[0].revision += 1;
    let err = write(&h, RunRecord::BranchRetrieval { receipt: stale })
        .await
        .unwrap_err();
    assert!(err.starts_with("BRANCH_READBACK_STALE"), "{err}");
    assert_eq!(rows(&h, TENANT), 3);

    // A retry with equal content replays; other content under the key refuses.
    assert!(
        write_branch(&h, record("a", outcome()))
            .await
            .unwrap()
            .replayed
    );
    let other = write_branch(&h, record("a", gap)).await.unwrap_err();
    assert!(other.starts_with("BRANCH_DUPLICATE"), "{other}");
    assert_eq!(rows(&h, TENANT), 3);

    branch_fusion_refusals_name_their_codes_and_leak_nothing().await;
}

/// Each refusal code and the cross-tenant case; the named test runs it.
async fn branch_fusion_refusals_name_their_codes_and_leak_nothing() {
    let h = Harness::new().await;
    write_branch(&h, record("a", outcome())).await.unwrap();

    let refused = |result: Result<StatisticalDecisionRecord, String>, code: &str| {
        let err = result.unwrap_err();
        assert!(err.contains(code), "{code}: {err}");
    };
    // A planned branch with no record.
    refused(
        fuse_in(&h, TENANT, params(&planned(&["a", "b"]), true)),
        "BRANCH_MISSING",
    );
    // A branch planned twice.
    refused(
        fuse_in(&h, TENANT, params(&planned(&["a", "a"]), true)),
        "BRANCH_DUPLICATE",
    );
    // A branch that ran on another input than planned.
    let moved = serde_json::to_string(&[PlannedBranch {
        branch_id: "a".into(),
        input_digest: OUTCOME.into(),
    }])
    .unwrap();
    refused(
        fuse_in(&h, TENANT, params(&moved, true)),
        "BRANCH_INPUT_CHANGED",
    );
    // No pinned policy.
    refused(
        fuse_in(&h, TENANT, params(&planned(&["a"]), false)),
        "FUSION_POLICY_ABSENT",
    );
    // A failed branch that omits its gap is refused at the write.
    let bare = write_branch(
        &h,
        record(
            "z",
            BranchResult::Gap {
                failure_gap: String::new(),
            },
        ),
    )
    .await
    .unwrap_err();
    assert!(bare.starts_with("BRANCH_MISSING"), "{bare}");
    assert_eq!(rows(&h, TENANT), 1);

    // Another tenant's record is never read: it stays missing here.
    let foreign = record("f", outcome());
    h.store
        .put_decision_artifacts(
            FOREIGN,
            &[(foreign.key(), encode_artifact(&foreign).unwrap())],
        )
        .unwrap();
    refused(
        fuse_in(&h, TENANT, params(&planned(&["f"]), true)),
        "BRANCH_MISSING",
    );
    assert!(fuse_in(&h, FOREIGN, params(&planned(&["f"]), true)).is_ok());
}
