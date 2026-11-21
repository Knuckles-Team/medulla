//! Reorder point and quantity (MEDULLA-BUSINESS-OPS-R002.5).
//!
//! Kind `reorder`. Input: a reorder policy, the sales of the review window, the stock
//! level, the open purchase order lines and an as-of day. The reorder point is the sales
//! velocity times the lead time plus the safety stock; the velocity is the units sold in
//! the review window over the days of the window, so the product is taken before the
//! division and rounded up. The position is on hand plus on order minus reserved. When the
//! position is below the point the rule reorders the shortfall, rounded up to a multiple of
//! the minimum order quantity. A velocity read from fewer sales than the policy asks for is
//! `insufficient_data`. Integer arithmetic only.

use super::rule_spec::{InputRef, RuleRefusal, RuleTrace};

/// The refusal code for a policy with a field out of range.
pub const REORDER_POLICY_INVALID: &str = "REORDER_POLICY_INVALID";
/// The refusal code for an amount that does not fit in 64 bits.
pub const REORDER_OVERFLOW: &str = "REORDER_OVERFLOW";

/// A `:ReorderPolicy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Days from order to receipt.
    pub lead_time_days: u32,
    /// Units held in reserve.
    pub safety_stock: i64,
    /// The smallest quantity the supplier sells; zero means any quantity.
    pub minimum_order_quantity: i64,
    /// The trailing days of sales the velocity is read over, at least 1.
    pub review_window_days: u32,
    /// The fewest sales the velocity may be read from.
    pub minimum_samples: usize,
}

/// One sale: an order line of the review window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sale {
    /// The order line node.
    pub node_id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The day of the sale, in days since 1970-01-01.
    pub day: i64,
    /// The units sold.
    pub quantity: i64,
}

/// One open purchase order line: units ordered and not yet received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenLine {
    /// The purchase order line node.
    pub node_id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The units still to arrive.
    pub quantity: i64,
}

/// A `:StockLevel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stock {
    /// The stock level node.
    pub node_id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The units held.
    pub on_hand: i64,
    /// The units held back for open orders.
    pub reserved: i64,
}

/// Whether the rule could decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The velocity was read from enough sales and the rule decided.
    Decided,
    /// Fewer sales than the policy asks for.
    InsufficientData,
}

impl Status {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decided => "decided",
            Self::InsufficientData => "insufficient_data",
        }
    }
}

/// The result of one evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderResult {
    /// Whether the rule decided.
    pub status: Status,
    /// Whether to reorder; false when the rule could not decide.
    pub reorder: bool,
    /// The reorder point; absent when the rule could not decide.
    pub point: Option<i64>,
    /// On hand plus on order minus reserved.
    pub position: i64,
    /// The units to order, a multiple of the minimum order quantity; zero unless reordering.
    pub quantity: i64,
    /// The sales the velocity was read from.
    pub samples: usize,
    /// The rule digest, the input nodes and the steps.
    pub trace: RuleTrace,
}

fn to_i64(value: i128, what: &str) -> Result<i64, RuleRefusal> {
    i64::try_from(value)
        .map_err(|_| RuleRefusal::new(REORDER_OVERFLOW, format!("{what} does not fit in 64 bits")))
}

/// Ceiling division for a non-negative numerator and a positive denominator.
fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    (numerator + denominator - 1).div_euclid(denominator)
}

/// The reorder decision for `policy` at `as_of`. Sales outside the trailing review window
/// (the days `as_of - review_window_days + 1 ..= as_of`) are not read. `trace` carries the
/// rule digest and the policy node; the sales, the stock level and the open lines read are
/// added to its inputs.
pub fn reorder_point(
    policy: &Policy,
    sales: &[Sale],
    stock: &Stock,
    open_lines: &[OpenLine],
    as_of: i64,
    mut trace: RuleTrace,
) -> Result<ReorderResult, RuleRefusal> {
    if policy.review_window_days == 0 {
        return Err(RuleRefusal::new(
            REORDER_POLICY_INVALID,
            "the review window is at least one day",
        ));
    }
    if policy.safety_stock < 0 || policy.minimum_order_quantity < 0 {
        return Err(RuleRefusal::new(
            REORDER_POLICY_INVALID,
            "the safety stock and the minimum order quantity are not negative",
        ));
    }
    let window_start = as_of - i64::from(policy.review_window_days);
    let in_window: Vec<&Sale> = sales
        .iter()
        .filter(|sale| sale.day > window_start && sale.day <= as_of)
        .collect();
    for sale in &in_window {
        trace
            .inputs
            .push(InputRef::new(sale.node_id.clone(), sale.revision));
    }
    trace
        .inputs
        .push(InputRef::new(stock.node_id.clone(), stock.revision));
    for line in open_lines {
        trace
            .inputs
            .push(InputRef::new(line.node_id.clone(), line.revision));
    }
    trace.inputs.sort();
    trace.inputs.dedup();

    let on_order: i128 = open_lines
        .iter()
        .map(|line| i128::from(line.quantity))
        .sum();
    let position = to_i64(
        i128::from(stock.on_hand) + on_order - i128::from(stock.reserved),
        "the position",
    )?;
    let samples = in_window.len();
    trace.step(format!(
        "{samples} sales in the {} days ending day {as_of}; position {position}",
        policy.review_window_days
    ));
    if samples < policy.minimum_samples {
        trace.step(format!(
            "insufficient_data: {samples} sales, the policy asks for {}",
            policy.minimum_samples
        ));
        return Ok(ReorderResult {
            status: Status::InsufficientData,
            reorder: false,
            point: None,
            position,
            quantity: 0,
            samples,
            trace,
        });
    }

    let units: i128 = in_window.iter().map(|sale| i128::from(sale.quantity)).sum();
    let demand = ceil_div(
        units.max(0) * i128::from(policy.lead_time_days),
        i128::from(policy.review_window_days),
    );
    let point = to_i64(
        demand + i128::from(policy.safety_stock),
        "the reorder point",
    )?;
    trace.step(format!(
        "point = ceil({units} * {} / {}) + {} = {point}",
        policy.lead_time_days, policy.review_window_days, policy.safety_stock
    ));
    let reorder = position < point;
    let quantity = if reorder {
        let shortfall = i128::from(point) - i128::from(position);
        let moq = i128::from(policy.minimum_order_quantity);
        let rounded = if moq > 1 {
            ceil_div(shortfall, moq) * moq
        } else {
            shortfall
        };
        to_i64(rounded, "the order quantity")?
    } else {
        0
    };
    trace.step(format!("reorder {reorder} quantity {quantity}"));
    Ok(ReorderResult {
        status: Status::Decided,
        reorder,
        point: Some(point),
        position,
        quantity,
        samples,
        trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::rule_spec::load;
    use std::collections::BTreeMap;

    fn trace() -> RuleTrace {
        RuleTrace::new(
            &load("reorder", 1, BTreeMap::new()).unwrap(),
            vec![InputRef::new("policy:p1", 1)],
        )
    }

    fn policy() -> Policy {
        Policy {
            lead_time_days: 10,
            safety_stock: 20,
            minimum_order_quantity: 50,
            review_window_days: 30,
            minimum_samples: 3,
        }
    }

    fn sale(node: &str, day: i64, quantity: i64) -> Sale {
        Sale {
            node_id: node.to_string(),
            revision: 1,
            day,
            quantity,
        }
    }

    fn stock(on_hand: i64, reserved: i64) -> Stock {
        Stock {
            node_id: "stock:s1".to_string(),
            revision: 4,
            on_hand,
            reserved,
        }
    }

    // spec: MEDULLA-BUSINESS-OPS-R002.5
    #[test]
    fn position_below_point_orders_rounded_up_to_moq() {
        let as_of = 1_000;
        // 90 units in the 30-day window: velocity 3 a day, lead time 10 days, demand 30,
        // point 30 + 20 = 50.
        let sales = [
            sale("line:1", as_of - 1, 40),
            sale("line:2", as_of - 15, 30),
            sale("line:3", as_of - 29, 20),
            // Outside the review window: not read.
            sale("line:old", as_of - 30, 500),
        ];
        let open = [OpenLine {
            node_id: "pol:1".to_string(),
            revision: 2,
            quantity: 5,
        }];

        // Position 10 + 5 - 3 = 12 is below 50: the shortfall 38 rounds up to 50.
        let result =
            reorder_point(&policy(), &sales, &stock(10, 3), &open, as_of, trace()).unwrap();
        assert_eq!(result.status, Status::Decided);
        assert_eq!(result.point, Some(50));
        assert_eq!(result.position, 12);
        assert!(result.reorder);
        assert_eq!(result.quantity, 50);
        assert_eq!(result.samples, 3);
        assert_eq!(
            result.trace.rule_digest,
            load("reorder", 1, BTreeMap::new()).unwrap().digest
        );
        for (id, revision) in [
            ("line:1", 1),
            ("stock:s1", 4),
            ("pol:1", 2),
            ("policy:p1", 1),
        ] {
            assert!(
                result.trace.inputs.contains(&InputRef::new(id, revision)),
                "{id}"
            );
        }
        assert!(!result
            .trace
            .inputs
            .iter()
            .any(|input| input.node_id == "line:old"));

        // A shortfall of 120 rounds up to 150; with no minimum the shortfall is ordered.
        let big = reorder_point(&policy(), &sales, &stock(0, 70), &[], as_of, trace()).unwrap();
        assert_eq!(big.position, -70);
        assert_eq!(big.quantity, 150);
        let mut loose = policy();
        loose.minimum_order_quantity = 0;
        let exact = reorder_point(&loose, &sales, &stock(0, 70), &[], as_of, trace()).unwrap();
        assert_eq!(exact.quantity, 120);

        // A position at or above the point does not reorder.
        let at = reorder_point(&policy(), &sales, &stock(50, 0), &[], as_of, trace()).unwrap();
        assert!(!at.reorder);
        assert_eq!(at.quantity, 0);
        assert_eq!(at.point, Some(50));

        // The product is rounded up before the division: 7 units over 30 days at a lead time
        // of 10 days is ceil(70 / 30) = 3 units of demand.
        let slow = [
            sale("line:1", as_of, 3),
            sale("line:2", as_of, 2),
            sale("line:3", as_of, 2),
        ];
        let mut rounding = policy();
        rounding.safety_stock = 0;
        let result = reorder_point(&rounding, &slow, &stock(0, 0), &[], as_of, trace()).unwrap();
        assert_eq!(result.point, Some(3));

        // Fewer sales than the policy asks for is insufficient_data, not a decision.
        let few = reorder_point(&policy(), &sales[..2], &stock(0, 0), &[], as_of, trace()).unwrap();
        assert_eq!(few.status, Status::InsufficientData);
        assert_eq!(few.status.as_str(), "insufficient_data");
        assert!(!few.reorder);
        assert_eq!(few.point, None);
        assert_eq!(few.quantity, 0);

        // The same inputs give the same result.
        assert_eq!(
            reorder_point(&policy(), &sales, &stock(10, 3), &open, as_of, trace()).unwrap(),
            reorder_point(&policy(), &sales, &stock(10, 3), &open, as_of, trace()).unwrap()
        );

        // Refusals.
        let mut bad = policy();
        bad.review_window_days = 0;
        assert_eq!(
            reorder_point(&bad, &sales, &stock(0, 0), &[], as_of, trace())
                .unwrap_err()
                .code,
            REORDER_POLICY_INVALID
        );
        let mut bad = policy();
        bad.safety_stock = -1;
        assert_eq!(
            reorder_point(&bad, &sales, &stock(0, 0), &[], as_of, trace())
                .unwrap_err()
                .code,
            REORDER_POLICY_INVALID
        );
        let huge = [
            sale("line:1", as_of, i64::MAX),
            sale("line:2", as_of, i64::MAX),
            sale("line:3", as_of, i64::MAX),
        ];
        let mut wide = policy();
        wide.lead_time_days = u32::MAX;
        wide.review_window_days = 1;
        assert_eq!(
            reorder_point(&wide, &huge, &stock(0, 0), &[], as_of, trace())
                .unwrap_err()
                .code,
            REORDER_OVERFLOW
        );
    }
}
