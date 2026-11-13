// CONCEPT:MEDULLA-KG.domains.finance-compute.futures-roll — fixed-point roll of a
// futures position from an expiring contract to its successor
// (MEDULLA-FINANCE-PRIMITIVES-R013.2.5).
//
// A long position of `quantity_ticks` closes the expiring contract at its
// published settlement and opens the successor at its published settlement.
// Roll cash is `quantity * multiplier * (expiring_settlement - successor_settlement)`;
// the rolled position value is `quantity * multiplier * successor_settlement`.
// Prices must sit on the tick grid of their contract. Money and quantity are
// fixed-point ticks at `lot_accounting::SCALE`; the multiplier is a whole count.

use std::fmt;

use crate::lot_accounting::{basis_numerator, Lot, SCALE};

/// One contract's published specification and settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractSpec {
    /// Published settlement price in ticks.
    pub settlement_ticks: i64,
    /// Expiry as UTC nanoseconds.
    pub expiry_ns: i64,
    /// Minimum price increment in ticks (positive).
    pub tick_size_ticks: i64,
    /// Contract multiplier, a positive whole count.
    pub multiplier: i64,
}

/// The cash and position value of one roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollOutcome {
    /// Cash received (positive) or paid (negative) by closing the expiring
    /// contract and opening the successor.
    pub roll_cash_ticks: i64,
    /// Notional value of the position held in the successor contract.
    pub position_value_ticks: i64,
}

/// Why a roll was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollError {
    NonPositiveQuantity,
    NonPositiveMultiplier,
    NonPositiveTickSize,
    /// A settlement is not a multiple of its contract's tick size.
    OffTickSettlement,
    /// The successor does not expire after the expiring contract.
    SuccessorNotAfterExpiry,
    ArithmeticOverflow,
}

impl fmt::Display for RollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            RollError::NonPositiveQuantity => "roll quantity is not positive",
            RollError::NonPositiveMultiplier => "contract multiplier is not positive",
            RollError::NonPositiveTickSize => "contract tick size is not positive",
            RollError::OffTickSettlement => "settlement is off its contract's tick grid",
            RollError::SuccessorNotAfterExpiry => {
                "successor does not expire after the expiring contract"
            }
            RollError::ArithmeticOverflow => "roll arithmetic overflowed",
        };
        f.write_str(text)
    }
}

impl std::error::Error for RollError {}

fn check_spec(spec: &ContractSpec) -> Result<(), RollError> {
    if spec.multiplier <= 0 {
        return Err(RollError::NonPositiveMultiplier);
    }
    if spec.tick_size_ticks <= 0 {
        return Err(RollError::NonPositiveTickSize);
    }
    if spec.settlement_ticks <= 0 || spec.settlement_ticks % spec.tick_size_ticks != 0 {
        return Err(RollError::OffTickSettlement);
    }
    Ok(())
}

/// `quantity_ticks * price_ticks / SCALE * multiplier`, rounded half up at the
/// division, as `i128` money ticks.
fn extend(numerator: i128, multiplier: i64) -> Result<i128, RollError> {
    let scale = SCALE as i128;
    let rounded = if numerator >= 0 {
        (numerator + scale / 2) / scale
    } else {
        -((-numerator + scale / 2) / scale)
    };
    rounded
        .checked_mul(multiplier as i128)
        .ok_or(RollError::ArithmeticOverflow)
}

/// Roll `quantity_ticks` of a long position from `expiring` to `successor`.
pub fn roll_position(
    expiring: &ContractSpec,
    successor: &ContractSpec,
    quantity_ticks: i64,
) -> Result<RollOutcome, RollError> {
    if quantity_ticks <= 0 {
        return Err(RollError::NonPositiveQuantity);
    }
    check_spec(expiring)?;
    check_spec(successor)?;
    if successor.expiry_ns <= expiring.expiry_ns {
        return Err(RollError::SuccessorNotAfterExpiry);
    }
    let closed = Lot {
        opened_by: 0,
        remaining_ticks: quantity_ticks,
        unit_cost_ticks: expiring.settlement_ticks,
    };
    let opened = Lot {
        opened_by: 0,
        remaining_ticks: quantity_ticks,
        unit_cost_ticks: successor.settlement_ticks,
    };
    let proceeds = extend(basis_numerator(&[closed]), expiring.multiplier)?;
    let cost = extend(basis_numerator(&[opened]), successor.multiplier)?;
    let to_i64 = |value: i128| i64::try_from(value).map_err(|_| RollError::ArithmeticOverflow);
    Ok(RollOutcome {
        roll_cash_ticks: to_i64(proceeds - cost)?,
        position_value_ticks: to_i64(cost)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(settlement: i64, expiry: i64) -> ContractSpec {
        ContractSpec {
            settlement_ticks: settlement * SCALE / 100,
            expiry_ns: expiry,
            tick_size_ticks: SCALE / 4,
            multiplier: 50,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.5
    #[test]
    fn a_roll_closes_at_the_expiring_and_opens_at_the_successor_settlement() {
        let outcome = roll_position(&spec(450_000, 1), &spec(450_525, 2), 2 * SCALE).unwrap();
        // 2 contracts x 50 x (4500.00 - 4505.25) = -525.00
        assert_eq!(outcome.roll_cash_ticks, -525 * SCALE);
        assert_eq!(outcome.position_value_ticks, 450_525 * SCALE);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.5
    #[test]
    fn off_tick_settlements_and_a_non_later_successor_are_refused() {
        let mut off = spec(450_000, 1);
        off.settlement_ticks += 1;
        assert_eq!(
            roll_position(&off, &spec(450_525, 2), SCALE),
            Err(RollError::OffTickSettlement)
        );
        assert_eq!(
            roll_position(&spec(450_000, 2), &spec(450_525, 2), SCALE),
            Err(RollError::SuccessorNotAfterExpiry)
        );
        assert_eq!(
            roll_position(&spec(450_000, 1), &spec(450_525, 2), 0),
            Err(RollError::NonPositiveQuantity)
        );
    }
}
