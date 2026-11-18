//! Served walk-forward replay evaluation (EH-528): `DecisionEval` in replay
//! mode fits, replays and seals an `EvaluationRun`, and refuses what replay
//! cannot answer honestly.

use super::*;
use medulla_agent_types::agent_component::{
    AgentComponentKind, AgentComponentPublishRequest, ComponentDependency,
};
use medulla_decision_stats::decision::replay::cursor_of;
use medulla_decision_types::decision::replay::{
    AllocationRule, EvalMode, EvaluationRun, RefitSpec, ReplayEnvironment, ReplayEvent, ReplaySpec,
    SharedCap, TrialLog, WalkForward,
};
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::decision_jobs::{evaluation_run_key, replay_cursor_key};

const Q32_ONE: i64 = 1 << 32;

/// A fitted draft plus the gold set it was fitted on (items one ms apart).
struct Fitted {
    data: LabelledDataset,
    gold: String,
    draft: EvalCandidate,
}

fn optimiser() -> OptimiserSpec {
    OptimiserSpec {
        max_iterations: 30,
        tolerance: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: 1 << 12,
        },
        seed: 0,
    }
}

async fn fitted(h: &Harness) -> Fitted {
    for (id, summary) in [
        ("tool-a-search", "web search engine for pages"),
        ("tool-b-files", "write files to disk"),
        ("tool-c-mail", "send an email message"),
    ] {
        h.publish_test_component(id, AgentComponentKind::Tool, summary, None::<&()>, None)
            .unwrap();
    }
    let (schema_pin, schema_digest) = publish_route_schema(h);
    let batch = decide(
        h,
        request(
            &schema_pin,
            None,
            DecisionPolicyRef::Default,
            QuestionSafety::Ordinary,
        ),
    )
    .await
    .unwrap();
    let FeatureMatrixRef::Inline {
        candidate_ids,
        values,
        ..
    } = &batch.records.as_slice()[0].inputs.feature_matrix
    else {
        panic!("inline matrix")
    };
    let ids: Vec<String> = candidate_ids.iter().cloned().collect();
    let mut data = dataset(&schema_digest, &ids, values.as_slice(), 240);
    let mut items: Vec<LabelledItem> = data.items.iter().cloned().collect();
    for (i, item) in items.iter_mut().enumerate() {
        item.recorded_at_ms = 10_000 + i as u64;
        // `dataset` jitters every candidate row alike, which a linear logit's
        // softmax cannot see, so a confident head earns the same utility on
        // every item. Label noise -- one item in five accepts the second
        // option -- makes the replayed utility path vary, as real gold sets do.
        if i % 5 == 0 {
            item.label = ItemLabel::Gold {
                acceptable: BoundedVec::new(vec![ids[1].clone()]).unwrap(),
                source: LabelSource::SyntheticConstruction,
            };
        }
    }
    data.items = BoundedVec::new(items).unwrap();
    let gold = super::super::stat_jobs::dataset_digest(&data).unwrap();
    let fit = DecisionFitRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: "replay-fit".to_string(),
        head_kind: HeadKind::ListwiseLogistic,
        feature_schema: schema_pin,
        policy: DecisionPolicyRef::Default,
        label_regime: LabelRegime::FullLabel {
            gold_set_digest: gold.clone(),
        },
        window: window(),
        optimiser: optimiser(),
        source: DatasetSource::Inline {
            dataset: Box::new(data.clone()),
        },
        penalty: None,
        features: Default::default(),
        parent_head: None,
        calibration_method: None,
    };
    let op = DecisionFitOp::Submit {
        request: Box::new(fit),
    };
    let job: DecisionJobRecord =
        decode(super::super::jobs::handle_decision_fit(&h.state, 40, &verified(), op).await)
            .unwrap();
    let DecisionJobOutput::Fit {
        draft_sha256,
        draft_length,
        ..
    } = succeeded(&job).clone()
    else {
        panic!("fit output")
    };
    Fitted {
        data,
        gold,
        draft: EvalCandidate::DraftArtifact {
            sha256: draft_sha256,
            length: draft_length,
        },
    }
}

fn spec() -> ReplaySpec {
    ReplaySpec {
        folds: WalkForward {
            train: 100,
            test: 40,
            step: 40,
            purge: 5,
            embargo: 5,
        },
        budget: SharedCap {
            cap: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: Q32_ONE,
            },
            rule: AllocationRule::Proportional,
        },
        env: ReplayEnvironment::PolicyIndependent,
        refit: Some(RefitSpec {
            head_kind: HeadKind::ListwiseLogistic,
            optimiser: optimiser(),
        }),
        trials: TrialLog {
            declared: 3,
            searched: 3,
        },
        incumbent: None,
        supersedes: None,
        events: Default::default(),
        resume_cursor: None,
    }
}

fn replay_request(fitted: &Fitted, key: &str, spec: ReplaySpec) -> DecisionEvalRequest {
    DecisionEvalRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: key.to_string(),
        candidate: fitted.draft.clone(),
        policy: DecisionPolicyRef::Default,
        estimators: BoundedVec::new(Vec::new()).unwrap(),
        gold_set_digest: Some(fitted.gold.clone()),
        window: window(),
        source: DatasetSource::Inline {
            dataset: Box::new(fitted.data.clone()),
        },
        mode: EvalMode::Replay {
            spec: Box::new(spec),
        },
        gold_guarantee: None,
    }
}

async fn submit(h: &Harness, request: DecisionEvalRequest) -> DecisionJobRecord {
    let op = DecisionEvalOp::Submit {
        request: Box::new(request),
    };
    decode(super::super::jobs::handle_decision_eval(&h.state, 41, &verified(), op).await).unwrap()
}

fn sealed(job: &DecisionJobRecord) -> EvaluationRun {
    let DecisionJobOutput::Replay { run } = succeeded(job).clone() else {
        panic!("replay output")
    };
    *run
}

fn failed(job: &DecisionJobRecord) -> &str {
    match &job.state {
        DecisionJobState::Failed { code } => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R119
#[tokio::test]
async fn a_replay_is_sealed_stored_and_supersedable() {
    let h = Harness::new().await;
    let fitted = fitted(&h).await;
    let run = sealed(&submit(&h, replay_request(&fitted, "replay-1", spec())).await);

    // 240 items: tests start at 105 and step 40 -> 105..145, 145..185, 185..225.
    assert_eq!(run.folds.len(), 3);
    assert_eq!(run.path.len(), 120);
    assert!(run
        .folds
        .iter()
        .all(|fold| fold.first_test_ms < fold.last_test_ms));
    assert_eq!(run.folds.as_slice()[0].first_test_ms, 10_105);
    // Refits differ from the submitted draft (they see only their window).
    let EvalCandidate::DraftArtifact { sha256, .. } = &fitted.draft else {
        unreachable!()
    };
    assert_eq!(run.head_digest, *sha256);
    assert!(run.folds.iter().all(|fold| fold.head_digest != *sha256));
    assert_eq!(run.validation.n_trials, 3);
    // The utility is linear in the applied budget: contributions sum to the path.
    let path: i64 = run.path.iter().map(|u| u.value).sum();
    let credited: i64 = run.contributions.iter().map(|c| c.utility.value).sum();
    assert!((path - credited).abs() < 1 << 12, "{path} vs {credited}");
    // Each step's applied budget is capped at one.
    let applied: i64 = run.contributions.iter().map(|c| c.applied.value).sum();
    assert!(applied <= 120 * Q32_ONE + (1 << 12));

    // Content-addressed: the digest reproduces, and the row is stored under it.
    let mut unsealed = run.clone();
    unsealed.run_digest = String::new();
    assert_eq!(
        medulla_query_types::solve_scalar::digest_text("eg/decision/evaluation-run/v1", &unsealed),
        run.run_digest
    );
    assert!(h
        .store
        .decision_artifact(TENANT, &evaluation_run_key(&run.run_digest))
        .unwrap()
        .is_some());

    // A revision names the run it supersedes; an unknown one is refused.
    let mut revised = spec();
    revised.supersedes = Some(run.run_digest.clone());
    revised.trials.declared = 4;
    let next = sealed(&submit(&h, replay_request(&fitted, "replay-2", revised)).await);
    assert_eq!(
        next.spec.supersedes.as_deref(),
        Some(run.run_digest.as_str())
    );
    assert_ne!(next.run_digest, run.run_digest);
    let mut dangling = spec();
    dangling.supersedes = Some(format!("sha256:{}", "0".repeat(64)));
    let refused = submit(&h, replay_request(&fitted, "replay-3", dangling)).await;
    assert!(failed(&refused).starts_with("REPLAY_SPEC_INVALID"));
}

// spec: MEDULLA-DECISION-ENGINE-R119
#[tokio::test]
async fn replay_refuses_what_it_cannot_answer() {
    let h = Harness::new().await;
    let fitted = fitted(&h).await;

    let mut understated = spec();
    understated.trials = TrialLog {
        declared: 1,
        searched: 20,
    };
    let job = submit(&h, replay_request(&fitted, "understated", understated)).await;
    assert!(failed(&job).starts_with("TRIALS_UNDERSTATED"));

    let mut dependent = spec();
    dependent.env = ReplayEnvironment::PolicyDependent;
    let job = submit(&h, replay_request(&fitted, "dependent", dependent)).await;
    assert!(failed(&job).starts_with("REPLAY_POLICY_DEPENDENT"));

    // No gold pin: the bandit regime, whose outcomes depend on the executed option.
    let mut bandit = replay_request(&fitted, "bandit", spec());
    bandit.gold_set_digest = None;
    let job = submit(&h, bandit).await;
    assert!(failed(&job).starts_with("REPLAY_POLICY_DEPENDENT"));

    // Mutation: items 95..=110 share one instant, so fold 0's last training
    // items (95..100) were recorded no earlier than its first test item (105):
    // same-instant data leaks into training.
    let mut future = fitted.data.clone();
    let mut items: Vec<LabelledItem> = future.items.iter().cloned().collect();
    for item in &mut items[95..=110] {
        item.recorded_at_ms = 10_105;
    }
    future.items = BoundedVec::new(items).unwrap();
    let gold = super::super::stat_jobs::dataset_digest(&future).unwrap();
    let mut look_ahead = replay_request(&fitted, "look-ahead", spec());
    look_ahead.source = DatasetSource::Inline {
        dataset: Box::new(future),
    };
    look_ahead.gold_set_digest = Some(gold);
    let job = submit(&h, look_ahead).await;
    assert!(failed(&job).starts_with("LOOK_AHEAD"), "{}", failed(&job));
}

/// An instant after every component revision the test publishes.
const LATER_MS: u64 = u64::MAX / 4;

/// Six events of forty outcome references each over the fitted gold set.
fn event_stream(fitted: &Fitted, snapshot: &ComponentDependency, count: usize) -> Vec<ReplayEvent> {
    let ids: Vec<String> = fitted
        .data
        .items
        .iter()
        .map(|item| item.item_id.clone())
        .collect();
    (0..count)
        .map(|k| ReplayEvent {
            event_id: format!("ev-{k}"),
            seq: 10 + k as u64,
            event_time_ms: LATER_MS,
            snapshot: snapshot.clone(),
            action_id: "tool-a-search".to_string(),
            outcome_refs: BoundedVec::new(ids[k * 40..(k + 1) * 40].to_vec()).unwrap(),
            source_revision: "rev-1".to_string(),
        })
        .collect()
}

fn event_spec(events: Vec<ReplayEvent>, resume: Option<String>) -> ReplaySpec {
    let mut spec = spec();
    spec.events = BoundedVec::new(events).unwrap();
    spec.resume_cursor = resume;
    spec
}

fn republish(h: &Harness, id: &str, summary: &str, expected_revision: u64) -> ComponentDependency {
    let mut draft = test_component_draft(TENANT, id);
    draft.kind = AgentComponentKind::Tool;
    draft.summary = summary.to_string();
    draft.classification = vec!["eg:capability/retrieval/web-search".to_string()];
    let committed = h
        .store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(
                &h.store,
                TENANT,
                &format!("republish-{id}"),
                200,
                expected_revision,
                "agent-component:publish",
            ),
            component: draft,
            evaluation_receipt_digest: None,
        })
        .unwrap();
    ComponentDependency {
        component_id: id.to_string(),
        kind: AgentComponentKind::Tool,
        definition_digest: committed.result.component.definition_digest,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R088.2.2
#[tokio::test]
async fn event_replay_resumes_from_its_cursor_with_the_same_digest() {
    let h = Harness::new().await;
    let fitted = fitted(&h).await;
    let head = h
        .store
        .current_component(TENANT, "tool-a-search")
        .unwrap()
        .unwrap();
    let snapshot = ComponentDependency {
        component_id: head.component_id.clone(),
        kind: head.kind,
        definition_digest: head.definition_digest.clone(),
    };
    let run_events = |count| event_stream(&fitted, &snapshot, count);

    // The whole stream, replayed once: the receipt binds every event digest in
    // order, the cursor of the last event, the policy and the result digest.
    let whole = sealed(
        &submit(
            &h,
            replay_request(&fitted, "ev-whole", event_spec(run_events(6), None)),
        )
        .await,
    );
    let receipt = whole.event_receipt.clone().expect("event receipt");
    assert_eq!(receipt.event_digests.len(), 6);
    assert_eq!(whole.path.len(), 120);
    assert_eq!(
        receipt.cursor,
        cursor_of(run_events(6).last().unwrap()).unwrap()
    );
    assert_eq!(receipt.policy, DecisionPolicyRef::Default);
    assert!(receipt.result_digest.starts_with("sha256:"));

    // A shorter stream reaches its own cursor; the replay is stored under it.
    let prefix = sealed(
        &submit(
            &h,
            replay_request(&fitted, "ev-prefix", event_spec(run_events(5), None)),
        )
        .await,
    );
    let prefix_receipt = prefix.event_receipt.clone().expect("prefix receipt");
    assert_eq!(prefix_receipt.event_digests.len(), 5);
    assert_eq!(
        prefix_receipt.event_digests.as_slice(),
        &receipt.event_digests.as_slice()[..5]
    );
    assert!(h
        .store
        .decision_artifact(TENANT, &replay_cursor_key(&prefix_receipt.cursor))
        .unwrap()
        .is_some());

    // Resumed from that cursor over the whole stream: same events, cursor and
    // result digest as the whole replay.
    let resumed = sealed(
        &submit(
            &h,
            replay_request(
                &fitted,
                "ev-resume",
                event_spec(run_events(6), Some(prefix_receipt.cursor.clone())),
            ),
        )
        .await,
    );
    let resumed_receipt = resumed.event_receipt.clone().expect("resumed receipt");
    assert_eq!(resumed_receipt.event_digests, receipt.event_digests);
    assert_eq!(resumed_receipt.cursor, receipt.cursor);
    assert_eq!(resumed_receipt.result_digest, receipt.result_digest);
    assert_eq!(resumed.path, whole.path);
    assert_ne!(resumed.run_digest, whole.run_digest);

    // A cursor that names no event, or that no earlier replay reached.
    let unknown = format!("99:sha256:{}", "1".repeat(64));
    let job = submit(
        &h,
        replay_request(
            &fitted,
            "ev-unknown",
            event_spec(run_events(6), Some(unknown)),
        ),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_SPEC_INVALID"),
        "{}",
        failed(&job)
    );
    let unreached = cursor_of(&run_events(6)[2]).unwrap();
    let job = submit(
        &h,
        replay_request(
            &fitted,
            "ev-unreached",
            event_spec(run_events(6), Some(unreached)),
        ),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_SPEC_INVALID"),
        "{}",
        failed(&job)
    );
    let job = submit(
        &h,
        replay_request(
            &fitted,
            "ev-cursor-only",
            event_spec(Vec::new(), Some(prefix_receipt.cursor.clone())),
        ),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_SPEC_INVALID"),
        "{}",
        failed(&job)
    );

    // Ordering refusals, served.
    let mut gap = run_events(3);
    gap[2].seq = 13;
    let job = submit(&h, replay_request(&fitted, "ev-gap", event_spec(gap, None))).await;
    assert!(
        failed(&job).starts_with("REPLAY_SEQUENCE_GAP"),
        "{}",
        failed(&job)
    );
    let mut behind = run_events(2);
    behind.swap(0, 1);
    let job = submit(
        &h,
        replay_request(&fitted, "ev-behind", event_spec(behind, None)),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_OUT_OF_ORDER"),
        "{}",
        failed(&job)
    );
    let mut conflict = run_events(2);
    let mut repeated = conflict[0].clone();
    repeated.action_id = "tool-b-files".to_string();
    conflict.push(repeated);
    let job = submit(
        &h,
        replay_request(&fitted, "ev-conflict", event_spec(conflict, None)),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_DUPLICATE_CONFLICT"),
        "{}",
        failed(&job)
    );

    // An outcome reference that names no admitted item.
    let mut dangling = run_events(6);
    dangling[0].outcome_refs = BoundedVec::new(vec!["no-such-item".to_string()]).unwrap();
    let job = submit(
        &h,
        replay_request(&fitted, "ev-dangling", event_spec(dangling, None)),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_SPEC_INVALID"),
        "{}",
        failed(&job)
    );

    // A policy-dependent environment refuses; naming an off-policy estimator
    // asks for the off-policy evaluation instead.
    let mut dependent = event_spec(run_events(6), None);
    dependent.env = ReplayEnvironment::PolicyDependent;
    let job = submit(
        &h,
        replay_request(&fitted, "ev-dependent", dependent.clone()),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_POLICY_DEPENDENT"),
        "{}",
        failed(&job)
    );
    let mut offpolicy = replay_request(&fitted, "ev-offpolicy", dependent);
    offpolicy.estimators = BoundedVec::new(vec![OpeEstimatorKind::Ips]).unwrap();
    let job = submit(&h, offpolicy).await;
    assert!(matches!(succeeded(&job), DecisionJobOutput::Eval { .. }));

    // A snapshot that no longer is the revision the component held at the
    // event's time refuses; so does one with no revision at that time.
    republish(
        &h,
        "tool-a-search",
        "web search engine, revised",
        head.entry_revision,
    );
    let job = submit(
        &h,
        replay_request(&fitted, "ev-changed", event_spec(run_events(6), None)),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_SNAPSHOT_CHANGED"),
        "{}",
        failed(&job)
    );
    let mut before = run_events(6);
    for event in &mut before {
        event.event_time_ms = 1;
    }
    let job = submit(
        &h,
        replay_request(&fitted, "ev-before", event_spec(before, None)),
    )
    .await;
    assert!(
        failed(&job).starts_with("REPLAY_SNAPSHOT_CHANGED"),
        "{}",
        failed(&job)
    );
}
