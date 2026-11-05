//! Price-action pattern classification (MEDULLA-FINANCE-PRIMITIVES-R750.751).
//!
//! A pure function over parallel `open`, `high`, `low`, `close` slices. Plain `f64`
//! in a fixed iteration order, so golden vectors replay on every target. Input
//! validation (equal lengths, finite values, `high >= low`, at least two bars) is the
//! served caller's refusal; `classify` assumes a valid window.

use medulla_result_types::compute_result::finance::{PatternKind, PatternMatch};

/// The three tunable thresholds of the classifier, read from the request-time
/// policy (the `pattern.*` server knobs), never from constants at the call site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatternKnobs {
    /// A doji body is at most this fraction of the bar range (`pattern.doji_body_max`).
    pub doji_body_max: f64,
    /// A hammer or shooting-star shadow is at least this multiple of the body
    /// (`pattern.shadow_ratio_min`).
    pub shadow_ratio_min: f64,
    /// The opposite shadow is at most this fraction of the range (`pattern.wick_max`).
    pub wick_max: f64,
}

impl Default for PatternKnobs {
    fn default() -> Self {
        Self {
            doji_body_max: 0.1,
            shadow_ratio_min: 2.0,
            wick_max: 0.1,
        }
    }
}

fn body_over_range(open: f64, high: f64, low: f64, close: f64) -> f64 {
    let range = high - low;
    if range > 0.0 {
        ((close - open).abs() / range).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Classify every bar. The output is ordered by index, then enum order
/// (`hammer`, `shooting_star`, `bullish_engulfing`, `bearish_engulfing`, `doji`,
/// `inside_bar`). Strength is `body / range` in `[0, 1]`; for a doji it is
/// `1 - body / range`.
pub fn classify(
    open: &[f64],
    high: &[f64],
    low: &[f64],
    close: &[f64],
    knobs: &PatternKnobs,
) -> Vec<PatternMatch> {
    let n = open.len().min(high.len()).min(low.len()).min(close.len());
    let mut out = Vec::new();
    for i in 0..n {
        let (o, h, l, c) = (open[i], high[i], low[i], close[i]);
        let range = h - l;
        let body = (c - o).abs();
        let lower = o.min(c) - l;
        let upper = h - o.max(c);
        let strength = body_over_range(o, h, l, c);
        let mut push = |kind: PatternKind, strength: f64| {
            out.push(PatternMatch {
                index: i,
                kind,
                strength,
            });
        };
        if body > 0.0 && lower >= knobs.shadow_ratio_min * body && upper <= knobs.wick_max * range {
            push(PatternKind::Hammer, strength);
        }
        if body > 0.0 && upper >= knobs.shadow_ratio_min * body && lower <= knobs.wick_max * range {
            push(PatternKind::ShootingStar, strength);
        }
        if i > 0 {
            let (po, pc) = (open[i - 1], close[i - 1]);
            if pc < po && c > o && o <= pc && c >= po {
                push(PatternKind::BullishEngulfing, strength);
            }
            if pc > po && c < o && o >= pc && c <= po {
                push(PatternKind::BearishEngulfing, strength);
            }
        }
        if range > 0.0 && body <= knobs.doji_body_max * range {
            push(PatternKind::Doji, 1.0 - body / range);
        }
        if i > 0 && h <= high[i - 1] && l >= low[i - 1] {
            push(PatternKind::InsideBar, strength);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(bars: &[(f64, f64, f64, f64)]) -> Vec<PatternMatch> {
        let col = |f: fn(&(f64, f64, f64, f64)) -> f64| bars.iter().map(f).collect::<Vec<_>>();
        classify(
            &col(|b| b.0),
            &col(|b| b.1),
            &col(|b| b.2),
            &col(|b| b.3),
            &PatternKnobs::default(),
        )
    }

    fn one(matches: &[PatternMatch], index: usize, kind: PatternKind, strength: f64) {
        assert_eq!(matches.len(), 1, "{matches:?}");
        assert_eq!(matches[0].index, index);
        assert_eq!(matches[0].kind, kind);
        assert!(
            (matches[0].strength - strength).abs() < 1e-9,
            "{matches:?} vs {strength}"
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R750.751
    #[test]
    fn golden_vectors_per_pattern() {
        one(
            &run(&[(10.0, 11.0, 8.0, 11.0)]),
            0,
            PatternKind::Hammer,
            1.0 / 3.0,
        );
        one(
            &run(&[(11.0, 13.0, 10.0, 10.0)]),
            0,
            PatternKind::ShootingStar,
            1.0 / 3.0,
        );
        one(&run(&[(10.0, 11.0, 9.0, 10.1)]), 0, PatternKind::Doji, 0.95);
        let bullish = run(&[(10.0, 10.5, 8.5, 9.0), (8.8, 11.2, 8.7, 11.0)]);
        one(&bullish, 1, PatternKind::BullishEngulfing, 0.88);
        let bearish = run(&[(9.0, 10.5, 8.5, 10.0), (10.2, 10.6, 8.8, 8.9)]);
        one(&bearish, 1, PatternKind::BearishEngulfing, 1.3 / 1.8);
        let inside = run(&[(10.0, 12.0, 8.0, 11.0), (10.0, 11.0, 9.0, 10.5)]);
        one(&inside, 1, PatternKind::InsideBar, 0.25);
        // A flat bar has no range: nothing matches and no strength divides by zero.
        assert!(run(&[(5.0, 5.0, 5.0, 5.0)]).is_empty());
    }

    #[test]
    fn output_is_ordered_by_index_then_enum_order() {
        let both = run(&[(10.0, 11.0, 8.0, 11.0), (11.0, 13.0, 10.0, 10.0)]);
        let order: Vec<_> = both.iter().map(|m| (m.index, m.kind)).collect();
        assert_eq!(
            order,
            vec![
                (0, PatternKind::Hammer),
                (1, PatternKind::ShootingStar),
                (1, PatternKind::BearishEngulfing),
            ]
        );
    }
}
