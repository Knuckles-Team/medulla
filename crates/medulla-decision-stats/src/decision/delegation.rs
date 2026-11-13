//! The pure delegation verdict (MEDULLA-DECISION-ENGINE-R134.102).
//!
//! Admit only when the window is closed, every counted record carries one
//! policy digest, enough outcomes are counted, the newest is fresh and the
//! pooled success rate reaches the threshold. A request threshold may only
//! raise the knob.

use std::collections::BTreeSet;

use medulla_decision_types::decision::delegation::{
    digest_parts, DelegationAdmissionRequest, DelegationKnobs, DelegationOutcome,
    DelegationRefusal, DelegationVerdict, Q32_ONE,
};

/// A Q32 rate rounded up to millionths.
fn q32_to_micros(q32: u64) -> u64 {
    let scaled = u128::from(q32) * 1_000_000;
    let one = u128::from(Q32_ONE);
    u64::try_from(scaled.div_ceil(one)).unwrap_or(u64::MAX)
}

/// Decide whether `outcomes` (the counted independent evaluations of the
/// class inside the window) admit delegation at `now_ms`.
pub fn admit_delegation(
    request: &DelegationAdmissionRequest,
    outcomes: &[DelegationOutcome],
    now_ms: u64,
    knobs: &DelegationKnobs,
) -> Result<DelegationVerdict, DelegationRefusal> {
    if request.window_end_ms > now_ms {
        return Err(DelegationRefusal::WindowOpen {
            window_end_ms: request.window_end_ms,
            now_ms,
        });
    }
    let revisions: BTreeSet<&str> = outcomes.iter().map(|o| o.policy_digest.as_str()).collect();
    if revisions.len() > 1 {
        return Err(DelegationRefusal::MixedRevisions {
            revisions: revisions.len(),
        });
    }
    let samples = outcomes.len() as u64;
    if samples < knobs.min_samples {
        return Err(DelegationRefusal::InsufficientSamples {
            samples,
            needed: knobs.min_samples,
        });
    }
    let newest = outcomes.iter().map(|o| o.recorded_at_ms).max().unwrap_or(0);
    if now_ms.saturating_sub(newest) > knobs.max_age_ms {
        return Err(DelegationRefusal::StaleEvidence {
            newest_ms: newest,
            max_age_ms: knobs.max_age_ms,
        });
    }
    let successes = outcomes.iter().filter(|o| o.success).count() as u64;
    let threshold_micros = knobs
        .quality_threshold_micros
        .max(q32_to_micros(request.threshold_q32));
    if u128::from(successes) * 1_000_000 < u128::from(threshold_micros) * u128::from(samples) {
        return Err(DelegationRefusal::BelowThreshold {
            successes,
            samples,
            threshold_micros,
        });
    }
    let mut ids: Vec<String> = outcomes
        .iter()
        .map(|o| format!("{}|{}|{}", o.record_id, o.evaluation_id, o.success))
        .collect();
    ids.sort_unstable();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let outcome_set_digest = digest_parts(&id_refs);
    let policy_digest = revisions.into_iter().next().unwrap_or_default().to_string();
    let threshold_text = threshold_micros.to_string();
    let window = format!("{}-{}", request.window_start_ms, request.window_end_ms);
    let samples_text = format!("{successes}/{samples}");
    let verdict_digest = digest_parts(&[
        &request.task_class,
        &request.metric_id,
        &threshold_text,
        &window,
        &samples_text,
        &outcome_set_digest,
        &policy_digest,
    ]);
    Ok(DelegationVerdict {
        task_class: request.task_class.clone(),
        metric_id: request.metric_id.clone(),
        threshold_micros,
        window_start_ms: request.window_start_ms,
        window_end_ms: request.window_end_ms,
        samples,
        successes,
        outcome_set_digest,
        policy_digest,
        verdict_digest,
    })
}
