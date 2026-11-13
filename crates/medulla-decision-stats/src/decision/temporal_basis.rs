//! Temporal basis rules (MEDULLA-DECISION-ENGINE-R146.2).
//!
//! A first pass that finds too few rows may have read the wrong clock. The
//! kernel detects when the three clocks of a node disagree, picks the basis to
//! retry on and builds the widened fallback windows. All functions are pure;
//! the caller runs the fallback query.

use medulla_decision_types::decision::temporal_query::{TemporalContext, TemporalKnobs, TimeBasis};

/// Milliseconds in one day.
pub const DAY_MS: i64 = 86_400_000;

/// The dates present in a context, in basis order (event, narrative, storage).
fn present(context: &TemporalContext) -> Vec<i64> {
    [
        context.event_ms,
        context.narrative_ms,
        Some(context.storage_ms),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Whether any two present dates differ by more than `gap_days`.
pub fn confusion(context: &TemporalContext, gap_days: u32) -> bool {
    let dates = present(context);
    let limit = i64::from(gap_days).saturating_mul(DAY_MS);
    dates
        .iter()
        .any(|a| dates.iter().any(|b| a.abs_diff(*b) > limit as u64))
}

/// Whether the first pass is insufficient: fewer rows than `min_hits`.
pub fn needs_fallback(first_pass_rows: u32, min_hits: u32) -> bool {
    first_pass_rows < min_hits
}

/// The basis to retry on: the first present date wins, in the order event,
/// then narrative, then storage.
pub fn choose_basis(context: &TemporalContext) -> TimeBasis {
    if context.event_ms.is_some() {
        TimeBasis::Event
    } else if context.narrative_ms.is_some() {
        TimeBasis::Narrative
    } else {
        TimeBasis::Storage
    }
}

/// The window of retry `retry_index` (0 is the first fallback): the original
/// window widened by `widen_days` on each side, once more for each later retry.
pub fn fallback_window(from_ms: i64, to_ms: i64, widen_days: u32, retry_index: u32) -> (i64, i64) {
    let widen = i64::from(widen_days)
        .saturating_mul(i64::from(retry_index) + 1)
        .saturating_mul(DAY_MS);
    (from_ms.saturating_sub(widen), to_ms.saturating_add(widen))
}

/// At most `max_retries` fallback windows, each widening again by the same amount.
pub fn fallback_windows(from_ms: i64, to_ms: i64, knobs: &TemporalKnobs) -> Vec<(i64, i64)> {
    (0..knobs.max_retries)
        .map(|retry| fallback_window(from_ms, to_ms, knobs.widen_days, retry))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(storage: i64, event: Option<i64>, narrative: Option<i64>) -> TemporalContext {
        TemporalContext {
            storage_ms: storage,
            event_ms: event,
            narrative_ms: narrative,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R146.2
    #[test]
    fn confusion_detection() {
        let day = DAY_MS;
        // Equal dates never confuse; a one-day gap is within the default gap of 1.
        assert!(!confusion(
            &context(10 * day, Some(10 * day), Some(10 * day)),
            1
        ));
        assert!(!confusion(&context(10 * day, Some(9 * day), None), 1));
        // An eight-day gap does.
        assert!(confusion(&context(18 * day, Some(10 * day), None), 1));
        assert!(confusion(
            &context(10 * day, Some(10 * day), Some(18 * day)),
            1
        ));
        // A gap of exactly the limit is not a confusion; one millisecond over is.
        assert!(!confusion(&context(11 * day, Some(10 * day), None), 1));
        assert!(confusion(&context(11 * day + 1, Some(10 * day), None), 1));
        // A gap of 0 flags any difference; missing dates are not compared.
        assert!(confusion(&context(10 * day + 1, Some(10 * day), None), 0));
        assert!(!confusion(&context(10 * day, None, None), 0));

        // First-pass sufficiency.
        assert!(needs_fallback(2, 3));
        assert!(!needs_fallback(3, 3));

        // Basis order: event, narrative, storage.
        assert_eq!(
            choose_basis(&context(1, Some(2), Some(3))),
            TimeBasis::Event
        );
        assert_eq!(
            choose_basis(&context(1, None, Some(3))),
            TimeBasis::Narrative
        );
        assert_eq!(choose_basis(&context(1, None, None)), TimeBasis::Storage);

        // Window arithmetic: widen each side, and widen again per retry.
        assert_eq!(
            fallback_window(100 * day, 110 * day, 7, 0),
            (93 * day, 117 * day)
        );
        assert_eq!(
            fallback_window(100 * day, 110 * day, 7, 1),
            (86 * day, 124 * day)
        );
        let knobs = TemporalKnobs {
            confusion_gap_days: 1,
            min_hits: 3,
            widen_days: 7,
            max_retries: 2,
        };
        assert_eq!(
            fallback_windows(100 * day, 110 * day, &knobs),
            vec![(93 * day, 117 * day), (86 * day, 124 * day)]
        );
        assert!(fallback_windows(
            0,
            1,
            &TemporalKnobs {
                max_retries: 0,
                ..knobs
            }
        )
        .is_empty());
        assert_eq!(
            fallback_window(i64::MIN, i64::MAX, 365, 3),
            (i64::MIN, i64::MAX)
        );
        for spec in medulla_decision_types::decision::temporal_query::KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
    }
}
