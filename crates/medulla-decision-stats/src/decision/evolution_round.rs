//! Judge versus outcome divergence of a criterion review
//! (MEDULLA-DECISION-ENGINE-R136.103).
//!
//! A rubric judge whose scores keep rising while the independent outcome (tests,
//! user acceptance) does not is being gamed or has drifted. The divergence is
//! the judge-score slope minus the outcome slope, each a least-squares slope
//! over the last `window` runs, computed in exact integer arithmetic so the
//! answer is deterministic.

use medulla_decision_outcome_types::decision::evolution_round::{CriterionRefusal, JudgeRun};

/// The knob values the divergence reads, resolved from the pinned policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DivergenceKnobs {
    /// Runs read, counted back from the newest.
    pub window: usize,
    /// Divergence above this refuses, in millionths of a score unit.
    pub threshold_micros: i64,
}

/// What the divergence read measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DivergenceReading {
    /// Judge slope minus outcome slope, millionths of a score unit per run.
    pub divergence_micros: i64,
    /// How many runs the slopes were fitted over.
    pub runs_read: usize,
}

/// `n * sum(i * y_i) - sum(i) * sum(y_i)`: the least-squares slope numerator
/// over `x = 0..n`, whose denominator `n * sum(i^2) - sum(i)^2` is shared by
/// every series of the same length.
fn slope_numerator(ys: impl Iterator<Item = i64> + Clone, n: i128) -> i128 {
    let sum_y: i128 = ys.clone().map(i128::from).sum();
    let sum_xy: i128 = ys
        .enumerate()
        .map(|(i, y)| (i as i128) * i128::from(y))
        .sum();
    n * sum_xy - (n * (n - 1) / 2) * sum_y
}

/// The divergence of `runs` (oldest first) over the last `knobs.window` runs.
/// Fewer than two runs carry no trend: the divergence is zero. Above
/// `knobs.threshold_micros` the evidence refuses with
/// `JUDGE_OUTCOME_DIVERGENCE`.
pub fn judge_outcome_divergence(
    runs: &[JudgeRun],
    knobs: &DivergenceKnobs,
) -> Result<DivergenceReading, CriterionRefusal> {
    let start = runs.len().saturating_sub(knobs.window);
    let read = &runs[start..];
    if read.len() < 2 {
        return Ok(DivergenceReading {
            divergence_micros: 0,
            runs_read: read.len(),
        });
    }
    let n = read.len() as i128;
    let denominator = n * ((n - 1) * n * (2 * n - 1) / 6) - (n * (n - 1) / 2).pow(2);
    let judge = slope_numerator(read.iter().map(|r| r.judge_score_micros), n);
    let outcome = slope_numerator(read.iter().map(|r| r.outcome_score_micros), n);
    let diff = judge - outcome;
    // Round half away from zero.
    let half = denominator / 2;
    let rounded = if diff >= 0 {
        (diff + half) / denominator
    } else {
        (diff - half) / denominator
    };
    let divergence_micros = i64::try_from(rounded).unwrap_or(i64::MAX);
    if divergence_micros > knobs.threshold_micros {
        return Err(CriterionRefusal::JudgeOutcomeDivergence {
            divergence_micros,
            threshold_micros: knobs.threshold_micros,
        });
    }
    Ok(DivergenceReading {
        divergence_micros,
        runs_read: read.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(pairs: &[(i64, i64)]) -> Vec<JudgeRun> {
        pairs
            .iter()
            .map(|&(j, o)| JudgeRun {
                judge_score_micros: j,
                outcome_score_micros: o,
            })
            .collect()
    }

    const KNOBS: DivergenceKnobs = DivergenceKnobs {
        window: 5,
        threshold_micros: 150_000,
    };

    // spec: MEDULLA-DECISION-ENGINE-R136.103
    #[test]
    fn divergence_is_judge_slope_minus_outcome_slope() {
        // Judge climbs 0.2 per run, outcome is flat: divergence 0.2 per run.
        let climbing = runs(&[
            (100_000, 500_000),
            (300_000, 500_000),
            (500_000, 500_000),
            (700_000, 500_000),
            (900_000, 500_000),
        ]);
        let refused = judge_outcome_divergence(&climbing, &KNOBS).unwrap_err();
        assert_eq!(
            refused,
            CriterionRefusal::JudgeOutcomeDivergence {
                divergence_micros: 200_000,
                threshold_micros: 150_000
            }
        );
        // Both climb together: no divergence.
        let together = runs(&[
            (100_000, 100_000),
            (300_000, 300_000),
            (500_000, 500_000),
            (700_000, 700_000),
            (900_000, 900_000),
        ]);
        let reading = judge_outcome_divergence(&together, &KNOBS).unwrap();
        assert_eq!(reading.divergence_micros, 0);
        assert_eq!(reading.runs_read, 5);
        // Only the last `window` runs are read: an old climb outside it is ignored.
        let mut old = climbing.clone();
        old.extend(runs(&[(900_000, 500_000); 5]));
        let reading = judge_outcome_divergence(&old, &KNOBS).unwrap();
        assert_eq!(reading.divergence_micros, 0);
        // The threshold is exclusive: a divergence equal to it passes.
        let at = DivergenceKnobs {
            window: 5,
            threshold_micros: 200_000,
        };
        assert!(judge_outcome_divergence(&climbing, &at).is_ok());
        // Fewer than two runs carry no trend.
        assert_eq!(
            judge_outcome_divergence(&runs(&[(1, 9)]), &KNOBS)
                .unwrap()
                .divergence_micros,
            0
        );
    }
}
