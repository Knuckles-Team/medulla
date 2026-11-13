// CONCEPT:MEDULLA-KG.domains.finance-compute.leverage — Leverage Order Governance
//
// Live-order eligibility boundary (MEDULLA-FINANCE-PRIMITIVES-R010.1): a live
// leveraged order is eligible only with a matching per-instrument policy and
// a designated approver. Analysis defaults to paper mode, so a paper-mode
// order is never eligible for live authorization. CFDs are flagged
// unavailable for US retail accounts regardless of any policy. Margin-call,
// liquidation and position-sizing scenario simulation are later slices
// (R010.2+).

use serde::{Deserialize, Serialize};

/// Whether an order is analyzed only (paper; never placed or authorized) or
/// live. Paper is the default for every new analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderMode {
    Paper,
    Live,
}

/// A per-instrument policy authorizing live leveraged orders, naming its
/// designated approvers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeveragePolicy {
    pub instrument_id: String,
    pub approvers: Vec<String>,
    /// CFDs stay unavailable for US retail accounts even under this policy.
    pub cfd_unavailable_us_retail: bool,
}

/// Why a live leveraged order was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LiveOrderRefusal {
    /// The default: an order is paper-mode unless explicitly made live.
    NotLive,
    NoPolicyForInstrument {
        instrument_id: String,
    },
    ApproverNotDesignated {
        approver: String,
    },
    CfdUnavailableForUsRetail {
        instrument_id: String,
    },
}

/// Decide whether a live leveraged order is eligible.
///
/// A paper-mode order is never eligible: there is nothing to authorize. A
/// live order is eligible only when a policy names both the instrument and
/// the acting approver, and a CFD for a US retail account is refused even
/// under a matching policy.
pub fn live_order_eligibility(
    mode: OrderMode,
    instrument_id: &str,
    approver: &str,
    is_cfd: bool,
    is_us_retail: bool,
    policy: Option<&LeveragePolicy>,
) -> Result<(), LiveOrderRefusal> {
    if mode == OrderMode::Paper {
        return Err(LiveOrderRefusal::NotLive);
    }
    let policy = match policy {
        Some(policy) if policy.instrument_id == instrument_id => policy,
        _ => {
            return Err(LiveOrderRefusal::NoPolicyForInstrument {
                instrument_id: instrument_id.to_string(),
            })
        }
    };
    if is_cfd && is_us_retail && policy.cfd_unavailable_us_retail {
        return Err(LiveOrderRefusal::CfdUnavailableForUsRetail {
            instrument_id: instrument_id.to_string(),
        });
    }
    if !policy.approvers.iter().any(|a| a == approver) {
        return Err(LiveOrderRefusal::ApproverNotDesignated {
            approver: approver.to_string(),
        });
    }
    Ok(())
}

/// One whole unit is this many ticks (matches `medulla_finance::golden_fixture::SCALE`).
const TICKS_PER_UNIT: i128 = 100_000_000;

/// The explicit assumptions of a daily-reset path (MEDULLA-FINANCE-PRIMITIVES-R013.2.4):
/// the target multiple, the annual expense ratio charged evenly per session, and the number
/// of sessions that make a year. Rounding is half away from zero after every session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyResetAssumptions {
    /// The daily-reset multiple in ticks (`3 * SCALE` is 3x; negative is inverse).
    pub leverage_ticks: i64,
    /// The annual expense ratio in ticks of one whole unit (`0.0095` is 950_000).
    pub expense_ratio_ticks: i64,
    /// Sessions per year the annual ratio is spread over; never zero.
    pub sessions_per_year: u32,
}

/// Why a daily-reset path was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DailyResetRefusal {
    NoSessions,
    SessionsPerYearZero,
    InitialNavNotPositive { initial_nav_ticks: i64 },
    Overflow { session: usize },
}

impl std::fmt::Display for DailyResetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSessions => write!(f, "a daily-reset path needs at least one session"),
            Self::SessionsPerYearZero => write!(f, "sessions_per_year must be positive"),
            Self::InitialNavNotPositive { initial_nav_ticks } => {
                write!(f, "initial NAV {initial_nav_ticks} ticks is not positive")
            }
            Self::Overflow { session } => write!(f, "NAV overflowed at session {session}"),
        }
    }
}

/// The NAV after every session of a daily-reset path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyResetPath {
    /// NAV in ticks after each session, in order.
    pub nav_ticks: Vec<i64>,
    /// The last NAV (also the last element of `nav_ticks`).
    pub final_nav_ticks: i64,
}

/// Half away from zero division of `numerator` by a positive `divisor`.
fn half_away(numerator: i128, divisor: i128) -> i128 {
    if numerator >= 0 {
        (numerator + divisor / 2) / divisor
    } else {
        -((-numerator + divisor / 2) / divisor)
    }
}

/// Replay a leveraged fund's daily-reset NAV over the underlying's session returns, in
/// fixed point (MEDULLA-FINANCE-PRIMITIVES-R013.2.4). Each session the fund earns the
/// multiple of that session's underlying return less one session of the expense ratio:
/// `nav = round(nav * (1 + leverage * r - ratio / sessions_per_year))`, rounded half away
/// from zero to a tick every session. A session factor at or below zero wipes the fund
/// out (NAV 0, and it stays 0). `returns_ticks` are the underlying's simple session
/// returns in ticks of one whole unit (`-0.10` is `-10_000_000`).
pub fn daily_reset_path(
    initial_nav_ticks: i64,
    returns_ticks: &[i64],
    assumptions: &DailyResetAssumptions,
) -> Result<DailyResetPath, DailyResetRefusal> {
    if returns_ticks.is_empty() {
        return Err(DailyResetRefusal::NoSessions);
    }
    if assumptions.sessions_per_year == 0 {
        return Err(DailyResetRefusal::SessionsPerYearZero);
    }
    if initial_nav_ticks <= 0 {
        return Err(DailyResetRefusal::InitialNavNotPositive { initial_nav_ticks });
    }
    let fee = half_away(
        i128::from(assumptions.expense_ratio_ticks),
        i128::from(assumptions.sessions_per_year),
    );
    let mut nav = i128::from(initial_nav_ticks);
    let mut path = Vec::with_capacity(returns_ticks.len());
    for (session, r) in returns_ticks.iter().enumerate() {
        let gross = half_away(
            i128::from(assumptions.leverage_ticks) * i128::from(*r),
            TICKS_PER_UNIT,
        );
        let factor = (TICKS_PER_UNIT + gross - fee).max(0);
        nav = half_away(nav * factor, TICKS_PER_UNIT);
        path.push(i64::try_from(nav).map_err(|_| DailyResetRefusal::Overflow { session })?);
    }
    let final_nav_ticks = *path.last().unwrap_or(&0);
    Ok(DailyResetPath {
        nav_ticks: path,
        final_nav_ticks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> LeveragePolicy {
        LeveragePolicy {
            instrument_id: "futures:ES".to_string(),
            approvers: vec!["user:alice".to_string()],
            cfd_unavailable_us_retail: true,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R010.1
    #[test]
    fn paper_mode_is_never_eligible_for_a_live_order() {
        let err = live_order_eligibility(
            OrderMode::Paper,
            "futures:ES",
            "user:alice",
            false,
            false,
            Some(&policy()),
        )
        .unwrap_err();
        assert_eq!(err, LiveOrderRefusal::NotLive);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R010.1
    #[test]
    fn a_live_order_without_a_matching_policy_is_refused() {
        let err = live_order_eligibility(
            OrderMode::Live,
            "futures:ES",
            "user:alice",
            false,
            false,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            LiveOrderRefusal::NoPolicyForInstrument { .. }
        ));
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R010.1
    #[test]
    fn a_live_order_with_an_undesignated_approver_is_refused() {
        let err = live_order_eligibility(
            OrderMode::Live,
            "futures:ES",
            "user:mallory",
            false,
            false,
            Some(&policy()),
        )
        .unwrap_err();
        assert_eq!(
            err,
            LiveOrderRefusal::ApproverNotDesignated {
                approver: "user:mallory".to_string()
            }
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R010.1
    #[test]
    fn a_cfd_is_refused_for_us_retail_regardless_of_policy() {
        let err = live_order_eligibility(
            OrderMode::Live,
            "futures:ES",
            "user:alice",
            true,
            true,
            Some(&policy()),
        )
        .unwrap_err();
        assert_eq!(
            err,
            LiveOrderRefusal::CfdUnavailableForUsRetail {
                instrument_id: "futures:ES".to_string()
            }
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R010.1
    #[test]
    fn a_live_order_with_a_matching_policy_and_approver_is_eligible() {
        assert!(live_order_eligibility(
            OrderMode::Live,
            "futures:ES",
            "user:alice",
            false,
            false,
            Some(&policy()),
        )
        .is_ok());
    }

    fn assumptions(leverage: i64, ratio: i64) -> DailyResetAssumptions {
        DailyResetAssumptions {
            leverage_ticks: leverage * TICKS_PER_UNIT as i64,
            expense_ratio_ticks: ratio,
            sessions_per_year: 252,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.4
    #[test]
    fn a_two_times_fund_loses_to_volatility_decay() {
        // +10% then -10% on the underlying: 100 -> 120 -> 96 for a 2x fund.
        let path = daily_reset_path(
            100 * TICKS_PER_UNIT as i64,
            &[10_000_000, -10_000_000],
            &assumptions(2, 0),
        )
        .unwrap();
        assert_eq!(
            path.nav_ticks,
            vec![120 * TICKS_PER_UNIT as i64, 96 * TICKS_PER_UNIT as i64]
        );
        assert_eq!(path.final_nav_ticks, 96 * TICKS_PER_UNIT as i64);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.4
    #[test]
    fn a_daily_reset_path_refuses_bad_assumptions_and_wipes_out_below_zero() {
        let one = 100_000_000;
        assert_eq!(
            daily_reset_path(one, &[], &assumptions(2, 0)),
            Err(DailyResetRefusal::NoSessions)
        );
        let mut bad = assumptions(2, 0);
        bad.sessions_per_year = 0;
        assert_eq!(
            daily_reset_path(one, &[1], &bad),
            Err(DailyResetRefusal::SessionsPerYearZero)
        );
        assert!(matches!(
            daily_reset_path(0, &[1], &assumptions(2, 0)),
            Err(DailyResetRefusal::InitialNavNotPositive { .. })
        ));
        // A 3x fund on a -40% session is wiped out and stays at zero.
        let wiped = daily_reset_path(one, &[-40_000_000, 10_000_000], &assumptions(3, 0)).unwrap();
        assert_eq!(wiped.nav_ticks, vec![0, 0]);
    }
}
