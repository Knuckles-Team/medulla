//! The trajectory decisions of a task plan through the actual Decide handler
//! (MEDULLA-DECISION-ENGINE-R140.104): digests, resolution kind, premises,
//! every refusal code of the four decisions, and tenant isolation of the
//! outcome records a decision reads.

use super::task_plan_tests::task_request;
use super::*;
use medulla_decision_types::decision::statistical::log::{EntryInputs, RecordVisibility};
use medulla_persistence_records::decision_jobs::{encode_artifact, evaluation_key, record_key};
use {
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::scorer_digest,
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::BudgetBranch,
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::ComplexityFeature,
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::ComplexityScorer,
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::FeatureWeight,
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::TrajectoryBudgetRequest,
};
use {
    medulla_decision_policy_types::decision::task_plan::trajectory_projection::ProjectionPin,
    medulla_decision_policy_types::decision::task_plan::trajectory_projection::ProjectionRequest,
    medulla_decision_policy_types::decision::task_plan::trajectory_projection::SourceTrajectory,
    medulla_decision_policy_types::decision::task_plan::trajectory_projection::TokenRange,
};
use {
    medulla_decision_program_types::decision::task_plan::branch_breadth::BranchHistory,
    medulla_decision_program_types::decision::task_plan::branch_breadth::BreadthRequest,
    medulla_decision_program_types::decision::task_plan::branch_breadth::OutcomeRef,
    medulla_decision_program_types::decision::task_plan::branch_breadth::ResultClass,
};
use {
    medulla_decision_program_types::decision::task_plan::pairwise::PairwiseCandidate,
    medulla_decision_program_types::decision::task_plan::pairwise::PairwisePolicyPin,
    medulla_decision_program_types::decision::task_plan::pairwise::PairwiseRecord,
    medulla_decision_program_types::decision::task_plan::pairwise::PairwiseRequest,
    medulla_decision_program_types::decision::task_plan::pairwise::Verdict,
    medulla_decision_program_types::decision::task_plan::pairwise::PAIRWISE_POLICY_ID,
    medulla_decision_program_types::decision::task_plan::pairwise::PAIRWISE_POLICY_REVISION,
};
use {
    medulla_decision_program_types::decision::task_plan::trajectory::TrajectoryAnswer,
    medulla_decision_program_types::decision::task_plan::trajectory::TrajectoryQuery,
    medulla_decision_program_types::decision::task_plan::trajectory::TRAJECTORY_PARAM,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_types::decision::record::ResolutionKind,
};

const OTHER_TENANT: &str = "tenant-trajectory-other";

fn now_ms() -> u64 {
    medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
}

fn with_query(pin: &ComponentDependency, query: TrajectoryQuery) -> DecideRequest {
    let mut request = task_request(pin);
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: TRAJECTORY_PARAM.to_string(),
        value: TypedValue::Trajectory(Box::new(query)),
    });
    request.params = BoundedVec::new(params).unwrap();
    request
}

/// Store `id` as a committed record of `tenant` with one outcome evaluation
/// per entry of `verdicts`, under the record digest `digest`.
fn write_outcome(
    h: &Harness,
    tenant: &str,
    template: &StatisticalDecisionRecord,
    id: &str,
    digest: &str,
    verdicts: &[Option<bool>],
    record_tenant: &str,
) {
    let mut record = template.clone();
    record.record_id = id.to_string();
    record.tenant_id = record_tenant.to_string();
    record.record_digest = digest.to_string();
    let entry = DecisionLogEntry {
        schema_version:
            medulla_decision_types::decision::statistical::log::DECISION_LOG_SCHEMA_VERSION,
        visibility: RecordVisibility::Tenant,
        committed_by: "decider".to_string(),
        committed_at_ms: record.created_at_ms,
        record: Box::new(record),
        inputs: EntryInputs::Inline,
        approval: None,
    };
    let mut rows = vec![(record_key(id), encode_artifact(&entry).unwrap())];
    for (n, success) in verdicts.iter().enumerate() {
        let evaluation_id = format!("evaluation-{n}");
        let stored = StoredEvaluation {
            evaluation: DecisionOutcomeEvaluation {
                record_id: id.to_string(),
                evaluation_id: evaluation_id.clone(),
                class: EvidenceClass::Observation,
                selected_agent: "agent-a".to_string(),
                lease_holder: "worker-a".to_string(),
                fidelity: OutcomeFidelity::ToolCalls,
                success: *success,
                backbone: None,
            },
            producer: "evaluator".to_string(),
            recorded_at_ms: 1,
        };
        rows.push((
            evaluation_key(id, &evaluation_id),
            encode_artifact(&stored).unwrap(),
        ));
    }
    h.store.put_decision_artifacts(tenant, &rows).unwrap();
}

fn scorer(weight: i64) -> ComplexityScorer {
    let mut scorer = ComplexityScorer {
        id: "loc-scorer".into(),
        revision: 1,
        digest: String::new(),
        weights: vec![FeatureWeight {
            feature_id: "loc".into(),
            weight,
        }],
    };
    scorer.digest = scorer_digest(&scorer);
    scorer
}

fn budget(value: i64, observed_at_ms: u64, requested_k: Option<u32>) -> TrajectoryBudgetRequest {
    TrajectoryBudgetRequest {
        task_digest: "sha256:task".into(),
        features: vec![ComplexityFeature {
            id: "loc".into(),
            value,
            unit: "lines".into(),
            source_ref: "repo:r".into(),
            source_revision: 3,
            source_digest: "sha256:feature".into(),
            observed_at_ms,
        }],
        scorer: Some(scorer(1_000_000)),
        requested_k,
    }
}

fn source(run: &str, digest: &str, revision: u64) -> SourceTrajectory {
    SourceTrajectory {
        run_id: run.into(),
        candidate_id: format!("cand-{run}"),
        source_revision: revision,
        trace_digest: digest.into(),
        trace_token_len: 100,
        removed: vec![TokenRange { start: 0, end: 10 }],
        retained: vec![TokenRange { start: 10, end: 50 }],
    }
}

fn projection(sources: Vec<SourceTrajectory>) -> ProjectionRequest {
    let pin = Some(ProjectionPin {
        id: "redact".into(),
        revision: 1,
    });
    ProjectionRequest {
        sources,
        redaction_policy: pin.clone(),
        tokenizer: pin,
        shuffle_seed: 7,
    }
}

fn breadth(refs: Vec<OutcomeRef>) -> BreadthRequest {
    BreadthRequest {
        search_id: "search".into(),
        branch_id: "branch".into(),
        ordinal: 1,
        plan_digest: "sha256:plan".into(),
        history: BranchHistory::Present {
            previous_breadth: 2,
            refs,
        },
    }
}

fn cited(id: &str, digest: &str, revision: u64, class: ResultClass) -> OutcomeRef {
    OutcomeRef {
        branch_id: id.into(),
        outcome_revision: revision,
        outcome_digest: digest.into(),
        result_class: class,
    }
}

fn candidates() -> Vec<PairwiseCandidate> {
    ["a", "b", "c"]
        .iter()
        .map(|id| PairwiseCandidate {
            id: (*id).into(),
            digest: format!("sha256:{id}"),
        })
        .collect()
}

fn judged(a: &str, b: &str, verdict: Verdict) -> PairwiseRecord {
    PairwiseRecord {
        candidate_a: a.into(),
        digest_a: format!("sha256:{a}"),
        candidate_b: b.into(),
        digest_b: format!("sha256:{b}"),
        judge_revision: "judge-1".into(),
        verdict,
        evidence_digest: format!("sha256:ev-{a}{b}"),
    }
}

fn pairwise(records: Vec<PairwiseRecord>, pinned: bool) -> PairwiseRequest {
    PairwiseRequest {
        candidates: candidates(),
        records,
        policy: pinned.then(|| PairwisePolicyPin {
            id: PAIRWISE_POLICY_ID.into(),
            revision: PAIRWISE_POLICY_REVISION,
        }),
    }
}

/// The registered code a refused trajectory decision leaves with; the refusal text
/// is `CODE: detail`, and the test proves the code.
async fn refused(h: &Harness, pin: &ComponentDependency, query: TrajectoryQuery) -> String {
    let text = decide(h, with_query(pin, query)).await.unwrap_err();
    text.split(": ").next().unwrap_or_default().to_string()
}

// spec: MEDULLA-DECISION-ENGINE-R140.104
#[tokio::test]
async fn decide_returns_pinned_trajectory_decisions() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);

    // The plan without a trajectory is the reference: the plan digest and the
    // plan body do not move when a decision rides on it.
    let plain = decide(&h, task_request(&pin)).await.unwrap();
    let plain_record = &plain.records.as_slice()[0];
    let plain_plan = plain_record.task_plan().unwrap().clone();
    assert!(plain_plan.trajectory.is_none());

    // Budget: a complex branch, the largest permitted K.
    let batch = decide(
        &h,
        with_query(
            &pin,
            TrajectoryQuery::Budget(budget(2_000_000, now_ms(), None)),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.task_plan().unwrap();
    assert_eq!(plan.plan_digest, plain_plan.plan_digest);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let decision = plan.trajectory.as_ref().expect("a trajectory decision");
    let TrajectoryAnswer::Budget(answer) = &decision.answer else {
        panic!("a budget answer");
    };
    assert_eq!(answer.branch, BudgetBranch::Complex);
    assert_eq!((answer.k_min, answer.k_max, answer.k), (2, 8, 8));
    assert_eq!(answer.score_micro, 2_000_000);
    assert_eq!(decision.decision_digest, answer.decision_digest);
    assert_eq!(decision.policy_digest, record.inputs.policy_digest);
    assert_eq!(decision.source_digests, {
        let mut expected = vec![scorer(1_000_000).digest, "sha256:feature".to_string()];
        expected.sort();
        expected
    });
    for subject in [
        "trajectory:task",
        "trajectory:scorer",
        "trajectory:feature:loc",
        "trajectory:score",
    ] {
        assert!(
            record
                .premises
                .iter()
                .any(|p| p.subject == subject && p.class == PremiseClass::Claim),
            "premise {subject}"
        );
    }
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    // A simple branch returns K = 1.
    let batch = decide(
        &h,
        with_query(&pin, TrajectoryQuery::Budget(budget(10, now_ms(), None))),
    )
    .await
    .unwrap();
    let TrajectoryAnswer::Budget(simple) = &batch.records.as_slice()[0]
        .task_plan()
        .unwrap()
        .trajectory
        .as_ref()
        .unwrap()
        .answer
    else {
        panic!("a budget answer");
    };
    assert_eq!((simple.branch, simple.k), (BudgetBranch::Simple, 1));

    // Budget refusals.
    let fresh = now_ms();
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Budget(budget(2_000_000, 1, None))
        )
        .await,
        "TRAJECTORY_FEATURE_STALE"
    );
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Budget(budget(2_000_000, fresh, Some(99)))
        )
        .await,
        "TRAJECTORY_K_INVALID"
    );
    let mut unpinned = budget(2_000_000, fresh, None);
    unpinned.scorer.as_mut().unwrap().digest = "sha256:forged".into();
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Budget(unpinned)).await,
        "TRAJECTORY_SCORER_UNPINNED"
    );
    let mut absent = budget(2_000_000, fresh, None);
    absent.scorer = None;
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Budget(absent)).await,
        "TRAJECTORY_POLICY_ABSENT"
    );
    let mut missing = budget(2_000_000, fresh, None);
    missing.features.clear();
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Budget(missing)).await,
        "TRAJECTORY_FEATURE_MISSING"
    );

    // Outcome records the projection and the breadth read.
    let template = plain_record.clone();
    write_outcome(
        &h,
        TENANT,
        &template,
        "run-a",
        "sha256:trace-a",
        &[Some(true)],
        TENANT,
    );
    write_outcome(
        &h,
        TENANT,
        &template,
        "run-b",
        "sha256:trace-b",
        &[Some(true), Some(false)],
        TENANT,
    );
    write_outcome(
        &h,
        TENANT,
        &template,
        "run-fail",
        "sha256:trace-f",
        &[Some(false)],
        TENANT,
    );
    write_outcome(
        &h,
        TENANT,
        &template,
        "run-open",
        "sha256:trace-o",
        &[],
        TENANT,
    );
    write_outcome(
        &h,
        TENANT,
        &template,
        "run-moved",
        "sha256:trace-m",
        &[Some(true)],
        OTHER_TENANT,
    );
    write_outcome(
        &h,
        OTHER_TENANT,
        &template,
        "run-foreign",
        "sha256:trace-x",
        &[Some(true)],
        OTHER_TENANT,
    );

    // Projection: parents bound to the stored revision and digest.
    let batch = decide(
        &h,
        with_query(
            &pin,
            TrajectoryQuery::Projection(projection(vec![
                source("run-a", "sha256:trace-a", 1),
                source("run-b", "sha256:trace-b", 2),
            ])),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let decision = record.task_plan().unwrap().trajectory.as_ref().unwrap();
    let TrajectoryAnswer::Projection(plan) = &decision.answer else {
        panic!("a projection answer");
    };
    let mut order = plan.order.clone();
    order.sort();
    assert_eq!(order, ["run-a", "run-b"]);
    assert_eq!(plan.new_candidate_id.len(), 64);
    assert_eq!(plan.parents.len(), 2);
    assert_eq!(decision.decision_digest, plan.decision_digest);
    assert_eq!(
        decision.source_digests,
        ["sha256:trace-a", "sha256:trace-b"]
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    for run in ["run-a", "run-b"] {
        assert!(record
            .premises
            .iter()
            .any(|p| p.subject == format!("projection:source:{run}")
                && p.class == PremiseClass::Claim));
    }
    // Projection refusals.
    let unknown = projection(vec![source("run-nobody", "sha256:t", 1)]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Projection(unknown)).await,
        "PROJECTION_SOURCE_UNKNOWN"
    );
    let foreign = projection(vec![source("run-foreign", "sha256:trace-x", 1)]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Projection(foreign)).await,
        "PROJECTION_SOURCE_UNKNOWN",
        "another tenant's outcome record is invisible"
    );
    let stale = projection(vec![source("run-a", "sha256:trace-a", 5)]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Projection(stale)).await,
        "PROJECTION_SOURCE_STALE"
    );
    let mut wrong_digest = projection(vec![source("run-a", "sha256:other", 1)]);
    wrong_digest.shuffle_seed = 1;
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Projection(wrong_digest)).await,
        "PROJECTION_SOURCE_STALE"
    );
    let mut out_of_range = source("run-a", "sha256:trace-a", 1);
    out_of_range.retained = vec![TokenRange {
        start: 90,
        end: 120,
    }];
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Projection(projection(vec![out_of_range]))
        )
        .await,
        "PROJECTION_RANGE_INVALID"
    );
    let mut overlapping = source("run-a", "sha256:trace-a", 1);
    overlapping.retained = vec![TokenRange { start: 5, end: 20 }];
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Projection(projection(vec![overlapping]))
        )
        .await,
        "PROJECTION_RANGE_OVERLAP"
    );
    let mut no_policy = projection(vec![source("run-a", "sha256:trace-a", 1)]);
    no_policy.tokenizer = None;
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Projection(no_policy)).await,
        "PROJECTION_POLICY_UNPINNED"
    );
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Projection(projection(Vec::new()))
        )
        .await,
        "PROJECTION_PARENT_INCOMPLETE"
    );

    // Breadth: an absent history starts at the knob; a present one moves by
    // the class of the stored outcome (a success lowers it by one).
    let absent = BreadthRequest {
        history: BranchHistory::Absent,
        ..breadth(Vec::new())
    };
    let batch = decide(&h, with_query(&pin, TrajectoryQuery::Breadth(absent)))
        .await
        .unwrap();
    let TrajectoryAnswer::Breadth(initial) = &batch.records.as_slice()[0]
        .task_plan()
        .unwrap()
        .trajectory
        .as_ref()
        .unwrap()
        .answer
    else {
        panic!("a breadth answer");
    };
    assert_eq!((initial.previous_breadth, initial.selected_breadth), (0, 2));
    let batch = decide(
        &h,
        with_query(
            &pin,
            TrajectoryQuery::Breadth(breadth(vec![cited(
                "run-a",
                "sha256:trace-a",
                1,
                ResultClass::Success,
            )])),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let decision = record.task_plan().unwrap().trajectory.as_ref().unwrap();
    let TrajectoryAnswer::Breadth(moved) = &decision.answer else {
        panic!("a breadth answer");
    };
    assert_eq!((moved.previous_breadth, moved.selected_breadth), (2, 1));
    assert_eq!(decision.source_digests, ["sha256:trace-a"]);
    assert_eq!(decision.decision_digest, moved.decision_digest);
    for subject in ["breadth:history", "breadth:previous", "breadth:selected"] {
        assert!(
            record
                .premises
                .iter()
                .any(|p| p.subject == subject && p.class == PremiseClass::Observation),
            "premise {subject}"
        );
    }
    // A mixed record is partial and a failed one raises the breadth.
    let batch = decide(
        &h,
        with_query(
            &pin,
            TrajectoryQuery::Breadth(breadth(vec![
                cited("run-b", "sha256:trace-b", 2, ResultClass::Partial),
                cited("run-fail", "sha256:trace-f", 1, ResultClass::Failure),
            ])),
        ),
    )
    .await
    .unwrap();
    let TrajectoryAnswer::Breadth(raised) = &batch.records.as_slice()[0]
        .task_plan()
        .unwrap()
        .trajectory
        .as_ref()
        .unwrap()
        .answer
    else {
        panic!("a breadth answer");
    };
    assert_eq!(raised.selected_breadth, 3);
    // Breadth refusals.
    let stale = breadth(vec![cited(
        "run-a",
        "sha256:trace-a",
        1,
        ResultClass::Failure,
    )]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Breadth(stale)).await,
        "BREADTH_HISTORY_STALE"
    );
    let moved_digest = breadth(vec![cited(
        "run-a",
        "sha256:elsewhere",
        1,
        ResultClass::Success,
    )]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Breadth(moved_digest)).await,
        "BREADTH_HISTORY_STALE"
    );
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Breadth(breadth(Vec::new()))).await,
        "BREADTH_HISTORY_INCOMPLETE"
    );
    for missing in ["run-nobody", "run-open"] {
        let request = breadth(vec![cited(missing, "sha256:t", 1, ResultClass::Success)]);
        assert_eq!(
            refused(&h, &pin, TrajectoryQuery::Breadth(request)).await,
            "BREADTH_HISTORY_INCOMPLETE",
            "{missing} has no stored outcome"
        );
    }
    let foreign = breadth(vec![cited(
        "run-foreign",
        "sha256:trace-x",
        1,
        ResultClass::Success,
    )]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Breadth(foreign)).await,
        "BREADTH_HISTORY_INCOMPLETE",
        "another tenant's outcome is invisible, never confirmed to exist"
    );
    let moved_tenant = breadth(vec![cited(
        "run-moved",
        "sha256:trace-m",
        1,
        ResultClass::Success,
    )]);
    assert_eq!(
        refused(&h, &pin, TrajectoryQuery::Breadth(moved_tenant)).await,
        "BREADTH_HISTORY_FORBIDDEN",
        "a visible row that belongs to another tenant is refused"
    );

    // Pairwise: a full round-robin ranks by wins minus losses.
    let round = vec![
        judged("a", "b", Verdict::WinA),
        judged("a", "c", Verdict::WinA),
        judged("b", "c", Verdict::WinB),
    ];
    let batch = decide(
        &h,
        with_query(
            &pin,
            TrajectoryQuery::Pairwise(pairwise(round.clone(), true)),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let decision = record.task_plan().unwrap().trajectory.as_ref().unwrap();
    let TrajectoryAnswer::Pairwise(ranking) = &decision.answer else {
        panic!("a pairwise answer");
    };
    let ids: Vec<&str> = ranking.ranked.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "c", "b"]);
    assert_eq!(ranking.kept, ["a", "c", "b"]);
    assert_eq!(decision.decision_digest, ranking.decision_digest);
    assert!(decision
        .source_digests
        .contains(&"sha256:ev-ab".to_string()));
    for subject in [
        "pairwise:record:a:b",
        "pairwise:record:a:c",
        "pairwise:record:b:c",
        "pairwise:score:a",
        "pairwise:score:b",
        "pairwise:score:c",
    ] {
        assert!(
            record
                .premises
                .iter()
                .any(|p| p.subject == subject && p.class == PremiseClass::Claim),
            "premise {subject}"
        );
    }
    // Pairwise refusals.
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Pairwise(pairwise(round.clone(), false))
        )
        .await,
        "PAIRWISE_POLICY_UNPINNED"
    );
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Pairwise(pairwise(round[..2].to_vec(), true))
        )
        .await,
        "PAIRWISE_PAIR_MISSING"
    );
    let mut conflicting = round.clone();
    conflicting.push(judged("b", "c", Verdict::WinA));
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Pairwise(pairwise(conflicting, true))
        )
        .await,
        "PAIRWISE_PAIR_CONFLICT"
    );
    let mut mismatched = round.clone();
    mismatched[0].digest_a = "sha256:changed".into();
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Pairwise(pairwise(mismatched, true))
        )
        .await,
        "PAIRWISE_DIGEST_MISMATCH"
    );

    // A second parameter that is not a trajectory query is refused.
    let mut request = task_request(&pin);
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: TRAJECTORY_PARAM.to_string(),
        value: TypedValue::Text("budget".into()),
    });
    request.params = BoundedVec::new(params).unwrap();
    let refused = decide(&h, request).await.unwrap_err();
    assert_eq!(refused.split(": ").next(), Some("PARAMETER_INVALID"));
}

/// Decide `query` on the plan and return the sealed record, checking what every
/// decision shares: the record digest recomputes, the decision digest is the
/// answer's own, the policy digest is the record's and the kind is a constraint
/// over claims; the same request decided again seals the same digests.
async fn sealed(
    h: &Harness,
    pin: &ComponentDependency,
    query: TrajectoryQuery,
    answer_digest: impl Fn(&TrajectoryAnswer) -> String,
) -> StatisticalDecisionRecord {
    let first = decide(h, with_query(pin, query.clone())).await.unwrap();
    let record = first.records.as_slice()[0].clone();
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(&record),
        record.record_digest
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let decision = record.task_plan().unwrap().trajectory.as_ref().unwrap();
    assert_eq!(decision.decision_digest, answer_digest(&decision.answer));
    assert_eq!(decision.policy_digest, record.inputs.policy_digest);
    let again = decide(h, with_query(pin, query)).await.unwrap();
    let replay = &again.records.as_slice()[0];
    assert_eq!(
        replay
            .task_plan()
            .unwrap()
            .trajectory
            .as_ref()
            .unwrap()
            .decision_digest,
        decision.decision_digest,
        "the same inputs seal the same decision"
    );
    record
}

/// Verifies MEDULLA-DECISION-ENGINE-R140.105: the budget, projection, breadth
/// and pairwise decisions run in sequence through `Decide` with tenant and
/// principal context; each returned decision is read back and checked (record
/// digest, decision digest, `ResolutionKind`, `EvidenceClass`, replay); then
/// every refusal code is submitted and asserted, with a cross-tenant negative.
// spec: MEDULLA-DECISION-ENGINE-R140.105
#[tokio::test]
async fn trajectory_decisions_journey() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let template = decide(&h, task_request(&pin))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    for (id, digest, verdicts, tenant) in [
        ("run-a", "sha256:trace-a", vec![Some(true)], TENANT),
        (
            "run-b",
            "sha256:trace-b",
            vec![Some(true), Some(false)],
            TENANT,
        ),
        ("run-fail", "sha256:trace-f", vec![Some(false)], TENANT),
        ("run-open", "sha256:trace-o", vec![], TENANT),
        (
            "run-moved",
            "sha256:trace-m",
            vec![Some(true)],
            OTHER_TENANT,
        ),
    ] {
        write_outcome(&h, TENANT, &template, id, digest, &verdicts, tenant);
    }
    write_outcome(
        &h,
        OTHER_TENANT,
        &template,
        "run-foreign",
        "sha256:trace-x",
        &[Some(true)],
        OTHER_TENANT,
    );

    // Budget, then projection, breadth and pairwise, each sealed and replayed.
    let record = sealed(
        &h,
        &pin,
        TrajectoryQuery::Budget(budget(2_000_000, now_ms(), None)),
        |a| match a {
            TrajectoryAnswer::Budget(b) => b.decision_digest.clone(),
            other => panic!("a budget answer, got {other:?}"),
        },
    )
    .await;
    assert!(record
        .premises
        .iter()
        .any(|p| p.subject == "trajectory:score"));
    let record = sealed(
        &h,
        &pin,
        TrajectoryQuery::Projection(projection(vec![
            source("run-a", "sha256:trace-a", 1),
            source("run-b", "sha256:trace-b", 2),
        ])),
        |a| match a {
            TrajectoryAnswer::Projection(p) => p.decision_digest.clone(),
            other => panic!("a projection answer, got {other:?}"),
        },
    )
    .await;
    assert!(record
        .premises
        .iter()
        .any(|p| p.subject == "projection:source:run-a"));
    let record = sealed(
        &h,
        &pin,
        TrajectoryQuery::Breadth(breadth(vec![cited(
            "run-a",
            "sha256:trace-a",
            1,
            ResultClass::Success,
        )])),
        |a| match a {
            TrajectoryAnswer::Breadth(b) => b.decision_digest.clone(),
            other => panic!("a breadth answer, got {other:?}"),
        },
    )
    .await;
    assert!(record
        .premises
        .iter()
        .any(|p| p.subject == "breadth:selected"));
    let round = vec![
        judged("a", "b", Verdict::WinA),
        judged("a", "c", Verdict::WinA),
        judged("b", "c", Verdict::WinB),
    ];
    let record = sealed(
        &h,
        &pin,
        TrajectoryQuery::Pairwise(pairwise(round.clone(), true)),
        |a| match a {
            TrajectoryAnswer::Pairwise(p) => p.decision_digest.clone(),
            other => panic!("a pairwise answer, got {other:?}"),
        },
    )
    .await;
    assert!(record
        .premises
        .iter()
        .any(|p| p.subject == "pairwise:score:a"));

    // Every refusal code, one case each.
    let fresh = now_ms();
    let mut unpinned = budget(2_000_000, fresh, None);
    unpinned.scorer.as_mut().unwrap().digest = "sha256:forged".into();
    let mut absent = budget(2_000_000, fresh, None);
    absent.scorer = None;
    let mut missing = budget(2_000_000, fresh, None);
    missing.features.clear();
    let mut wrong_digest = projection(vec![source("run-a", "sha256:other", 1)]);
    wrong_digest.shuffle_seed = 1;
    let mut out_of_range = source("run-a", "sha256:trace-a", 1);
    out_of_range.retained = vec![TokenRange {
        start: 90,
        end: 120,
    }];
    let mut overlapping = source("run-a", "sha256:trace-a", 1);
    overlapping.retained = vec![TokenRange { start: 5, end: 20 }];
    let mut no_policy = projection(vec![source("run-a", "sha256:trace-a", 1)]);
    no_policy.tokenizer = None;
    let mut conflicting = round.clone();
    conflicting.push(judged("b", "c", Verdict::WinA));
    let mut mismatched = round.clone();
    mismatched[0].digest_a = "sha256:changed".into();
    let cases: Vec<(&str, TrajectoryQuery)> = vec![
        (
            "TRAJECTORY_FEATURE_STALE",
            TrajectoryQuery::Budget(budget(2_000_000, 1, None)),
        ),
        (
            "TRAJECTORY_K_INVALID",
            TrajectoryQuery::Budget(budget(2_000_000, fresh, Some(99))),
        ),
        (
            "TRAJECTORY_SCORER_UNPINNED",
            TrajectoryQuery::Budget(unpinned),
        ),
        ("TRAJECTORY_POLICY_ABSENT", TrajectoryQuery::Budget(absent)),
        (
            "TRAJECTORY_FEATURE_MISSING",
            TrajectoryQuery::Budget(missing),
        ),
        (
            "PROJECTION_SOURCE_UNKNOWN",
            TrajectoryQuery::Projection(projection(vec![source("run-nobody", "sha256:t", 1)])),
        ),
        (
            "PROJECTION_SOURCE_STALE",
            TrajectoryQuery::Projection(projection(vec![source("run-a", "sha256:trace-a", 5)])),
        ),
        (
            "PROJECTION_SOURCE_STALE",
            TrajectoryQuery::Projection(wrong_digest),
        ),
        (
            "PROJECTION_RANGE_INVALID",
            TrajectoryQuery::Projection(projection(vec![out_of_range])),
        ),
        (
            "PROJECTION_RANGE_OVERLAP",
            TrajectoryQuery::Projection(projection(vec![overlapping])),
        ),
        (
            "PROJECTION_POLICY_UNPINNED",
            TrajectoryQuery::Projection(no_policy),
        ),
        (
            "PROJECTION_PARENT_INCOMPLETE",
            TrajectoryQuery::Projection(projection(Vec::new())),
        ),
        (
            "BREADTH_HISTORY_STALE",
            TrajectoryQuery::Breadth(breadth(vec![cited(
                "run-a",
                "sha256:trace-a",
                1,
                ResultClass::Failure,
            )])),
        ),
        (
            "BREADTH_HISTORY_INCOMPLETE",
            TrajectoryQuery::Breadth(breadth(Vec::new())),
        ),
        (
            "BREADTH_HISTORY_FORBIDDEN",
            TrajectoryQuery::Breadth(breadth(vec![cited(
                "run-moved",
                "sha256:trace-m",
                1,
                ResultClass::Success,
            )])),
        ),
        (
            "PAIRWISE_POLICY_UNPINNED",
            TrajectoryQuery::Pairwise(pairwise(round.clone(), false)),
        ),
        (
            "PAIRWISE_PAIR_MISSING",
            TrajectoryQuery::Pairwise(pairwise(round[..2].to_vec(), true)),
        ),
        (
            "PAIRWISE_PAIR_CONFLICT",
            TrajectoryQuery::Pairwise(pairwise(conflicting, true)),
        ),
        (
            "PAIRWISE_DIGEST_MISMATCH",
            TrajectoryQuery::Pairwise(pairwise(mismatched, true)),
        ),
    ];
    for (code, query) in cases {
        assert_eq!(refused(&h, &pin, query).await, code);
    }

    // Cross-tenant negatives: another tenant's outcome record is invisible, so
    // the projection and breadth readers refuse as if it never existed.
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Projection(projection(vec![source(
                "run-foreign",
                "sha256:trace-x",
                1
            )]))
        )
        .await,
        "PROJECTION_SOURCE_UNKNOWN"
    );
    assert_eq!(
        refused(
            &h,
            &pin,
            TrajectoryQuery::Breadth(breadth(vec![cited(
                "run-foreign",
                "sha256:trace-x",
                1,
                ResultClass::Success
            )]))
        )
        .await,
        "BREADTH_HISTORY_INCOMPLETE"
    );
}
