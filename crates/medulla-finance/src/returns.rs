//! The return engine (MEDULLA-FINANCE-PRIMITIVES-R005.3.1): time-weighted, money-weighted and
//! annualized (XIRR) returns as pure functions over valuation points and external cash flows.
//!
//! Money is fixed-point at [`SCALE`]; the time-weighted return is integer arithmetic in `i128`
//! with half-up rounding. The money-weighted and annualized rates have no closed form, so they
//! are solved by bisection in `f64` with a fixed iteration count and converted to ticks, which
//! makes the same inputs on one build give the same bytes.

use crate::lot_accounting::SCALE;

/// Nanoseconds in a 365-day year, the XIRR year.
const YEAR_NS: f64 = 365.0 * 86_400.0 * 1e9;
/// Lower bound of the rate search.
const RATE_LOW: f64 = -0.999_999;
/// Upper bound of the rate search.
const RATE_HIGH: f64 = 1000.0;
/// Bisection iterations, exactly.
const ITERATIONS: usize = 200;

/// The account's market value, cash included, in the base currency at a UTC instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValuationPoint {
    /// UTC instant in nanoseconds.
    pub ns: i64,
    /// Market value in ticks.
    pub value_ticks: i64,
}

/// An external cash flow into the account: a deposit is positive, a withdrawal negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnFlow {
    /// UTC instant in nanoseconds.
    pub ns: i64,
    /// Amount in ticks.
    pub amount_ticks: i64,
}

/// Why a return could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnError {
    /// Fewer than two points, unsorted points, or an end not after the start.
    NoPeriod,
    /// A period's start value is zero or negative.
    NonPositiveStartValue,
    /// The rate function has the same sign at both search bounds.
    NoSolution,
    /// A tick value left the `i64` range.
    Overflow,
}

/// `n / d` rounded half up, for `d > 0`.
fn half_up(n: i128, d: i128) -> i128 {
    (2 * n + d).div_euclid(2 * d)
}

/// The time-weighted return in ticks. `valuations` holds the window's start point, every
/// valuation inside it and the end point, ascending by `ns`; a flow with `ns` in
/// `(t[i-1], t[i]]` belongs to period `i` and is taken at the period's end.
pub fn twr_ticks(valuations: &[ValuationPoint], flows: &[ReturnFlow]) -> Result<i64, ReturnError> {
    if valuations.len() < 2 || valuations.windows(2).any(|w| w[1].ns < w[0].ns) {
        return Err(ReturnError::NoPeriod);
    }
    let scale = i128::from(SCALE);
    let mut product = scale;
    for pair in valuations.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        if previous.value_ticks <= 0 {
            return Err(ReturnError::NonPositiveStartValue);
        }
        let flow_sum: i128 = flows
            .iter()
            .filter(|f| f.ns > previous.ns && f.ns <= current.ns)
            .map(|f| i128::from(f.amount_ticks))
            .sum();
        let growth = half_up(
            (i128::from(current.value_ticks) - flow_sum) * scale,
            i128::from(previous.value_ticks),
        );
        product = half_up(product * growth, scale);
        if i64::try_from(product).is_err() {
            return Err(ReturnError::Overflow);
        }
    }
    i64::try_from(product - scale).map_err(|_| ReturnError::Overflow)
}

/// Check the window and return its flows with their exponent fractions.
fn window(
    start: ValuationPoint,
    end: ValuationPoint,
    flows: &[ReturnFlow],
) -> Result<Vec<(f64, f64)>, ReturnError> {
    if end.ns <= start.ns {
        return Err(ReturnError::NoPeriod);
    }
    if start.value_ticks <= 0 {
        return Err(ReturnError::NonPositiveStartValue);
    }
    Ok(flows
        .iter()
        .filter(|f| f.ns > start.ns && f.ns <= end.ns)
        .map(|f| (f.amount_ticks as f64, (end.ns - f.ns) as f64))
        .collect())
}

/// Solve `v0 * (1 + r)^e0 + sum(F_k * (1 + r)^e_k) = vT` by bisection and return `r` in ticks.
/// `exponent` maps a nanosecond distance to its exponent.
fn solve(
    start: ValuationPoint,
    end: ValuationPoint,
    flows: &[(f64, f64)],
    exponent: impl Fn(f64) -> f64,
) -> Result<i64, ReturnError> {
    let span = (end.ns - start.ns) as f64;
    let f = |r: f64| -> f64 {
        let base = 1.0 + r;
        let mut total = start.value_ticks as f64 * base.powf(exponent(span));
        for (amount, distance) in flows {
            total += amount * base.powf(exponent(*distance));
        }
        total - end.value_ticks as f64
    };
    let (mut low, mut high) = (RATE_LOW, RATE_HIGH);
    let f_low = f(low);
    if f_low * f(high) > 0.0 {
        return Err(ReturnError::NoSolution);
    }
    for _ in 0..ITERATIONS {
        let mid = (low + high) / 2.0;
        if f(mid) * f_low > 0.0 {
            low = mid;
        } else {
            high = mid;
        }
    }
    let ticks = ((low + high) / 2.0 * SCALE as f64).round();
    if !ticks.is_finite() || ticks.abs() >= i64::MAX as f64 {
        return Err(ReturnError::Overflow);
    }
    Ok(ticks as i64)
}

/// The money-weighted return in ticks: the (not annualized) rate `r` solving
/// `v0 * (1 + r) + sum(F_k * (1 + r)^w_k) = vT` with `w_k = (end - t_k) / (end - start)`.
pub fn mwr_ticks(
    start: ValuationPoint,
    end: ValuationPoint,
    flows: &[ReturnFlow],
) -> Result<i64, ReturnError> {
    let flows = window(start, end, flows)?;
    let span = (end.ns - start.ns) as f64;
    solve(start, end, &flows, |distance| distance / span)
}

/// The annualized return in ticks: the annual rate `x` solving
/// `v0 * (1 + x)^y + sum(F_k * (1 + x)^y_k) = vT` with years measured in 365-day years.
pub fn xirr_ticks(
    start: ValuationPoint,
    end: ValuationPoint,
    flows: &[ReturnFlow],
) -> Result<i64, ReturnError> {
    let flows = window(start, end, flows)?;
    solve(start, end, &flows, |distance| distance / YEAR_NS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(ns: i64, whole: f64) -> ValuationPoint {
        ValuationPoint {
            ns,
            value_ticks: (whole * SCALE as f64).round() as i64,
        }
    }

    fn flow(ns: i64, whole: i64) -> ReturnFlow {
        ReturnFlow {
            ns,
            amount_ticks: whole * SCALE,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.1
    #[test]
    fn twr_matches_hand_computed_two_period_example() {
        let valuations = [point(0, 100.0), point(10, 110.0), point(20, 121.1)];
        let flows = [flow(20, 10)];
        // g1 = 1.10, g2 = (121.1 - 10) / 110 = 1.01, TWR = 0.111.
        assert_eq!(twr_ticks(&valuations, &flows), Ok(11_100_000));
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.1
    #[test]
    fn mwr_and_xirr_solve_a_one_flow_example() {
        let year = (YEAR_NS) as i64;
        let start = point(0, 100.0);
        let end = point(year, 165.0);
        let flows = [flow(year / 2, 50)];
        // 100 s^2 + 50 s - 165 = 0 with s = sqrt(1 + r), solved in closed form.
        let s = (-50.0 + (2500.0f64 + 4.0 * 100.0 * 165.0).sqrt()) / 200.0;
        let expected = ((s * s - 1.0) * SCALE as f64).round() as i64;
        for got in [
            mwr_ticks(start, end, &flows).unwrap(),
            xirr_ticks(start, end, &flows).unwrap(),
        ] {
            assert!((got - expected).abs() <= 1, "{got} vs {expected}");
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.1
    #[test]
    fn no_sign_change_is_no_solution_and_zero_start_is_refused() {
        let end = point(1_000_000_000, 1.0e12);
        assert_eq!(
            mwr_ticks(point(0, 1.0), end, &[]),
            Err(ReturnError::NoSolution)
        );
        assert_eq!(
            mwr_ticks(point(0, 0.0), point(10, 5.0), &[]),
            Err(ReturnError::NonPositiveStartValue)
        );
        assert_eq!(
            xirr_ticks(point(10, 1.0), point(10, 5.0), &[]),
            Err(ReturnError::NoPeriod)
        );
        assert_eq!(twr_ticks(&[point(0, 1.0)], &[]), Err(ReturnError::NoPeriod));
        assert_eq!(
            twr_ticks(&[point(0, 0.0), point(5, 1.0)], &[]),
            Err(ReturnError::NonPositiveStartValue)
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.1
    #[test]
    fn replay_is_byte_identical() {
        let year = YEAR_NS as i64;
        let start = point(0, 100.0);
        let end = point(year, 165.0);
        let flows = [flow(year / 2, 50)];
        let first = (
            mwr_ticks(start, end, &flows),
            xirr_ticks(start, end, &flows),
            twr_ticks(&[start, end], &flows),
        );
        for _ in 0..3 {
            assert_eq!(
                first,
                (
                    mwr_ticks(start, end, &flows),
                    xirr_ticks(start, end, &flows),
                    twr_ticks(&[start, end], &flows)
                )
            );
        }
    }
}
