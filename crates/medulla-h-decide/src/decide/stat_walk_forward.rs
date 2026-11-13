//! `DecisionEval` in replay mode: walk-forward policy replay sealed as an
//! `EvaluationRun` (EH-528, ANALYTICS-HARVEST AH-08).
//!
//! The admitted gold items, in time order, are the steps. The candidate head
//! (refitted per fold on its trailing training window when the spec asks) and
//! the incumbent (a pinned head, or the uniform allocation) each allocate the
//! shared budget over every test step's options. The engine computes every
//! statistic itself -- the deflated Sharpe ratio with the declared trial count,
//! the probability of backtest overfitting over the folds, and Diebold-Mariano
//! against the incumbent -- with the same validation kernels a finance
//! `BacktestRun` seals, and refuses what replay cannot honestly answer: a
//! policy-dependent environment, bandit labels, an understated trial count, a
//! look-ahead.

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_stats::decision::admission::Regime;
use medulla_decision_stats::decision::fit::FitSpec;
use medulla_decision_stats::decision::refusal::RefusalResult;
use medulla_decision_stats::{decision::quant::q32, decision::quant::value_of};
use medulla_decision_stats::{
    decision::replay::cursor_of, decision::replay::gold_steps, decision::replay::head_digest,
    decision::replay::max_drawdown, decision::replay::mean, decision::replay::order_events,
    decision::replay::replay, decision::replay::sharpe, decision::replay::time_ordered,
    decision::replay::HeadReplay, decision::replay::OrderedEvents, decision::replay::ReplayOutcome,
    decision::replay::ReplayPolicy, decision::replay::ReplayStep, decision::replay::Uniform,
};
use medulla_decision_types::decision::statistical::dataset::LabelledItem;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::{
    decision::replay::EvaluationRun, decision::replay::OptionContribution,
    decision::replay::ReplayEnvironment, decision::replay::ReplayEvent,
    decision::replay::ReplayFoldView, decision::replay::ReplayReceipt,
    decision::replay::ReplaySpec, decision::replay::ReplayValidation, decision::replay::TrialLog,
};
use medulla_decision_types::{
    decision::DecisionEvalRequest, decision::DecisionJobOutput, decision::QuantisedValue,
};
use medulla_numeric::evaluation::backtest::{
    deflated_sharpe_ratio, diebold_mariano, probability_of_backtest_overfit,
};
use medulla_query_types::solve_scalar::digest_text;
use medulla_types::contract::BoundedVec;

use super::stat_jobs::{candidate_head, dataset_digest, EvalInputs, JobRun};
use super::stat_support::refusal;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_persistence_records::decision_jobs::evaluation_run_key;
use medulla_persistence_records::decision_jobs::replay_cursor_key;

/// Domain of an `EvaluationRun` digest.
const RUN_DOMAIN: &str = "eg/decision/evaluation-run/v1";
/// Domain of the digest of an event replay's result: the run without its
/// receipt, run digest and resume cursor, so a resumed replay of a stream
/// reproduces the digest of a replay of the whole stream.
const RESULT_DOMAIN: &str = "eg/decision/replay-result/v1";

fn rendered<T>(result: RefusalResult<T>) -> Result<T, String> {
    result.map_err(|refused| refused.render())
}

fn quantised(value: f64) -> Result<QuantisedValue, String> {
    rendered(q32(value))
}

/// The assumptions replay rests on, checked before anything is read.
fn check_spec(spec: &ReplaySpec, regime: Regime) -> Result<(), String> {
    let dependent = spec.env == ReplayEnvironment::PolicyDependent || regime != Regime::FullLabel;
    if dependent {
        return Err(refusal(
            StatisticalErrorCode::ReplayPolicyDependent,
            "replay evaluates only a policy-independent environment over gold labels; use the off-policy estimators",
        ));
    }
    let TrialLog { declared, searched } = spec.trials;
    if declared == 0 || declared < searched {
        return Err(refusal(
            StatisticalErrorCode::TrialsUnderstated,
            format!("{declared} trials declared, the search log records {searched}"),
        ));
    }
    Ok(())
}

fn check_supersedes(
    store: &AgentLibraryStore,
    tenant_id: &str,
    spec: &ReplaySpec,
) -> Result<(), String> {
    let Some(previous) = &spec.supersedes else {
        return Ok(());
    };
    match store.decision_artifact(tenant_id, &evaluation_run_key(previous))? {
        Some(_) => Ok(()),
        None => Err(refusal(
            StatisticalErrorCode::ReplaySpecInvalid,
            format!("supersedes names no evaluation run {previous}"),
        )),
    }
}

/// The candidate and incumbent replayed over the steps.
fn replayed(
    store: &AgentLibraryStore,
    inputs: &EvalInputs,
    request: &DecisionEvalRequest,
    spec: &ReplaySpec,
    items: &[&LabelledItem],
    steps: &[ReplayStep],
) -> Result<ReplayOutcome, String> {
    let refit = spec.refit.map(|refit| FitSpec {
        head_kind: refit.head_kind,
        regime: Regime::FullLabel,
        optimiser: refit.optimiser,
        feature_schema_digest: &inputs.head.feature_schema_digest,
        statistical: &inputs.policy.statistical,
        penalty: None,
        calibration_method: None,
    });
    let mut candidate = HeadReplay {
        dataset: &inputs.dataset,
        items,
        head: inputs.head.clone(),
        refit,
    };
    let mut incumbent: Box<dyn ReplayPolicy + '_> = match &spec.incumbent {
        None => Box::new(Uniform { steps }),
        Some(pin) => Box::new(HeadReplay {
            dataset: &inputs.dataset,
            items,
            head: incumbent_head(store, inputs, &request.tenant_id, pin)?,
            refit: None,
        }),
    };
    let cap = value_of(spec.budget.cap);
    rendered(replay(
        steps,
        &spec.folds,
        cap,
        &mut candidate,
        incumbent.as_mut(),
    ))
}

fn incumbent_head(
    store: &AgentLibraryStore,
    inputs: &EvalInputs,
    tenant_id: &str,
    pin: &medulla_decision_types::decision::EvalCandidate,
) -> Result<medulla_decision_types::decision::statistical::head::DecisionHeadBody, String> {
    let head = candidate_head(store, tenant_id, pin)?;
    if head.feature_schema_digest != inputs.dataset.feature_schema_digest {
        return Err(refusal(
            StatisticalErrorCode::HeadInvalid,
            "the incumbent reads a different feature schema than the dataset",
        ));
    }
    Ok(head)
}

/// Rows = folds, columns = (candidate, incumbent): in-sample is the training
/// window, out-of-sample the test window.
fn overfit_matrices(outcome: &ReplayOutcome) -> (Vec<Vec<f64>>, Vec<Vec<f64>>) {
    outcome
        .folds
        .iter()
        .map(|fold| {
            (
                vec![fold.insample, fold.incumbent_insample],
                vec![mean(&fold.path), mean(&fold.incumbent_path)],
            )
        })
        .unzip()
}

fn negated(path: &[f64]) -> Vec<f64> {
    path.iter().map(|utility| -utility).collect()
}

fn validation(outcome: &ReplayOutcome, trials: TrialLog) -> Result<ReplayValidation, String> {
    let path = outcome.path();
    let observed = sharpe(&path).ok_or_else(|| {
        refusal(
            StatisticalErrorCode::ReplaySpecInvalid,
            "the replayed utility path has no variance; a Sharpe ratio is undefined",
        )
    })?;
    let (insample, oos) = overfit_matrices(outcome);
    let compared = diebold_mariano(&negated(&path), &negated(&outcome.incumbent_path()), 1);
    Ok(ReplayValidation {
        observed_sharpe: quantised(observed)?,
        n_trials: trials.declared,
        deflated_sharpe: quantised(deflated_sharpe_ratio(
            observed,
            trials.declared as usize,
            &path,
        ))?,
        probability_backtest_overfit: quantised(probability_of_backtest_overfit(&insample, &oos))?,
        diebold_mariano: quantised(compared.statistic)?,
        diebold_mariano_p: quantised(compared.p_value)?,
    })
}

fn fold_views(
    outcome: &ReplayOutcome,
    steps: &[ReplayStep],
) -> Result<Vec<ReplayFoldView>, String> {
    outcome
        .folds
        .iter()
        .map(|fold| {
            Ok(ReplayFoldView {
                train_items: fold.fold.train.len() as u32,
                test_items: fold.fold.test.len() as u32,
                first_test_ms: steps[fold.fold.test.start].at_ms,
                last_test_ms: steps[fold.fold.test.end - 1].at_ms,
                head_digest: fold.head_digest.clone(),
                utility: quantised(fold.path.iter().sum())?,
                incumbent_utility: quantised(fold.incumbent_path.iter().sum())?,
                max_drawdown: quantised(max_drawdown(&fold.path))?,
                abstained: fold.abstained,
            })
        })
        .collect()
}

fn contributions(outcome: &ReplayOutcome) -> Result<Vec<OptionContribution>, String> {
    outcome
        .contributions
        .iter()
        .map(|(option_id, (applied, utility))| {
            Ok(OptionContribution {
                option_id: option_id.clone(),
                applied: quantised(*applied)?,
                utility: quantised(*utility)?,
            })
        })
        .collect()
}

fn bounded<T, const N: usize>(values: Vec<T>, what: &str) -> Result<BoundedVec<T, N>, String> {
    BoundedVec::new(values).map_err(|detail| {
        refusal(
            StatisticalErrorCode::ReplaySpecInvalid,
            format!("{what}: {detail}"),
        )
    })
}

/// The recorded events of an event replay, ordered and checked.
struct EventStream {
    ordered: OrderedEvents,
}

fn invalid(detail: impl std::fmt::Display) -> String {
    refusal(StatisticalErrorCode::ReplaySpecInvalid, detail)
}

/// The component revision an event read must be the one the component held at
/// the event's time: the latest revision published at or before it.
fn check_snapshot(
    store: &AgentLibraryStore,
    tenant_id: &str,
    event: &ReplayEvent,
) -> Result<(), String> {
    let pinned = &event.snapshot;
    let held = store
        .component_revisions(tenant_id, &pinned.component_id)?
        .into_iter()
        .rfind(|revision| revision.updated_at_ms <= event.event_time_ms);
    match held {
        Some(revision)
            if revision.kind == pinned.kind
                && revision.definition_digest == pinned.definition_digest =>
        {
            Ok(())
        }
        Some(revision) => Err(refusal(
            StatisticalErrorCode::ReplaySnapshotChanged,
            format!(
                "event `{}` read {} at {}, which held {} at {}",
                event.event_id,
                pinned.component_id,
                pinned.definition_digest,
                revision.definition_digest,
                event.event_time_ms
            ),
        )),
        None => Err(refusal(
            StatisticalErrorCode::ReplaySnapshotChanged,
            format!(
                "event `{}` read {}, which had no revision at {}",
                event.event_id, pinned.component_id, event.event_time_ms
            ),
        )),
    }
}

/// Where a resumed replay starts: the event after the cursor, once an earlier
/// run is stored under that cursor and applied the same events up to it.
fn resume_start(
    store: &AgentLibraryStore,
    tenant_id: &str,
    ordered: &OrderedEvents,
    cursor: &str,
) -> Result<usize, String> {
    let mut at = None;
    for (index, event) in ordered.applied.iter().enumerate() {
        if rendered(cursor_of(event))? == cursor {
            at = Some(index);
            break;
        }
    }
    let Some(at) = at else {
        return Err(invalid(format!(
            "resume cursor {cursor} names no event of the stream"
        )));
    };
    let stored = store
        .decision_artifact(tenant_id, &replay_cursor_key(cursor))?
        .ok_or_else(|| invalid(format!("no earlier replay reached cursor {cursor}")))?;
    let run_digest = String::from_utf8(stored)
        .map_err(|_| "CORRUPT_DECISION_ARTIFACT: invalid replay cursor row".to_string())?;
    let earlier = store
        .decision_artifact(tenant_id, &evaluation_run_key(&run_digest))?
        .ok_or_else(|| "CORRUPT_DECISION_ARTIFACT: replay cursor names no run".to_string())?;
    let earlier: EvaluationRun = decode_artifact(&earlier, "evaluation run")?;
    let prefix = earlier
        .event_receipt
        .as_ref()
        .map(|receipt| receipt.event_digests.as_slice())
        .unwrap_or_default();
    if prefix != &ordered.digests[..=at] {
        return Err(invalid(format!(
            "the earlier replay at cursor {cursor} applied different events"
        )));
    }
    Ok(at + 1)
}

/// Order the spec's events, check the ones a resumed replay has not already
/// attested against their snapshots, and return the stream. `None` for a
/// replay of every admitted item.
fn event_stream(
    store: &AgentLibraryStore,
    tenant_id: &str,
    spec: &ReplaySpec,
) -> Result<Option<EventStream>, String> {
    if spec.events.as_slice().is_empty() {
        if spec.resume_cursor.is_some() {
            return Err(invalid("a resume cursor needs the event stream it resumes"));
        }
        return Ok(None);
    }
    let ordered = rendered(order_events(spec.events.as_slice()))?;
    let start = match &spec.resume_cursor {
        Some(cursor) => resume_start(store, tenant_id, &ordered, cursor)?,
        None => 0,
    };
    for event in &ordered.applied[start..] {
        check_snapshot(store, tenant_id, event)?;
    }
    Ok(Some(EventStream { ordered }))
}

/// The admitted items the applied events' outcome references name.
fn event_items<'a>(
    stream: &EventStream,
    admitted: &[&'a LabelledItem],
) -> Result<Vec<&'a LabelledItem>, String> {
    let named: std::collections::BTreeSet<&str> = stream
        .ordered
        .applied
        .iter()
        .flat_map(|event| event.outcome_refs.as_slice().iter().map(String::as_str))
        .collect();
    let items: Vec<&LabelledItem> = admitted
        .iter()
        .copied()
        .filter(|item| named.contains(item.item_id.as_str()))
        .collect();
    if let Some(missing) = named
        .iter()
        .find(|id| !items.iter().any(|item| item.item_id == **id))
    {
        return Err(invalid(format!(
            "outcome reference {missing} names no admitted item"
        )));
    }
    Ok(items)
}

/// Seal the run: every output derived here, the digest over all of it.
fn seal(
    inputs: &EvalInputs,
    spec: &ReplaySpec,
    outcome: &ReplayOutcome,
    steps: &[ReplayStep],
    stream: Option<&EventStream>,
    policy: &DecisionPolicyRef,
) -> Result<EvaluationRun, String> {
    let path = outcome
        .path()
        .into_iter()
        .map(quantised)
        .collect::<Result<Vec<_>, _>>()?;
    let mut run = EvaluationRun {
        run_digest: String::new(),
        head_digest: rendered(head_digest(&inputs.head))?,
        policy_digest: inputs.policy.digest.clone(),
        dataset_digest: dataset_digest(&inputs.dataset)?,
        spec: spec.clone(),
        folds: bounded(fold_views(outcome, steps)?, "folds")?,
        path: bounded(path, "replayed steps")?,
        contributions: bounded(contributions(outcome)?, "option contributions")?,
        validation: validation(outcome, spec.trials)?,
        synthetic: inputs.dataset.synthetic,
        event_receipt: None,
    };
    if let Some(stream) = stream {
        let mut result = run.clone();
        result.spec.resume_cursor = None;
        run.event_receipt = Some(ReplayReceipt {
            event_digests: bounded(stream.ordered.digests.clone(), "event digests")?,
            cursor: stream.ordered.cursor.clone().unwrap_or_default(),
            policy: policy.clone(),
            result_digest: digest_text(RESULT_DOMAIN, &result),
        });
    }
    run.run_digest = digest_text(RUN_DOMAIN, &run);
    Ok(run)
}

/// Run one replay evaluation to its sealed record and artifact row.
pub(super) fn run(
    store: &AgentLibraryStore,
    inputs: &EvalInputs,
    request: &DecisionEvalRequest,
    spec: &ReplaySpec,
) -> JobRun {
    check_spec(spec, inputs.regime)?;
    check_supersedes(store, &request.tenant_id, spec)?;
    let stream = event_stream(store, &request.tenant_id, spec)?;
    let admitted = inputs.admitted(request.window);
    let named;
    let candidates = match &stream {
        Some(stream) => {
            named = event_items(stream, &admitted.items)?;
            &named
        }
        None => &admitted.items,
    };
    let items = time_ordered(candidates);
    let steps = rendered(gold_steps(&items))?;
    let outcome = replayed(store, inputs, request, spec, &items, &steps)?;
    let run = seal(
        inputs,
        spec,
        &outcome,
        &steps,
        stream.as_ref(),
        &request.policy,
    )?;
    let mut rows = vec![(evaluation_run_key(&run.run_digest), encode_artifact(&run)?)];
    if let Some(receipt) = &run.event_receipt {
        let key = replay_cursor_key(&receipt.cursor);
        if store.decision_artifact(&request.tenant_id, &key)?.is_none() {
            rows.push((key, run.run_digest.clone().into_bytes()));
        }
    }
    Ok((DecisionJobOutput::Replay { run: Box::new(run) }, rows))
}
