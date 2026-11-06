//! OTel spans for the statistical decision surface (EH-074).
//!
//! Each served call runs inside one `tracing` span (`medulla.decide`,
//! `medulla.decision_fit`, `medulla.decision_eval`), exported by the engine's OTLP layer
//! when it is configured, and ends with one structured event carrying the
//! resolution kind, evidence class, outcome, abstention reason, candidate
//! count, exploration/audit flags, effective sample sizes and latency. The
//! dashboards and alert rules over them belong to W6 (LGTM spanmetrics).

use std::future::Future;
use std::time::Instant;

use tracing::Instrument;

use medulla_decision_types::decision::replay::EvaluationRun;
use medulla_decision_types::{
    decision::jobs::DecisionEvalReceipt, decision::jobs::DecisionThresholdAssessment,
};
use medulla_decision_types::{
    decision::statistical::StatisticalDecisionRecord, decision::statistical::StatisticalOutcome,
};

/// The span one served statistical call runs in.
pub(super) fn span(method: &'static str, tenant_id: &str) -> tracing::Span {
    tracing::info_span!("medulla.decide.method", method, tenant = tenant_id)
}

/// One stage of the decision pipeline (MEDULLA-DECISION-ENGINE-R072.100). The
/// set is closed, so the `stage` metric label is bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Admit,
    Visibility,
    Candidates,
    Constraints,
    Entailment,
    Solve,
    Rank,
    Conclude,
    Record,
    Commit,
    Learn,
    Replay,
}

impl Stage {
    /// The stage token in the span name `medulla.decide.<stage>` and the metric label.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Stage::Admit => "admit",
            Stage::Visibility => "visibility",
            Stage::Candidates => "candidates",
            Stage::Constraints => "constraints",
            Stage::Entailment => "entailment",
            Stage::Solve => "solve",
            Stage::Rank => "rank",
            Stage::Conclude => "conclude",
            Stage::Record => "record",
            Stage::Commit => "commit",
            Stage::Learn => "learn",
            Stage::Replay => "replay",
        }
    }
}

/// The span of one pipeline stage: `medulla.decide.<stage>` with the
/// attributes `family`, `tenant`, `outcome` and `duration_ms` (the last two
/// are recorded when the stage ends).
pub(super) fn stage_span(stage: Stage, family: &str, tenant: &str) -> tracing::Span {
    macro_rules! named {
        ($name:literal) => {
            tracing::info_span!(
                $name,
                family,
                tenant,
                outcome = tracing::field::Empty,
                duration_ms = tracing::field::Empty
            )
        };
    }
    match stage {
        Stage::Admit => named!("medulla.decide.admit"),
        Stage::Visibility => named!("medulla.decide.visibility"),
        Stage::Candidates => named!("medulla.decide.candidates"),
        Stage::Constraints => named!("medulla.decide.constraints"),
        Stage::Entailment => named!("medulla.decide.entailment"),
        Stage::Solve => named!("medulla.decide.solve"),
        Stage::Rank => named!("medulla.decide.rank"),
        Stage::Conclude => named!("medulla.decide.conclude"),
        Stage::Record => named!("medulla.decide.record"),
        Stage::Commit => named!("medulla.decide.commit"),
        Stage::Learn => named!("medulla.decide.learn"),
        Stage::Replay => named!("medulla.decide.replay"),
    }
}

fn end_stage(span: &tracing::Span, stage: Stage, family: &str, ok: bool, started: Instant) {
    let elapsed = started.elapsed();
    span.record("outcome", if ok { "ok" } else { "error" });
    span.record("duration_ms", elapsed.as_millis() as u64);
    medulla_kernel::metrics::decide_stage_seconds(family, stage.as_str(), elapsed.as_secs_f64());
}

/// Run one synchronous stage inside its span.
pub(super) fn stage<T, E>(
    stage: Stage,
    family: &str,
    tenant: &str,
    run: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let span = stage_span(stage, family, tenant);
    let started = Instant::now();
    let result = span.in_scope(run);
    end_stage(&span, stage, family, result.is_ok(), started);
    result
}

/// Run one asynchronous stage inside its span.
pub(super) async fn stage_async<T, E>(
    stage: Stage,
    family: &str,
    tenant: &str,
    run: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let span = stage_span(stage, family, tenant);
    let started = Instant::now();
    let result = run.instrument(span.clone()).await;
    end_stage(&span, stage, family, result.is_ok(), started);
    result
}

/// A stage whose work already finished (a job run): one span, closed at once,
/// carrying the work's own duration.
pub(super) fn stage_done(stage: Stage, family: &str, tenant: &str, ok: bool, started: Instant) {
    let span = stage_span(stage, family, tenant);
    end_stage(&span, stage, family, ok, started);
}

/// The metric and span family label of a question kind.
pub(super) fn family(kind: medulla_decision_types::decision::statistical::QuestionKind) -> String {
    format!("{kind:?}").to_ascii_lowercase()
}

/// One solved or abstained assembly: abstention codes and the solver's node use.
pub(super) fn assembled(record: &medulla_decision_types::decision::DecisionRecord) {
    use medulla_decision_types::decision::DecisionOutcome;
    match &record.outcome {
        DecisionOutcome::Solved { certificate, .. } => medulla_kernel::metrics::solver_nodes_ratio(
            certificate.nodes_expanded,
            record.inputs.solver.node_budget,
        ),
        DecisionOutcome::Abstained { reasons } => {
            for reason in reasons.iter() {
                medulla_kernel::metrics::decide_abstentions(&variant_token(&format!("{reason:?}")));
            }
        }
    }
}

fn variant_token(debug: &str) -> String {
    debug
        .split([' ', '{', '('])
        .next()
        .unwrap_or("")
        .to_string()
}

fn outcome_label(outcome: &StatisticalOutcome) -> (&'static str, String) {
    match outcome {
        StatisticalOutcome::Acted { .. } => ("acted", String::new()),
        StatisticalOutcome::Explored { .. } => ("explored", String::new()),
        StatisticalOutcome::Advisory { .. } => ("advisory", String::new()),
        StatisticalOutcome::Abstained { reasons } => (
            "abstained",
            reasons
                .iter()
                .map(|reason| {
                    format!("{reason:?}")
                        .split([' ', '{'])
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(","),
        ),
    }
}

/// One `Decide` answer.
pub(super) fn decided(record: &StatisticalDecisionRecord, candidates: usize, started: Instant) {
    let (outcome, reasons) = outcome_label(&record.outcome);
    let evidence = format!("{:?}", record.evidence_class);
    if outcome == "abstained" {
        for code in reasons.split(',').filter(|code| !code.is_empty()) {
            medulla_kernel::metrics::decide_abstentions(code);
        }
    }
    medulla_kernel::metrics::decision_answered(outcome, &evidence, &reasons);
    medulla_kernel::metrics::decision_latency("Decide", started.elapsed().as_secs_f64());
    if record.inputs.exploration.is_some() {
        medulla_kernel::metrics::decision_explored(matches!(
            record.outcome,
            StatisticalOutcome::Explored { .. }
        ));
    }
    tracing::info!(
        target: "medulla.decide",
        resolution_kind = ?record.resolution_kind,
        evidence_class = ?record.evidence_class,
        outcome,
        abstain_reasons = %reasons,
        candidates,
        explored = record.inputs.exploration.is_some(),
        audit_sampled = record.audit.is_some_and(|a| a.sampled),
        synthetic = record.synthetic_evidence,
        latency_ms = started.elapsed().as_millis() as u64,
        "decision served"
    );
}

/// One finished replay evaluation (EH-528).
pub(super) fn replayed(run: &EvaluationRun, started: Instant) {
    medulla_kernel::metrics::decision_latency("DecisionEval", started.elapsed().as_secs_f64());
    tracing::info!(
        target: "medulla.decide",
        run_digest = %run.run_digest,
        folds = run.folds.len(),
        steps = run.path.len(),
        n_trials = run.validation.n_trials,
        deflated_sharpe_q32 = run.validation.deflated_sharpe.value,
        pbo_q32 = run.validation.probability_backtest_overfit.value,
        latency_ms = started.elapsed().as_millis() as u64,
        "decision head replayed"
    );
}

/// One finished evaluation.
pub(super) fn evaluated(receipt: &DecisionEvalReceipt, started: Instant) {
    medulla_kernel::metrics::decision_evaluated(receipt.passed);
    medulla_kernel::metrics::decision_latency("DecisionEval", started.elapsed().as_secs_f64());
    let min_ess = receipt
        .estimates
        .iter()
        .map(|e| e.effective_sample_size.value)
        .min()
        .unwrap_or(0);
    tracing::info!(
        target: "medulla.decide",
        passed = receipt.passed,
        n_records = receipt.n_records,
        estimators = receipt.estimates.len(),
        min_ess_q32 = min_ess,
        failed_gates = %receipt.failed_gates.as_slice().join(","),
        latency_ms = started.elapsed().as_millis() as u64,
        "decision head evaluated"
    );
}

/// Emit only after the full-label receipt and its pinned-policy assessment
/// have committed. Idempotent job replays must not call this function.
pub(super) fn threshold_assessed(assessment: Option<&DecisionThresholdAssessment>) {
    let Some(assessment) = assessment else {
        return;
    };
    fn result(value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "breach",
            Some(false) => "ok",
            None => "unavailable",
        }
    }

    let support = if assessment.insufficient_support {
        "breach"
    } else {
        "ok"
    };
    medulla_kernel::metrics::decision_threshold_assessed("support", support);
    medulla_kernel::metrics::decision_threshold_assessed(
        "coverage",
        result(assessment.coverage_below_policy),
    );
    medulla_kernel::metrics::decision_threshold_assessed(
        "act_risk",
        result(assessment.act_risk_above_policy),
    );
}

/// One finished fit.
pub(super) fn fitted(n_training: u64, calibrated: bool, started: Instant) {
    medulla_kernel::metrics::decision_latency("DecisionFit", started.elapsed().as_secs_f64());
    tracing::info!(
        target: "medulla.decide",
        n_training,
        calibrated,
        latency_ms = started.elapsed().as_millis() as u64,
        "decision head fitted"
    );
}

/// One refused call.
pub(super) fn refused(method: &'static str, error: &str) {
    let code = refusal_code(error);
    medulla_kernel::metrics::decision_refused(method, code);
    tracing::info!(target: "medulla.decide", method, code, "decision call refused");
}

/// The closed code token of a refusal text, or `OTHER` for free text, so the
/// metric label set stays bounded.
fn refusal_code(error: &str) -> &str {
    let code = error.split(':').next().unwrap_or("");
    let closed = !code.is_empty() && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_');
    if closed {
        code
    } else {
        "OTHER"
    }
}

/// One served decision-log call.
pub(super) fn logged(ok: bool, started: Instant) {
    medulla_kernel::metrics::decision_latency("DecisionLog", started.elapsed().as_secs_f64());
    tracing::info!(
        target: "medulla.decide",
        ok,
        latency_ms = started.elapsed().as_millis() as u64,
        "decision log call served"
    );
}
