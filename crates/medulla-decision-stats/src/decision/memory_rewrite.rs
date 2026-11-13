//! Fidelity-gated memory rewrite kernel (MEDULLA-DECISION-ENGINE-R141.100).
//!
//! [`evaluate_rewrite`] is a pure, deterministic function of the proposal, the
//! resolved knobs and the stored head revision: no clock, no store. The
//! `DecisionEval` job authority writes the receipt it returns.

use std::collections::BTreeSet;

use medulla_decision_types::decision::memory_rewrite::{
    MemoryFidelityMetric, MemoryRewriteEvaluation, MemoryRewriteKnobs, MemoryRewriteProposal,
    RewriteHistory, RewriteReason, RewriteVerdict, MEMORY_ACTIVE_SIGNALS_EMPTY,
    MEMORY_HISTORY_MISSING, MEMORY_METRIC_UNRESOLVED, MEMORY_POLICY_MISSING, MEMORY_REVISION_STALE,
    MEMORY_TOKEN_DENOMINATOR_ZERO, Q4_ONE,
};
use medulla_types::refusal_code::RefusalCode;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// One signal in normal form: NFC, lower case, whitespace runs collapsed to one
/// space, trimmed.
pub fn normalize_signal(signal: &str) -> String {
    let composed: String = signal.nfc().collect();
    let lowered = composed.to_lowercase();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The set of SHA-256 hashes of the normalized, non-empty signals.
fn signal_hashes(signals: &[String]) -> BTreeSet<[u8; 32]> {
    signals
        .iter()
        .map(|signal| normalize_signal(signal))
        .filter(|signal| !signal.is_empty())
        .map(|signal| Sha256::digest(signal.as_bytes()).into())
        .collect()
}

/// `numerator / denominator` rounded half to even.
fn round_half_even(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    let twice_remainder = (numerator % denominator) * 2;
    match twice_remainder.cmp(&denominator) {
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => quotient + (quotient % 2),
        std::cmp::Ordering::Less => quotient,
    }
}

/// Evaluate one proposal. Refusals, in order: unresolved metric, missing
/// policy, missing or mismatched history, zero token count, no active signals,
/// a stale active revision. Otherwise the verdict is `Discard` with every
/// reason that applies, or `Admit`.
pub fn evaluate_rewrite(
    proposal: &MemoryRewriteProposal,
    knobs: &MemoryRewriteKnobs,
    stored_head_revision: u64,
) -> Result<MemoryRewriteEvaluation, RefusalCode> {
    let metric =
        MemoryFidelityMetric::resolve(&proposal.metric_id).ok_or(MEMORY_METRIC_UNRESOLVED)?;
    if proposal.policy.trim().is_empty() {
        return Err(MEMORY_POLICY_MISSING);
    }
    let history = &proposal.history;
    if history.history_digest.is_empty()
        || history.history_digest != RewriteHistory::digest_of(history.entries.as_slice())
    {
        return Err(MEMORY_HISTORY_MISSING);
    }
    if proposal.token_count == 0 {
        return Err(MEMORY_TOKEN_DENOMINATOR_ZERO);
    }
    let active = signal_hashes(&proposal.active_signals);
    if active.is_empty() {
        return Err(MEMORY_ACTIVE_SIGNALS_EMPTY);
    }
    if proposal.active.revision != stored_head_revision {
        return Err(MEMORY_REVISION_STALE);
    }

    let candidate = signal_hashes(&proposal.candidate_signals);
    let kept = active.intersection(&candidate).count() as u128;
    let fidelity_q4 = round_half_even(kept * u128::from(Q4_ONE), active.len() as u128) as u32;
    let signal_count = candidate.len() as u64;
    let signal_per_token_q4 = round_half_even(
        u128::from(signal_count) * u128::from(Q4_ONE),
        u128::from(proposal.token_count),
    ) as u64;

    // Q4 values are hundredths of a percent; the knobs are millionths.
    let below_min = |q4: u64| q4 * 100 < knobs.min_fidelity_micros;
    let mut reason_codes = Vec::new();
    if below_min(u64::from(fidelity_q4)) {
        reason_codes.push(RewriteReason::FidelityBelowMin);
    }
    if signal_per_token_q4 * 100 < knobs.min_signal_per_token_micros {
        reason_codes.push(RewriteReason::SignalEfficiencyBelowMin);
    }
    let entries = history.entries.as_slice();
    let window = &entries[entries.len().saturating_sub(knobs.history_window)..];
    let degraded = window
        .iter()
        .filter(|entry| below_min(u64::from(entry.fidelity_q4)))
        .count();
    if degraded >= knobs.max_degradations {
        reason_codes.push(RewriteReason::DegradationRuleTriggered);
    }
    let verdict = if reason_codes.is_empty() {
        RewriteVerdict::Admit
    } else {
        RewriteVerdict::Discard
    };

    let mut evaluation = MemoryRewriteEvaluation {
        proposal: proposal.clone(),
        metric_revision: metric.revision,
        metric_digest: metric.specification_digest(),
        fidelity_q4,
        signal_count,
        token_count: proposal.token_count,
        signal_per_token_q4,
        verdict,
        reason_codes,
        receipt_digest: String::new(),
    };
    evaluation.receipt_digest = evaluation.compute_receipt_digest();
    Ok(evaluation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::memory_rewrite::{
        HistoryEntry, RevisionRef, MEMORY_REWRITE_SCHEMA_VERSION,
    };

    fn knobs() -> MemoryRewriteKnobs {
        MemoryRewriteKnobs {
            min_fidelity_micros: 950_000,
            min_signal_per_token_micros: 20_000,
            history_window: 5,
            max_degradations: 2,
        }
    }

    fn signals(count: usize) -> Vec<String> {
        (0..count).map(|i| format!("signal number {i}")).collect()
    }

    fn proposal(active: usize, kept: usize, extra: usize, tokens: u64) -> MemoryRewriteProposal {
        let mut candidate = signals(active)[..kept].to_vec();
        candidate.extend((0..extra).map(|i| format!("new signal {i}")));
        MemoryRewriteProposal {
            schema_version: MEMORY_REWRITE_SCHEMA_VERSION,
            tenant_id: "tenant-a".into(),
            idempotency_key: "key-1".into(),
            record_id: "memory:1".into(),
            active: RevisionRef {
                revision: 4,
                digest: "a".repeat(64),
            },
            candidate: RevisionRef {
                revision: 5,
                digest: "b".repeat(64),
            },
            active_signals: signals(active),
            candidate_signals: candidate,
            token_count: tokens,
            metric_id: "memory_fidelity_set_overlap".into(),
            policy: "c".repeat(64),
            history: RewriteHistory::sealed(Vec::new()).unwrap(),
        }
    }

    fn with_history(mut p: MemoryRewriteProposal, q4: &[u32]) -> MemoryRewriteProposal {
        let entries = q4
            .iter()
            .enumerate()
            .map(|(i, f)| HistoryEntry {
                revision: i as u64,
                fidelity_q4: *f,
            })
            .collect();
        p.history = RewriteHistory::sealed(entries).unwrap();
        p
    }

    // spec: MEDULLA-DECISION-ENGINE-R141.100
    #[test]
    fn fidelity_is_set_overlap_rounded_half_to_even() {
        let fidelity = |active, kept| {
            evaluate_rewrite(&proposal(active, kept, 0, 100), &knobs(), 4)
                .unwrap()
                .fidelity_q4
        };
        assert_eq!(fidelity(3, 3), 10_000);
        assert_eq!(fidelity(3, 1), 3_333);
        assert_eq!(fidelity(3, 2), 6_667);
        assert_eq!(fidelity(3, 0), 0);
        // Exact halves round to the even neighbour: 312.5 -> 312, 937.5 -> 938,
        // 1562.5 -> 1562.
        assert_eq!(fidelity(32, 1), 312);
        assert_eq!(fidelity(32, 3), 938);
        assert_eq!(fidelity(32, 5), 1_562);

        // Signals are compared in normal form and counted once.
        let mut p = proposal(2, 0, 0, 10);
        p.active_signals = vec!["Caf\u{e9} Open".into(), "second  signal".into()];
        p.candidate_signals = vec![
            "cafe\u{301}   open".into(),
            "  SECOND signal ".into(),
            "second signal".into(),
            "   ".into(),
        ];
        let evaluation = evaluate_rewrite(&p, &knobs(), 4).unwrap();
        assert_eq!(evaluation.fidelity_q4, 10_000);
        assert_eq!(evaluation.signal_count, 2);
        // signal_per_token = 2 / 10, rounded to four decimals.
        assert_eq!(evaluation.signal_per_token_q4, 2_000);
        // 1/3 = 0.3333, 2/3 = 0.6667 (half-even leaves non-halves alone).
        let efficiency = evaluate_rewrite(&proposal(3, 3, 0, 3), &knobs(), 4).unwrap();
        assert_eq!(efficiency.signal_per_token_q4, 10_000);
        let third = evaluate_rewrite(&proposal(1, 1, 0, 3), &knobs(), 4).unwrap();
        assert_eq!(third.signal_per_token_q4, 3_333);
        let half_even = evaluate_rewrite(&proposal(1, 1, 0, 32), &knobs(), 4).unwrap();
        assert_eq!(half_even.signal_per_token_q4, 312);

        // Each refusal.
        let base = proposal(4, 4, 0, 100);
        let mut unknown_metric = base.clone();
        unknown_metric.metric_id = "memory_fidelity_other".into();
        assert_eq!(
            evaluate_rewrite(&unknown_metric, &knobs(), 4),
            Err(MEMORY_METRIC_UNRESOLVED)
        );
        let mut no_policy = base.clone();
        no_policy.policy = String::new();
        assert_eq!(
            evaluate_rewrite(&no_policy, &knobs(), 4),
            Err(MEMORY_POLICY_MISSING)
        );
        let mut no_history = base.clone();
        no_history.history.history_digest = String::new();
        assert_eq!(
            evaluate_rewrite(&no_history, &knobs(), 4),
            Err(MEMORY_HISTORY_MISSING)
        );
        let mut forged_history = with_history(base.clone(), &[9_900]);
        forged_history.history.entries =
            medulla_types::contract::BoundedVec::new(vec![HistoryEntry {
                revision: 0,
                fidelity_q4: 10_000,
            }])
            .unwrap();
        assert_eq!(
            evaluate_rewrite(&forged_history, &knobs(), 4),
            Err(MEMORY_HISTORY_MISSING)
        );
        assert_eq!(
            evaluate_rewrite(&proposal(4, 4, 0, 0), &knobs(), 4),
            Err(MEMORY_TOKEN_DENOMINATOR_ZERO)
        );
        let mut no_active = base.clone();
        no_active.active_signals = vec!["  ".into()];
        assert_eq!(
            evaluate_rewrite(&no_active, &knobs(), 4),
            Err(MEMORY_ACTIVE_SIGNALS_EMPTY)
        );
        assert_eq!(
            evaluate_rewrite(&base, &knobs(), 5),
            Err(MEMORY_REVISION_STALE)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R141.100
    #[test]
    fn verdicts_name_every_reason_and_seal_a_receipt() {
        // 20 signals kept in full, enough per token: Admit.
        let admit = evaluate_rewrite(&proposal(20, 20, 0, 100), &knobs(), 4).unwrap();
        assert_eq!(admit.verdict, RewriteVerdict::Admit);
        assert!(admit.reason_codes.is_empty());
        assert_eq!(admit.receipt_digest, admit.compute_receipt_digest());
        assert_eq!(admit.receipt_digest.len(), 64);
        assert_eq!(admit.metric_revision, 1);
        let again = evaluate_rewrite(&proposal(20, 20, 0, 100), &knobs(), 4).unwrap();
        assert_eq!(admit, again);

        // 18 of 20 kept is 0.9: fidelity below 0.95.
        let low = evaluate_rewrite(&proposal(20, 18, 0, 100), &knobs(), 4).unwrap();
        assert_eq!(low.verdict, RewriteVerdict::Discard);
        assert_eq!(low.reason_codes, vec![RewriteReason::FidelityBelowMin]);
        // Exactly the minimum is not below it: 19 of 20 is 0.95.
        let edge = evaluate_rewrite(&proposal(20, 19, 0, 100), &knobs(), 4).unwrap();
        assert_eq!(edge.verdict, RewriteVerdict::Admit);

        // 20 signals over 2000 tokens is 0.01 < 0.02.
        let sparse = evaluate_rewrite(&proposal(20, 20, 0, 2_000), &knobs(), 4).unwrap();
        assert_eq!(
            sparse.reason_codes,
            vec![RewriteReason::SignalEfficiencyBelowMin]
        );

        // Two below-minimum entries within the last five trigger the rule; two
        // outside the window do not.
        let degraded = with_history(proposal(20, 20, 0, 100), &[9_000, 9_900, 9_000, 9_900]);
        let degraded = evaluate_rewrite(&degraded, &knobs(), 4).unwrap();
        assert_eq!(degraded.verdict, RewriteVerdict::Discard);
        assert_eq!(
            degraded.reason_codes,
            vec![RewriteReason::DegradationRuleTriggered]
        );
        let old = with_history(
            proposal(20, 20, 0, 100),
            &[9_000, 9_000, 9_900, 9_900, 9_900, 9_900, 9_900],
        );
        assert_eq!(
            evaluate_rewrite(&old, &knobs(), 4).unwrap().verdict,
            RewriteVerdict::Admit
        );

        // Every reason at once, in declared order.
        let all = with_history(proposal(20, 10, 0, 5_000), &[1, 1]);
        let all = evaluate_rewrite(&all, &knobs(), 4).unwrap();
        assert_eq!(
            all.reason_codes,
            vec![
                RewriteReason::FidelityBelowMin,
                RewriteReason::SignalEfficiencyBelowMin,
                RewriteReason::DegradationRuleTriggered,
            ]
        );

        // The premises name every input and the knobs.
        let premises = admit.premises(&knobs());
        assert_eq!(premises[0], "memory:record=memory:1");
        assert!(premises.contains(&"memory:fidelity_q4=10000".to_string()));
        assert!(premises.contains(&"memory:metric=memory_fidelity_set_overlap@1".to_string()));
        assert!(premises.contains(&"knob:memory_rewrite.min_fidelity=0.950000".to_string()));
        assert_eq!(premises.len(), 11);
    }
}
