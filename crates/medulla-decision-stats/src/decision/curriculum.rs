//! Curriculum promotion kernel (MEDULLA-DECISION-ENGINE-R751.754).
//!
//! [`promotion_verdict`] promotes a stage when the success rate of its latest
//! full window of independently evaluated outcomes reaches the stage policy's
//! threshold. A window that is not full holds. The rate comparison is exact
//! integer arithmetic on `Q32`; no float, no clock, no store.

use medulla_decision_types::decision::curriculum::{
    CurriculumStage, PromotionPolicy, PromotionReading, PromotionVerdict,
};
use medulla_decision_types::decision::numeric::QuantScaleTag;

/// The verdict for `stage` from its `outcomes` (newest first, `true` is a
/// success) over a window of `window_runs` runs.
pub fn promotion_verdict(
    stage: &CurriculumStage,
    policy: &PromotionPolicy,
    outcomes: &[bool],
    window_runs: u32,
) -> Result<PromotionReading, String> {
    let _ = stage;
    if policy.promote_threshold.scale != QuantScaleTag::Q32 {
        return Err("promote_threshold is off scale Q32".to_string());
    }
    let threshold = i128::from(policy.promote_threshold.value);
    if !(0..=1i128 << 32).contains(&threshold) {
        return Err("promote_threshold is outside [0, 1]".to_string());
    }
    let window: Vec<bool> = outcomes
        .iter()
        .copied()
        .take(window_runs as usize)
        .collect();
    let runs = u32::try_from(window.len()).unwrap_or(u32::MAX);
    let successes = u32::try_from(window.iter().filter(|ok| **ok).count()).unwrap_or(u32::MAX);
    let full = runs >= window_runs;
    let cleared = full && i128::from(successes) << 32 >= threshold * i128::from(runs);
    let verdict = if cleared {
        PromotionVerdict::Promote {
            next_stage_id: policy.next_stage_id.clone(),
        }
    } else {
        PromotionVerdict::Hold
    };
    Ok(PromotionReading {
        verdict,
        successes,
        runs,
        window_runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::numeric::QuantisedValue;

    fn stage() -> CurriculumStage {
        CurriculumStage {
            stage_id: "short".into(),
            horizon_steps: 8,
            window_runs: 5,
        }
    }

    fn policy(threshold: i64) -> PromotionPolicy {
        PromotionPolicy {
            promote_threshold: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: threshold,
            },
            next_stage_id: "long".into(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R751.754
    #[test]
    fn promotion_needs_a_full_window() {
        let three_quarters = 3 << 30;
        // Four runs, all successes: the window is not full, so it holds.
        let short = promotion_verdict(&stage(), &policy(three_quarters), &[true; 4], 5).unwrap();
        assert_eq!(short.verdict, PromotionVerdict::Hold);
        assert_eq!((short.successes, short.runs), (4, 4));
        // A full window at exactly the threshold promotes.
        let outcomes = [true, true, true, false, true, false, false];
        let full = promotion_verdict(&stage(), &policy(3 << 30), &outcomes[..4], 4).unwrap();
        assert_eq!(
            full.verdict,
            PromotionVerdict::Promote {
                next_stage_id: "long".into()
            }
        );
        // Older runs beyond the window do not count.
        let window = promotion_verdict(&stage(), &policy(3 << 30), &outcomes, 4).unwrap();
        assert_eq!((window.successes, window.runs), (3, 4));
        // One short of the threshold holds.
        let below =
            promotion_verdict(&stage(), &policy(3 << 30), &[true, true, false, false], 4).unwrap();
        assert_eq!(below.verdict, PromotionVerdict::Hold);
        // A threshold outside [0, 1] is refused.
        assert!(promotion_verdict(&stage(), &policy(2 << 32), &[true; 5], 5).is_err());
    }
}
