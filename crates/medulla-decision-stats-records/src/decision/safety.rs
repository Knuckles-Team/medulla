//! The safety bound evaluator (MEDULLA-DECISION-ENGINE-R001.100.1).
//!
//! Pure and clock-free: the caller passes `now_ms`. A missing, stale or
//! wrong-unit measurement withholds exactly like an exceeded limit. The first
//! violated bound in `bound_id` order names the verdict, so the verdict does
//! not depend on the order the caller listed bounds or measurements in.

use {
    medulla_decision_statistical_types::decision::safety::BoundCheck,
    medulla_decision_statistical_types::decision::safety::BoundOutcome,
    medulla_decision_statistical_types::decision::safety::SafetyBound,
    medulla_decision_statistical_types::decision::safety::SafetyMeasurement,
    medulla_decision_statistical_types::decision::safety::SafetyVerdict,
};

/// Check every bound against the measurements. Returns one [`BoundCheck`] per
/// bound, sorted by `bound_id`, and the verdict.
pub fn evaluate_bounds(
    bounds: &[SafetyBound],
    measurements: &[SafetyMeasurement],
    now_ms: u64,
    max_age_ms: u64,
) -> (Vec<BoundCheck>, SafetyVerdict) {
    let mut ordered: Vec<&SafetyBound> = bounds.iter().collect();
    ordered.sort_by(|a, b| a.bound_id.cmp(&b.bound_id));
    let checks: Vec<BoundCheck> = ordered
        .into_iter()
        .map(|bound| check_one(bound, measurements, now_ms, max_age_ms))
        .collect();
    let verdict = checks
        .iter()
        .find(|c| c.outcome != BoundOutcome::Within)
        .map_or(SafetyVerdict::Pass, |c| SafetyVerdict::Withheld {
            bound_id: c.bound_id.clone(),
            outcome: c.outcome,
            limit_q32: c.limit_q32,
            measured_q32: c.measured_q32,
        });
    (checks, verdict)
}

fn check_one(
    bound: &SafetyBound,
    measurements: &[SafetyMeasurement],
    now_ms: u64,
    max_age_ms: u64,
) -> BoundCheck {
    let found = measurements.iter().find(|m| m.bound_id == bound.bound_id);
    let (outcome, measured) = match found {
        None => (BoundOutcome::MeasurementMissing, None),
        Some(m) if m.unit != bound.unit => (BoundOutcome::UnitMismatch, Some(m.value_q32)),
        Some(m) if now_ms.saturating_sub(m.measured_at_ms) > max_age_ms => {
            (BoundOutcome::MeasurementStale, Some(m.value_q32))
        }
        Some(m) if bound.comparison.holds(m.value_q32, bound.limit_q32) => {
            (BoundOutcome::Within, Some(m.value_q32))
        }
        Some(m) => (BoundOutcome::Exceeded, Some(m.value_q32)),
    };
    BoundCheck {
        bound_id: bound.bound_id.clone(),
        outcome,
        limit_q32: bound.limit_q32,
        measured_q32: measured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_statistical_types::decision::safety::BoundComparison;

    const NOW: u64 = 10_000_000;
    const AGE: u64 = 3_600_000;

    fn bound(id: &str, comparison: BoundComparison, limit: i64) -> SafetyBound {
        SafetyBound {
            bound_id: id.into(),
            revision: 1,
            digest: format!("d-{id}"),
            metric_id: format!("m-{id}"),
            unit: "ms".into(),
            comparison,
            limit_q32: limit,
        }
    }

    fn measure(id: &str, value: i64) -> SafetyMeasurement {
        SafetyMeasurement {
            bound_id: id.into(),
            value_q32: value,
            unit: "ms".into(),
            source_revision: 1,
            measured_at_ms: NOW - 1_000,
        }
    }

    fn outcome(b: SafetyBound, m: Option<SafetyMeasurement>) -> BoundOutcome {
        let ms: Vec<_> = m.into_iter().collect();
        evaluate_bounds(&[b], &ms, NOW, AGE).0[0].outcome
    }

    // spec: MEDULLA-DECISION-ENGINE-R001.100.1
    #[test]
    fn first_violated_bound_withholds() {
        // Each comparison at its boundary: limit 100, values 99 / 100 / 101.
        let cases = [
            (BoundComparison::Lt, [true, false, false]),
            (BoundComparison::Le, [true, true, false]),
            (BoundComparison::Gt, [false, false, true]),
            (BoundComparison::Ge, [false, true, true]),
        ];
        for (cmp, expect) in cases {
            for (value, inside) in [99, 100, 101].into_iter().zip(expect) {
                let got = outcome(bound("b", cmp, 100), Some(measure("b", value)));
                let want = if inside {
                    BoundOutcome::Within
                } else {
                    BoundOutcome::Exceeded
                };
                assert_eq!(got, want, "{cmp:?} at {value}");
            }
        }

        // Missing, unit mismatch and stale measurements each withhold.
        let b = bound("b", BoundComparison::Le, 100);
        assert_eq!(outcome(b.clone(), None), BoundOutcome::MeasurementMissing);
        let mut wrong_unit = measure("b", 1);
        wrong_unit.unit = "s".into();
        assert_eq!(
            outcome(b.clone(), Some(wrong_unit)),
            BoundOutcome::UnitMismatch
        );
        let mut stale = measure("b", 1);
        stale.measured_at_ms = NOW - AGE - 1;
        assert_eq!(
            outcome(b.clone(), Some(stale)),
            BoundOutcome::MeasurementStale
        );
        // Exactly at the age limit is still fresh.
        let mut edge = measure("b", 1);
        edge.measured_at_ms = NOW - AGE;
        assert_eq!(outcome(b, Some(edge)), BoundOutcome::Within);

        // The first violated bound in bound_id order names the verdict,
        // whatever order the caller listed them in.
        let bounds = [
            bound("z", BoundComparison::Le, 10),
            bound("a", BoundComparison::Le, 10),
            bound("m", BoundComparison::Le, 10),
        ];
        let ms = [measure("z", 99), measure("a", 1), measure("m", 99)];
        let (checks, verdict) = evaluate_bounds(&bounds, &ms, NOW, AGE);
        let ids: Vec<_> = checks.iter().map(|c| c.bound_id.as_str()).collect();
        assert_eq!(ids, ["a", "m", "z"]);
        assert_eq!(
            verdict,
            SafetyVerdict::Withheld {
                bound_id: "m".into(),
                outcome: BoundOutcome::Exceeded,
                limit_q32: 10,
                measured_q32: Some(99),
            }
        );

        // All inside: pass. No bounds: pass.
        let ok = [measure("a", 1), measure("m", 2), measure("z", 3)];
        assert_eq!(
            evaluate_bounds(&bounds, &ok, NOW, AGE).1,
            SafetyVerdict::Pass
        );
        assert_eq!(evaluate_bounds(&[], &[], NOW, AGE).1, SafetyVerdict::Pass);
    }
}
