//! MEDULLA-DECISION-ENGINE-R135.3, R135.4: a successful run is committed
//! through `DecisionLog` (`DecisionLogOp::RunRecord`) and looked up through
//! `Decide` (`QuestionKind::ProgramLookup`). `Decide` writes nothing; every
//! refusal stores nothing; another tenant's program is never read.

use super::*;
use medulla_decision_types::decision::program_store::{
    program_key, CompiledProgram, RunRecord, RunRecordStored, StoredRun,
};
use medulla_decision_types::decision::run_governance::{
    RunOutcome, RunTraceRecord, StepKind, StepOutcome, TraceArgument, TraceScalar, TraceStep,
};
use medulla_decision_types::decision::{EvidenceClass, PremiseClass, ResolutionKind};
use medulla_persistence_records::decision_jobs::encode_artifact;

const FOREIGN: &str = "tenant-foreign";
const SIGNATURE: &str = "summarize-report";

fn trace(run_id: &str, outcome: RunOutcome, steps: usize, title: &str) -> RunTraceRecord {
    let make = |index: usize| TraceStep {
        index: index as u32,
        kind: StepKind::ToolCall,
        action_ref: format!("tool:t{index}"),
        outcome: StepOutcome::Ok,
        arguments: BoundedVec::new(vec![
            TraceArgument {
                name: "title".into(),
                value: TraceScalar::Text(title.into()),
            },
            TraceArgument {
                name: "limit".into(),
                value: TraceScalar::Int(10),
            },
        ])
        .unwrap(),
    };
    RunTraceRecord {
        schema_version: 1,
        run_id: run_id.into(),
        task_signature: SIGNATURE.into(),
        model_id: "model-1".into(),
        steps: BoundedVec::new((0..steps).map(make).collect()).unwrap(),
        outcome,
        started_at_ms: 1,
    }
}

fn run_record_op(trace: RunTraceRecord) -> DecisionLogOp {
    DecisionLogOp::RunRecord {
        tenant_id: TENANT.to_string(),
        record: RunRecord::CompiledProgram {
            trace,
            policy: DecisionPolicyRef::Default,
            reusable: false,
            plan_binding: None,
            preplan_receipt: None,
        },
    }
}

/// The compiled program a committed run record stored.
fn program_of(stored: &RunRecordStored) -> CompiledProgram {
    match &stored.stored {
        StoredRun::CompiledProgram { program, .. } => program.clone(),
        other => panic!("a program record, got {other:?}"),
    }
}

async fn commit_run(h: &Harness, trace: RunTraceRecord) -> Result<RunRecordStored, String> {
    decode(super::log_tests::log_op(h, "recorder", run_record_op(trace)).await)
}

fn lookup_request(h: &Harness, signature: Option<&str>) -> DecideRequest {
    let (schema, _) = publish_route_schema(h);
    let params = signature
        .map(|text| {
            vec![TypedParam {
                name: "task_signature".to_string(),
                value: TypedValue::Text(text.to_string()),
            }]
        })
        .unwrap_or_default();
    declared_request(
        &schema,
        TENANT,
        QuestionKind::ProgramLookup,
        "program",
        DecisionPolicyRef::Default,
        params,
    )
}

/// The program the answer carries, or the abstain codes it names.
fn answered(record: &StatisticalDecisionRecord) -> Result<CompiledProgram, Vec<String>> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let id = &scores.as_slice()[0].option_id;
            let body = id.strip_prefix("program:").expect("a program answer");
            Ok(serde_json::from_str(body).expect("the answer carries the program"))
        }
        StatisticalOutcome::Abstained { reasons } => Err(reasons
            .iter()
            .map(|reason| match reason {
                medulla_decision_types::decision::AbstainReason::Family { code, .. } => {
                    code.as_str().to_string()
                }
                other => panic!("unexpected abstain reason {other:?}"),
            })
            .collect()),
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn stored_rows(h: &Harness, tenant: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(tenant, "run_record/", 100)
        .unwrap()
        .len()
}

fn lookup_in(
    h: &Harness,
    tenant: &str,
    signature: &str,
) -> Result<StatisticalDecisionRecord, String> {
    let schema = publish_route_schema_in(h, tenant);
    let request = declared_request(
        &schema,
        tenant,
        QuestionKind::ProgramLookup,
        "program",
        DecisionPolicyRef::Default,
        vec![TypedParam {
            name: "task_signature".to_string(),
            value: TypedValue::Text(signature.to_string()),
        }],
    );
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
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

// spec: MEDULLA-DECISION-ENGINE-R135.3
#[tokio::test]
async fn decide_routes_program_lookup() {
    let h = Harness::new().await;

    // Nothing is committed: the lookup abstains and `Decide` stored nothing.
    let batch = decide(&h, lookup_request(&h, Some(SIGNATURE)))
        .await
        .unwrap();
    assert_eq!(
        answered(&batch.records.as_slice()[0]),
        Err(vec!["NO_PROGRAM".to_string()])
    );
    assert_eq!(stored_rows(&h, TENANT), 0);

    // The commit goes through `DecisionLog` and is the only write.
    let stored = commit_run(&h, trace("run-1", RunOutcome::Succeeded, 3, "weekly"))
        .await
        .unwrap();
    assert!(!stored.replayed);
    assert_eq!(stored.key, program_key(SIGNATURE));
    assert_eq!(stored_rows(&h, TENANT), 1);

    // The served lookup returns the committed program, equal field for field.
    let batch = decide(&h, lookup_request(&h, Some(SIGNATURE)))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(answered(record), Ok(program_of(&stored).clone()));
    assert_eq!(record.question.kind, QuestionKind::ProgramLookup);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    assert_eq!(stored_rows(&h, TENANT), 1, "Decide wrote nothing");

    // The premises (R135.5) name the signature, the counts and the source run.
    let premise = |fact: &str| record.premises.iter().find(|p| p.fact == fact);
    for fact in [
        "program.task_signature=summarize-report",
        "program.step_count=3",
        "program.parameter_count=2",
        "program.source_run_id=run-1",
    ] {
        let found = premise(fact).unwrap_or_else(|| panic!("premise {fact} is missing"));
        assert_eq!(found.class, PremiseClass::Claim);
    }
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("knob:program.max_steps=")));

    // A retry with equal content answers the stored row; other content under
    // the key refuses IDEMPOTENCY_CONFLICT and leaves the row as it was.
    let again = commit_run(&h, trace("run-1", RunOutcome::Succeeded, 3, "weekly"))
        .await
        .unwrap();
    assert!(again.replayed);
    assert_eq!(program_of(&again), program_of(&stored));
    let conflict = commit_run(&h, trace("run-2", RunOutcome::Succeeded, 3, "weekly"))
        .await
        .unwrap_err();
    assert!(conflict.starts_with("IDEMPOTENCY_CONFLICT"), "{conflict}");
    assert_eq!(stored_rows(&h, TENANT), 1);

    // Another tenant's program is never returned: the same signature stored
    // under a foreign tenant is invisible here, and visible to its own tenant.
    let foreign = CompiledProgram {
        source_run_id: "run-foreign".into(),
        ..program_of(&stored).clone()
    }
    .sealed();
    h.store
        .put_decision_artifacts(
            FOREIGN,
            &[(program_key(SIGNATURE), encode_artifact(&foreign).unwrap())],
        )
        .unwrap();
    assert_eq!(
        answered(&lookup_in(&h, TENANT, SIGNATURE).unwrap()),
        Ok(program_of(&stored))
    );
    assert_eq!(
        answered(&lookup_in(&h, FOREIGN, SIGNATURE).unwrap()),
        Ok(foreign)
    );
    assert_eq!(
        answered(&lookup_in(&h, FOREIGN, "never-compiled").unwrap()),
        Err(vec!["NO_PROGRAM".to_string()])
    );
}

// spec: MEDULLA-DECISION-ENGINE-R135.4
#[tokio::test]
async fn overlong_trace_is_refused() {
    let h = Harness::new().await;

    let long = commit_run(&h, trace("run-long", RunOutcome::Succeeded, 41, "weekly"))
        .await
        .unwrap_err();
    assert!(long.contains("PROGRAM_TOO_LONG"), "{long}");
    assert!(long.contains("41 steps"), "{long}");
    assert_eq!(stored_rows(&h, TENANT), 0);

    for outcome in [RunOutcome::Failed, RunOutcome::Cancelled] {
        let failed = commit_run(&h, trace("run-failed", outcome, 2, "weekly"))
            .await
            .unwrap_err();
        assert!(failed.contains("RUN_NOT_SUCCESSFUL"), "{failed}");
        assert_eq!(stored_rows(&h, TENANT), 0);
    }

    // A lookup with no stored program abstains with NO_PROGRAM and stores nothing.
    let batch = decide(&h, lookup_request(&h, Some("unknown")))
        .await
        .unwrap();
    assert_eq!(
        answered(&batch.records.as_slice()[0]),
        Err(vec!["NO_PROGRAM".to_string()])
    );
    assert_eq!(stored_rows(&h, TENANT), 0);

    // A program stored for another tenant never answers this tenant's lookup.
    let ok = commit_run(&h, trace("run-ok", RunOutcome::Succeeded, 2, "weekly"))
        .await
        .unwrap();
    let foreign_key = program_key("foreign-only");
    h.store
        .put_decision_artifacts(
            FOREIGN,
            &[(foreign_key, encode_artifact(&program_of(&ok)).unwrap())],
        )
        .unwrap();
    let batch = decide(&h, lookup_request(&h, Some("foreign-only")))
        .await
        .unwrap();
    assert_eq!(
        answered(&batch.records.as_slice()[0]),
        Err(vec!["NO_PROGRAM".to_string()])
    );

    // The lookup requires a signature.
    let missing = decide(&h, lookup_request(&h, None)).await.unwrap_err();
    assert!(missing.starts_with("PARAMETER_INVALID"), "{missing}");
}

/// The pre-plan receipt tests (MEDULLA-DECISION-ENGINE-R135.7) need the engine's Blob CAS.
#[cfg(feature = "blob")]
mod preplan {
    use super::*;
    use medulla_decision_types::decision::program_store::preplan::{
        pattern_key, PlanBinding, PrePlanTestReceipt, ReusablePattern,
    };
    use medulla_server_blob::blob::engine_bodies::EngineBody;
    use medulla_server_blob::blob::BlobStateExt;
    use sha2::{Digest, Sha256};

    fn hex64(c: char) -> String {
        std::iter::repeat(c).take(64).collect()
    }

    /// Store `body` in `tenant`'s Blob CAS, as the harness does through the
    /// blob-put port before the commit; the manifest digest it answers.
    async fn put_manifest(h: &Harness, tenant: &str, body: &[u8]) -> String {
        let cursors = h.state.read().await.blob_cursors().cloned().unwrap();
        let sha256 = medulla_types::contract::Digest256::from_bytes(Sha256::digest(body).into());
        let stored = cursors
            .store
            .put_engine_bodies(
                tenant,
                &[EngineBody {
                    sha256,
                    body: body.to_vec(),
                }],
                5,
            )
            .unwrap();
        stored[0].manifest_digest.clone()
    }

    fn binding() -> PlanBinding {
        PlanBinding {
            repository_revision: "rev-1".into(),
            command_digest: hex64('a'),
            planned_at_ms: 10_000,
        }
    }

    fn receipt(manifest: &str) -> PrePlanTestReceipt {
        PrePlanTestReceipt {
            tenant_id: TENANT.into(),
            run_id: "run-1".into(),
            repository_id: "repo".into(),
            repository_revision: "rev-1".into(),
            command_id: "cargo-test".into(),
            command_digest: hex64('a'),
            started_at_ms: 1_000,
            completed_at_ms: 2_000,
            exit_code: 0,
            artifact_manifest_digest: manifest.into(),
            artifact_count: 2,
            result_digest: hex64('c'),
        }
    }

    async fn promote(
        h: &Harness,
        binding: Option<PlanBinding>,
        receipt: Option<PrePlanTestReceipt>,
    ) -> Result<RunRecordStored, String> {
        let op = DecisionLogOp::RunRecord {
            tenant_id: TENANT.to_string(),
            record: RunRecord::CompiledProgram {
                trace: trace("run-1", RunOutcome::Succeeded, 3, "weekly"),
                policy: DecisionPolicyRef::Default,
                reusable: true,
                plan_binding: binding,
                preplan_receipt: receipt,
            },
        };
        decode(super::super::log_tests::log_op(h, "recorder", op).await)
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.7
    #[tokio::test]
    async fn reusable_pattern_requires_a_verified_preplan_receipt() {
        let h = Harness::new().await;
        let manifest = put_manifest(&h, TENANT, b"junit.xml\ncoverage.json").await;
        let foreign_manifest = put_manifest(&h, FOREIGN, b"other tenant artifacts").await;
        let ok = receipt(&manifest);

        // Each blocking case refuses with its code and leaves the library empty.
        let edit = |f: &dyn Fn(&mut PrePlanTestReceipt)| {
            let mut r = ok.clone();
            f(&mut r);
            Some(r)
        };
        let cases: Vec<(&str, Option<PlanBinding>, Option<PrePlanTestReceipt>)> = vec![
            ("PREPLAN_RECEIPT_MISSING", Some(binding()), None),
            ("PREPLAN_RECEIPT_MISSING", None, Some(ok.clone())),
            (
                "PREPLAN_COMMAND_MISMATCH",
                Some(binding()),
                edit(&|r| r.command_digest = hex64('d')),
            ),
            (
                "PREPLAN_COMMAND_MISMATCH",
                Some(binding()),
                edit(&|r| r.repository_revision = "rev-2".into()),
            ),
            (
                "PREPLAN_RECEIPT_STALE",
                Some(binding()),
                edit(&|r| r.completed_at_ms = 10_000),
            ),
            (
                "PREPLAN_RECEIPT_STALE",
                Some(PlanBinding {
                    planned_at_ms: 2_000 + 24 * 3_600_000 + 1,
                    ..binding()
                }),
                Some(ok.clone()),
            ),
            (
                "PREPLAN_TESTS_FAILED",
                Some(binding()),
                edit(&|r| r.exit_code = 2),
            ),
            (
                "PREPLAN_ARTIFACTS_EMPTY",
                Some(binding()),
                edit(&|r| r.artifact_count = 0),
            ),
            (
                "PREPLAN_ARTIFACTS_UNREADABLE",
                Some(binding()),
                edit(&|r| r.artifact_manifest_digest = hex64('e')),
            ),
            // Another tenant's manifest reads as unreadable here.
            (
                "PREPLAN_ARTIFACTS_UNREADABLE",
                Some(binding()),
                edit(&|r| r.artifact_manifest_digest = foreign_manifest.clone()),
            ),
        ];
        for (code, bind, rec) in cases {
            let refused = promote(&h, bind, rec).await.unwrap_err();
            assert!(refused.starts_with(code), "{code}: {refused}");
            assert_eq!(stored_rows(&h, TENANT), 0, "{code} stored nothing");
        }

        // An empty manifest body is the empty-manifest refusal.
        let empty = put_manifest(&h, TENANT, b"").await;
        let refused = promote(&h, Some(binding()), Some(receipt(&empty)))
            .await
            .unwrap_err();
        assert!(refused.starts_with("PREPLAN_ARTIFACTS_EMPTY"), "{refused}");
        assert_eq!(stored_rows(&h, TENANT), 0);

        // Valid evidence promotes the pattern.
        let stored = promote(&h, Some(binding()), Some(ok.clone()))
            .await
            .unwrap();
        let StoredRun::CompiledProgram { program, pattern } = &stored.stored else {
            panic!("a program record");
        };
        let pattern = pattern.clone().expect("the pattern was promoted");
        assert!(pattern.verdict.tests_ran && pattern.verdict.artifacts_exist);
        assert_eq!(pattern.receipt, ok);
        assert_eq!(pattern.plan, binding());
        assert_eq!(pattern.program_digest, program.program_digest);

        // Independent readback of receipt, pattern and the source trace refs.
        let read: ReusablePattern = decode_artifact_of(&h, &pattern_key(SIGNATURE));
        assert_eq!(read, pattern);
        assert_eq!(read.receipt.run_id, read.source_run_id);
        assert_eq!(read.verdict.policy_digest.len(), 64);
        assert_eq!(read.verdict.evidence_refs.len(), 3);
        assert_eq!(stored_rows(&h, TENANT), 2, "the program and its pattern");

        // The pattern is indexed through ProgramLookup.
        let batch = decide(&h, lookup_request(&h, Some(SIGNATURE)))
            .await
            .unwrap();
        let record = &batch.records.as_slice()[0];
        assert_eq!(answered(record), Ok(program.clone()));
        for fact in [
            "program.reusable=true".to_string(),
            format!("program.preplan_manifest_digest={manifest}"),
            format!("program.preplan_result_digest={}", hex64('c')),
        ] {
            let found = record.premises.iter().find(|p| p.fact == fact);
            assert_eq!(found.map(|p| p.class), Some(PremiseClass::Claim), "{fact}");
        }

        // A retry with equal content answers the stored rows.
        let again = promote(&h, Some(binding()), Some(ok)).await.unwrap();
        assert!(again.replayed);
        assert_eq!(stored_rows(&h, TENANT), 2);
    }

    fn decode_artifact_of<T: serde::de::DeserializeOwned>(h: &Harness, key: &str) -> T {
        let bytes = h.store.decision_artifact(TENANT, key).unwrap().unwrap();
        medulla_persistence_records::decision_jobs::decode_artifact(&bytes, "pattern").unwrap()
    }
}
