//! `DecisionLog.learn` with a run outcome (MEDULLA-DECISION-ENGINE-R100.102.3,
//! .4, .5): the one write that credits a delegated run.
//!
//! The caller records a run's answer digest, cited rows and step rewards. The
//! engine joins the cited rows to the rows the referenced decisions retrieved
//! (their stored `RetrievalOutcome`s), credits each step against its
//! capability's baseline and stores one [`RunCreditReceipt`].
//!
//! Stored rows (decision artifact store, one tenant):
//! * `run_credit/run/<run_id>` -- the receipt, written once.
//! * `run_credit/baseline/<sha256 of capability_iri>/<inverted revision>` --
//!   one row per revision, written once. The revision is stored as
//!   `u32::MAX - revision` in ten digits, so the latest revision sorts first
//!   and the head is one page read, however many revisions exist.
//!
//! The receipt and every baseline revision commit in one atomic batch, so a
//! refusal -- or a concurrent writer that took the next revision first, which
//! retries once against the new head -- leaves nothing half written.

use std::collections::BTreeMap;

use medulla_decision_stats::decision::run_credit::{credit_steps, Baseline};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::statistical::run_credit::{
    baseline_weight_q16, RunCreditReceipt, RunOutcome, StepCredit, KNOBS,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::{BoundedVec, Digest256};
use serde::{Deserialize, Serialize};

use super::stat_executor::ExecutionContext;
use super::stat_log::{visible_entry, LogReader};
use super::stat_retrieval::{check_joinable, stored_outcome};
use super::stat_support::{refusal, resolve_policy};

const RUN_PREFIX: &str = "run_credit/run/";
const BASELINE_PREFIX: &str = "run_credit/baseline/";

/// One stored baseline revision of a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredBaseline {
    pub(super) capability_iri: String,
    pub(super) value_q16: i32,
    pub(super) revision: u32,
}

fn invalid_argument(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

fn run_key(run_id: &str) -> String {
    format!("{RUN_PREFIX}{run_id}")
}

fn capability_prefix(capability_iri: &str) -> String {
    let hash = Digest256::sha256(capability_iri.as_bytes()).to_hex();
    format!("{BASELINE_PREFIX}{hash}/")
}

fn baseline_key(capability_iri: &str, revision: u32) -> String {
    format!(
        "{}{:010}",
        capability_prefix(capability_iri),
        u32::MAX - revision
    )
}

/// The latest stored baseline of `capability_iri`, if any.
pub(super) fn latest_baseline(
    store: &AgentLibraryStore,
    tenant_id: &str,
    capability_iri: &str,
) -> Result<Option<StoredBaseline>, String> {
    store
        .decision_artifacts_page(tenant_id, &capability_prefix(capability_iri), None, 1)?
        .into_iter()
        .next()
        .map(|(_, bytes)| decode_artifact(&bytes, "run credit baseline"))
        .transpose()
}

/// The stored receipt of run `run_id`, if any.
pub(super) fn stored_receipt(
    store: &AgentLibraryStore,
    tenant_id: &str,
    run_id: &str,
) -> Result<Option<RunCreditReceipt>, String> {
    store
        .decision_artifact(tenant_id, &run_key(run_id))?
        .map(|bytes| decode_artifact(&bytes, "run credit receipt"))
        .transpose()
}

fn json_digest<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(Digest256::sha256(&bytes).to_hex())
}

/// The two knobs a run reads, from the default decision policy.
fn run_knobs(ctx: &ExecutionContext) -> Result<(usize, KnobValue), String> {
    let policy = resolve_policy(ctx.store, ctx.tenant_id, &DecisionPolicyRef::Default)?;
    match (policy.policy.knob(&KNOBS[0]), policy.policy.knob(&KNOBS[1])) {
        (KnobValue::Integer(max_steps), weight @ KnobValue::DecimalMicros(_)) => {
            Ok((usize::try_from(max_steps).unwrap_or(usize::MAX), weight))
        }
        _ => Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "run_credit knobs have the wrong value type",
        )),
    }
}

/// Credit one run and store its receipt and new baselines.
pub(super) fn record_run(
    ctx: &ExecutionContext,
    reader: &LogReader,
    outcome: RunOutcome,
) -> Result<RunCreditReceipt, String> {
    let (max_steps, weight) = run_knobs(ctx)?;
    outcome
        .check(max_steps)
        .map_err(|refused| format!("{}: {}", refused.code.as_str(), refused.detail))?;

    let mut retrieved = Vec::with_capacity(outcome.decision_ids.len());
    for decision_id in outcome.decision_ids.iter() {
        let entry = visible_entry(ctx.store, reader, decision_id)?;
        check_joinable(entry, reader).map_err(|detail| {
            invalid_argument(format!(
                "run {} decision {decision_id}: {detail}",
                outcome.run_id
            ))
        })?;
        let stored =
            stored_outcome(ctx.store, &reader.tenant_id, decision_id)?.ok_or_else(|| {
                invalid_argument(format!(
                    "run {} decision {decision_id} has no recorded retrieval outcome",
                    outcome.run_id
                ))
            })?;
        retrieved.push(stored.outcome);
    }
    for row in outcome.cited_row_ids.iter() {
        let returned = retrieved
            .iter()
            .any(|r| r.returned.iter().any(|unit| unit.evidence_id == *row));
        if !returned {
            return Err(refusal(
                StatisticalErrorCode::CitationNotRetrieved,
                format!(
                    "run {} cites row {row}, which no referenced decision returned",
                    outcome.run_id
                ),
            ));
        }
    }

    let outcome_digest = json_digest(&outcome)?;
    for attempt in 0..2 {
        if let Some(stored) = stored_receipt(ctx.store, ctx.tenant_id, &outcome.run_id)? {
            if stored.outcome_digest == outcome_digest {
                return Ok(stored);
            }
            return Err(refusal(
                StatisticalErrorCode::IdempotencyConflict,
                format!("run {} is recorded with another outcome", outcome.run_id),
            ));
        }
        let (receipt, rows) = credit(ctx, &outcome, &retrieved, &outcome_digest, weight)?;
        match ctx.store.put_decision_artifacts(ctx.tenant_id, &rows) {
            Ok(()) => return Ok(receipt),
            // Another writer took the next revision first: read the new head once.
            Err(error) if attempt == 0 && error.starts_with("IDEMPOTENCY_CONFLICT") => {}
            Err(error) => return Err(error),
        }
    }
    Err(refusal(
        StatisticalErrorCode::IdempotencyConflict,
        format!(
            "run {} lost a concurrent baseline write twice",
            outcome.run_id
        ),
    ))
}

/// Run the kernel against the current baselines; the receipt and its rows.
fn credit(
    ctx: &ExecutionContext,
    outcome: &RunOutcome,
    retrieved: &[medulla_decision_types::decision::statistical::retrieval::RetrievalOutcome],
    outcome_digest: &str,
    weight: KnobValue,
) -> Result<(RunCreditReceipt, Vec<(String, Vec<u8>)>), String> {
    let mut baselines = BTreeMap::new();
    for step in outcome.steps.iter() {
        if baselines.contains_key(&step.capability_iri) {
            continue;
        }
        if let Some(stored) = latest_baseline(ctx.store, ctx.tenant_id, &step.capability_iri)? {
            baselines.insert(
                step.capability_iri.clone(),
                Baseline {
                    value_q16: stored.value_q16,
                    revision: stored.revision,
                },
            );
        }
    }
    let KnobValue::DecimalMicros(micros) = weight else {
        return Err("run_credit.baseline_weight is a decimal".to_string());
    };
    let w_q16 = baseline_weight_q16(micros);
    let refs: Vec<_> = retrieved.iter().collect();
    let result = credit_steps(
        &refs,
        outcome.cited_row_ids.as_slice(),
        outcome.success,
        outcome.steps.as_slice(),
        &baselines,
        w_q16,
    );
    let steps = result
        .advantages
        .iter()
        .map(|a| StepCredit {
            step_idx: a.step_idx,
            advantage_q16: a.advantage_q16,
            baseline_revision: a.baseline_revision,
        })
        .collect();
    let premises = vec![
        format!("run:{}", outcome.run_id),
        format!("run:positives={}", result.positives.len()),
        format!("run:negatives={}", result.negatives.len()),
        format!("knob:{}={}", KNOBS[1].name, weight.render()),
    ];
    let mut receipt = RunCreditReceipt {
        run_id: outcome.run_id.clone(),
        decision_ids: outcome.decision_ids.clone(),
        outcome_digest: outcome_digest.to_string(),
        positive_count: u32::try_from(result.positives.len()).unwrap_or(u32::MAX),
        negative_count: u32::try_from(result.negatives.len()).unwrap_or(u32::MAX),
        steps: BoundedVec::new(steps).map_err(|_| "a run holds too many steps".to_string())?,
        premises: BoundedVec::new(premises).map_err(|_| "too many premises".to_string())?,
        receipt_digest: String::new(),
    };
    receipt.receipt_digest = json_digest(&receipt)?;
    let mut rows = vec![(run_key(&outcome.run_id), encode_artifact(&receipt)?)];
    for update in &result.updates {
        let stored = StoredBaseline {
            capability_iri: update.capability_iri.clone(),
            value_q16: update.baseline.value_q16,
            revision: update.baseline.revision,
        };
        rows.push((
            baseline_key(&update.capability_iri, update.baseline.revision),
            encode_artifact(&stored)?,
        ));
    }
    Ok((receipt, rows))
}
