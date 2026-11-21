//! Threshold applicability (MEDULLA-BUSINESS-OPS-R002.3).
//!
//! Kind `threshold`. Input: an applicability expression over named metrics, a trailing
//! period in days and an as-of day. A metric is a trailing sum, count or maximum over
//! ledger, order or payment data points, in fixed-point minor units. Operators: `<`,
//! `<=`, `>`, `>=`, `between`, `and`, `or`. Output: whether the rule applies, each
//! metric value and the nodes summed. A metric with no data in the window is
//! `unknown`, not false; `and` and `or` follow three-valued logic, so a false operand of
//! `and` and a true operand of `or` still decide the result.

use std::collections::BTreeMap;

use super::rule_spec::{InputRef, RuleRefusal, RuleTrace};

/// The refusal code for an expression that names a metric no definition declares.
pub const THRESHOLD_METRIC_UNDEFINED: &str = "THRESHOLD_METRIC_UNDEFINED";
/// The refusal code for a malformed expression or period.
pub const THRESHOLD_EXPRESSION_INVALID: &str = "THRESHOLD_EXPRESSION_INVALID";
/// The refusal code for a sum that does not fit in 64 bits.
pub const THRESHOLD_OVERFLOW: &str = "THRESHOLD_OVERFLOW";

/// The deepest expression one evaluation accepts.
pub const MAX_DEPTH: usize = 32;

/// How a metric aggregates the data points in its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aggregate {
    /// The sum of the amounts.
    Sum,
    /// The number of data points.
    Count,
    /// The largest amount.
    Max,
}

/// A named metric: an aggregate over the data points of one series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricDef {
    /// The metric name the expression uses.
    pub name: String,
    /// The series of data points it aggregates.
    pub series: String,
    /// How it aggregates.
    pub aggregate: Aggregate,
}

/// One data point read from a ledger, order or payment node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataPoint {
    /// The node the point was read from.
    pub node_id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The series the point belongs to.
    pub series: String,
    /// The day the point falls on, in days since 1970-01-01.
    pub day: i64,
    /// The amount in minor units; ignored by a count.
    pub amount: i64,
}

/// A comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// `<`.
    Less,
    /// `<=`.
    LessOrEqual,
    /// `>`.
    Greater,
    /// `>=`.
    GreaterOrEqual,
}

/// An applicability expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// A metric compared with a value.
    Compare {
        /// The metric name.
        metric: String,
        /// The operator.
        op: Comparison,
        /// The value, in the metric's unit.
        value: i64,
    },
    /// A metric inside `low..=high`.
    Between {
        /// The metric name.
        metric: String,
        /// The lower bound, inclusive.
        low: i64,
        /// The upper bound, inclusive.
        high: i64,
    },
    /// Both operands.
    And(Box<Expr>, Box<Expr>),
    /// Either operand.
    Or(Box<Expr>, Box<Expr>),
}

/// Whether a rule applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applies {
    /// The expression holds.
    Yes,
    /// The expression does not hold.
    No,
    /// A metric the expression needs has no data.
    Unknown,
}

/// One metric value with the nodes it was computed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricValue {
    /// The metric name.
    pub name: String,
    /// The value, or `None` when the window held no data.
    pub value: Option<i64>,
    /// The nodes summed, sorted.
    pub nodes: Vec<String>,
}

/// The result of one evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applicability {
    /// Whether the rule applies.
    pub applies: Applies,
    /// Each metric the expression names, by name.
    pub metrics: Vec<MetricValue>,
    /// The rule digest, the input nodes and the steps.
    pub trace: RuleTrace,
}

fn not_valid(detail: impl Into<String>) -> RuleRefusal {
    RuleRefusal::new(THRESHOLD_EXPRESSION_INVALID, detail)
}

fn collect_metrics<'a>(
    expr: &'a Expr,
    depth: usize,
    names: &mut Vec<&'a str>,
) -> Result<(), RuleRefusal> {
    if depth > MAX_DEPTH {
        return Err(not_valid(format!("the expression is deeper than {MAX_DEPTH}")));
    }
    match expr {
        Expr::Compare { metric, .. } => names.push(metric),
        Expr::Between { metric, low, high } => {
            if low > high {
                return Err(not_valid(format!("between {low} and {high} is empty")));
            }
            names.push(metric);
        }
        Expr::And(left, right) | Expr::Or(left, right) => {
            collect_metrics(left, depth + 1, names)?;
            collect_metrics(right, depth + 1, names)?;
        }
    }
    Ok(())
}

fn compute(
    def: &MetricDef,
    points: &[DataPoint],
    window_start: i64,
    as_of: i64,
) -> Result<(MetricValue, Vec<InputRef>), RuleRefusal> {
    let mut used: Vec<&DataPoint> = points
        .iter()
        .filter(|p| p.series == def.series && p.day > window_start && p.day <= as_of)
        .collect();
    used.sort_by(|a, b| (&a.node_id, a.revision).cmp(&(&b.node_id, b.revision)));
    let value = if used.is_empty() {
        None
    } else {
        Some(match def.aggregate {
            Aggregate::Count => used.len() as i64,
            Aggregate::Max => used.iter().map(|p| p.amount).max().unwrap_or(0),
            Aggregate::Sum => {
                let mut total = 0_i64;
                for point in &used {
                    total = total.checked_add(point.amount).ok_or_else(|| {
                        RuleRefusal::new(
                            THRESHOLD_OVERFLOW,
                            format!("the sum of `{}` does not fit in 64 bits", def.name),
                        )
                    })?;
                }
                total
            }
        })
    };
    let mut nodes: Vec<String> = used.iter().map(|p| p.node_id.clone()).collect();
    nodes.dedup();
    let inputs = used
        .iter()
        .map(|p| InputRef::new(p.node_id.clone(), p.revision))
        .collect();
    Ok((
        MetricValue {
            name: def.name.clone(),
            value,
            nodes,
        },
        inputs,
    ))
}

fn decide(expr: &Expr, values: &BTreeMap<&str, Option<i64>>) -> Applies {
    let check = |metric: &str, test: &dyn Fn(i64) -> bool| match values.get(metric).copied().flatten()
    {
        None => Applies::Unknown,
        Some(value) if test(value) => Applies::Yes,
        Some(_) => Applies::No,
    };
    match expr {
        Expr::Compare { metric, op, value } => check(metric, &|v| match op {
            Comparison::Less => v < *value,
            Comparison::LessOrEqual => v <= *value,
            Comparison::Greater => v > *value,
            Comparison::GreaterOrEqual => v >= *value,
        }),
        Expr::Between { metric, low, high } => check(metric, &|v| v >= *low && v <= *high),
        Expr::And(left, right) => match (decide(left, values), decide(right, values)) {
            (Applies::No, _) | (_, Applies::No) => Applies::No,
            (Applies::Yes, Applies::Yes) => Applies::Yes,
            _ => Applies::Unknown,
        },
        Expr::Or(left, right) => match (decide(left, values), decide(right, values)) {
            (Applies::Yes, _) | (_, Applies::Yes) => Applies::Yes,
            (Applies::No, Applies::No) => Applies::No,
            _ => Applies::Unknown,
        },
    }
}

/// Whether `expr` applies. Each metric the expression names is computed over the trailing
/// `period_days` ending at `as_of` (the days `as_of - period_days + 1 ..= as_of`) from
/// `points`. `trace` carries the rule digest and the rule's own input nodes; the data
/// nodes read and the steps taken are added to it.
pub fn applies(
    expr: &Expr,
    metrics: &[MetricDef],
    points: &[DataPoint],
    period_days: u32,
    as_of: i64,
    mut trace: RuleTrace,
) -> Result<Applicability, RuleRefusal> {
    if period_days == 0 {
        return Err(not_valid("the period is at least one day"));
    }
    let mut names = Vec::new();
    collect_metrics(expr, 0, &mut names)?;
    names.sort_unstable();
    names.dedup();
    let window_start = as_of - i64::from(period_days);
    trace.step(format!(
        "trailing {period_days} days ending day {as_of} (after day {window_start})"
    ));
    let mut computed = Vec::with_capacity(names.len());
    let mut values: BTreeMap<&str, Option<i64>> = BTreeMap::new();
    for name in names {
        let def = metrics.iter().find(|def| def.name == name).ok_or_else(|| {
            RuleRefusal::new(
                THRESHOLD_METRIC_UNDEFINED,
                format!("no metric definition named `{name}`"),
            )
        })?;
        let (value, inputs) = compute(def, points, window_start, as_of)?;
        trace.step(format!("metric {} = {:?}", value.name, value.value));
        trace.inputs.extend(inputs);
        values.insert(name, value.value);
        computed.push(value);
    }
    trace.inputs.sort();
    trace.inputs.dedup();
    let applies = decide(expr, &values);
    trace.step(format!("applies {applies:?}"));
    Ok(Applicability {
        applies,
        metrics: computed,
        trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::rule_spec::load;

    fn trace() -> RuleTrace {
        RuleTrace::new(
            &load("threshold", 1, BTreeMap::new()).unwrap(),
            vec![InputRef::new("rule:t1", 1)],
        )
    }

    fn metric(name: &str, series: &str, aggregate: Aggregate) -> MetricDef {
        MetricDef {
            name: name.to_string(),
            series: series.to_string(),
            aggregate,
        }
    }

    fn point(node: &str, series: &str, day: i64, amount: i64) -> DataPoint {
        DataPoint {
            node_id: node.to_string(),
            revision: 1,
            series: series.to_string(),
            day,
            amount,
        }
    }

    fn compare(metric: &str, op: Comparison, value: i64) -> Expr {
        Expr::Compare {
            metric: metric.to_string(),
            op,
            value,
        }
    }

    // spec: MEDULLA-BUSINESS-OPS-R002.3
    #[test]
    fn trailing_revenue_gates_obligation_and_missing_data_is_unknown() {
        let metrics = [
            metric("revenue", "payments", Aggregate::Sum),
            metric("orders", "orders", Aggregate::Count),
            metric("largest", "payments", Aggregate::Max),
        ];
        let as_of = 20_000;
        let points = [
            point("pay:1", "payments", as_of - 10, 6_000_000),
            point("pay:2", "payments", as_of - 200, 5_500_000),
            // Outside the trailing 365 days: not summed.
            point("pay:old", "payments", as_of - 365, 9_000_000),
            // After the as-of day: not summed.
            point("pay:future", "payments", as_of + 1, 9_000_000),
        ];
        let gate = compare("revenue", Comparison::GreaterOrEqual, 10_000_000);

        // Trailing revenue 11,500,000 minor units gates the obligation in.
        let result = applies(&gate, &metrics, &points, 365, as_of, trace()).unwrap();
        assert_eq!(result.applies, Applies::Yes);
        assert_eq!(result.metrics.len(), 1);
        assert_eq!(result.metrics[0].value, Some(11_500_000));
        assert_eq!(result.metrics[0].nodes, ["pay:1", "pay:2"]);
        assert!(result
            .trace
            .inputs
            .contains(&InputRef::new("pay:1", 1)));
        assert!(result.trace.inputs.contains(&InputRef::new("rule:t1", 1)));
        assert!(!result
            .trace
            .inputs
            .iter()
            .any(|input| input.node_id == "pay:old" || input.node_id == "pay:future"));

        // A higher threshold gates it out.
        let high = compare("revenue", Comparison::GreaterOrEqual, 12_000_000);
        assert_eq!(
            applies(&high, &metrics, &points, 365, as_of, trace()).unwrap().applies,
            Applies::No
        );

        // A metric with no data is unknown, not false.
        let orders = compare("orders", Comparison::Greater, 0);
        let missing = applies(&orders, &metrics, &points, 365, as_of, trace()).unwrap();
        assert_eq!(missing.applies, Applies::Unknown);
        assert_eq!(missing.metrics[0].value, None);
        assert!(missing.metrics[0].nodes.is_empty());

        // Three-valued logic: a false operand decides `and`; a true operand decides `or`;
        // otherwise unknown stays unknown.
        let and = |a: Expr, b: Expr| Expr::And(Box::new(a), Box::new(b));
        let or = |a: Expr, b: Expr| Expr::Or(Box::new(a), Box::new(b));
        let yes = gate.clone();
        let no = high.clone();
        let unknown = orders.clone();
        let check = |expr: Expr| applies(&expr, &metrics, &points, 365, as_of, trace()).unwrap().applies;
        assert_eq!(check(and(no.clone(), unknown.clone())), Applies::No);
        assert_eq!(check(and(yes.clone(), unknown.clone())), Applies::Unknown);
        assert_eq!(check(and(yes.clone(), yes.clone())), Applies::Yes);
        assert_eq!(check(or(yes.clone(), unknown.clone())), Applies::Yes);
        assert_eq!(check(or(no.clone(), unknown)), Applies::Unknown);
        assert_eq!(check(or(no.clone(), no)), Applies::No);

        // Between is inclusive; count and max aggregate over the window.
        let between = Expr::Between {
            metric: "revenue".to_string(),
            low: 11_500_000,
            high: 11_500_000,
        };
        assert_eq!(check(between), Applies::Yes);
        let largest = compare("largest", Comparison::Less, 6_000_001);
        let result = applies(&largest, &metrics, &points, 365, as_of, trace()).unwrap();
        assert_eq!(result.applies, Applies::Yes);
        assert_eq!(result.metrics[0].value, Some(6_000_000));

        // The same inputs give the same result.
        assert_eq!(
            applies(&gate, &metrics, &points, 365, as_of, trace()).unwrap(),
            applies(&gate, &metrics, &points, 365, as_of, trace()).unwrap()
        );
    }

    #[test]
    fn refusals_name_the_fault() {
        let metrics = [metric("revenue", "payments", Aggregate::Sum)];
        let gate = compare("revenue", Comparison::Greater, 1);
        let undefined = compare("profit", Comparison::Greater, 1);
        assert_eq!(
            applies(&undefined, &metrics, &[], 30, 100, trace()).unwrap_err().code,
            THRESHOLD_METRIC_UNDEFINED
        );
        assert_eq!(
            applies(&gate, &metrics, &[], 0, 100, trace()).unwrap_err().code,
            THRESHOLD_EXPRESSION_INVALID
        );
        let empty = Expr::Between {
            metric: "revenue".to_string(),
            low: 5,
            high: 1,
        };
        assert_eq!(
            applies(&empty, &metrics, &[], 30, 100, trace()).unwrap_err().code,
            THRESHOLD_EXPRESSION_INVALID
        );
        let overflow = [
            point("a", "payments", 99, i64::MAX),
            point("b", "payments", 99, 1),
        ];
        assert_eq!(
            applies(&gate, &metrics, &overflow, 30, 100, trace()).unwrap_err().code,
            THRESHOLD_OVERFLOW
        );
        let mut deep = gate.clone();
        for _ in 0..=MAX_DEPTH {
            deep = Expr::And(Box::new(deep), Box::new(gate.clone()));
        }
        assert_eq!(
            applies(&deep, &metrics, &[], 30, 100, trace()).unwrap_err().code,
            THRESHOLD_EXPRESSION_INVALID
        );
    }
}
