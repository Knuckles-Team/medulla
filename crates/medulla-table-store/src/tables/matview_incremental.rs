//! The incremental view program (MEDULLA-INCREMENTAL-VIEWS-R009.1).
//!
//! A SQL view over one user table with a filter, a scalar projection, or a grouped `count`,
//! `sum` or `avg` over exact numbers is described by an [`IncrementalViewProgram`]. The program
//! is plain data: it serializes with `serde` into the `__sql_matviews__` catalog record of
//! MEDULLA-ENGINE-PARITY-R014.1, and the only evaluator here is [`RowExpr::eval`].
//!
//! Values are the `serde_json::Value`s the table store keeps in a row: a JSON integer is an
//! integer, a JSON string is text, and a decimal column holds its canonical decimal string. A
//! decimal operand is therefore named by [`RowExpr::Decimal`], which the compiler wraps around a
//! numeric column; arithmetic also reads a string operand as a decimal, because arithmetic on
//! text has no other meaning. Integers and decimals are exact (`i128` unscaled value and scale);
//! a float cell is refused. Boolean logic is SQL three-valued: `Value::Null` is unknown, and a
//! filter keeps a row only when its filter evaluates to `true`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cmp::Ordering;

/// One table row: column name to cell value.
pub type Row = Map<String, Value>;

/// A scalar expression over one row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RowExpr {
    Column(String),
    Literal(Value),
    /// Reads the operand as an exact decimal (an integer or a canonical decimal string).
    Decimal(Box<RowExpr>),
    Add(Box<RowExpr>, Box<RowExpr>),
    Sub(Box<RowExpr>, Box<RowExpr>),
    Mul(Box<RowExpr>, Box<RowExpr>),
    Eq(Box<RowExpr>, Box<RowExpr>),
    Ne(Box<RowExpr>, Box<RowExpr>),
    Lt(Box<RowExpr>, Box<RowExpr>),
    Le(Box<RowExpr>, Box<RowExpr>),
    Gt(Box<RowExpr>, Box<RowExpr>),
    Ge(Box<RowExpr>, Box<RowExpr>),
    And(Box<RowExpr>, Box<RowExpr>),
    Or(Box<RowExpr>, Box<RowExpr>),
    Not(Box<RowExpr>),
    IsNull(Box<RowExpr>),
    IsNotNull(Box<RowExpr>),
}

/// An exact aggregate of a grouped view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExactAgg {
    Count,
    Sum(RowExpr),
    Avg(RowExpr),
}

/// What the view stores per source row or per group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ViewOutput {
    Rows(Vec<(String, RowExpr)>),
    Groups {
        keys: Vec<(String, RowExpr)>,
        aggs: Vec<(String, ExactAgg)>,
    },
}

/// An equi-join of the program's table with a second table (MEDULLA-INCREMENTAL-VIEWS-R013.1).
/// The output rows key on the pair of source primary keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquiJoin {
    /// The joined table.
    pub right: String,
    /// The column of the program's table that the join compares.
    pub left_key: String,
    /// The column of the right table that the join compares.
    pub right_key: String,
}

/// The incremental program of one materialized view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncrementalViewProgram {
    pub view: String,
    pub table: String,
    /// The join with a second table, when the view has one.
    #[serde(default)]
    pub join: Option<EquiJoin>,
    pub filter: Option<RowExpr>,
    pub output: ViewOutput,
}

/// An evaluated scalar: the exact number forms, text, a boolean or unknown.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    Null,
    Bool(bool),
    Text(String),
    /// `unscaled / 10^scale`; an integer has scale 0.
    Num { unscaled: i128, scale: u32 },
}

fn overflow() -> String {
    "ROW_EXPR_OVERFLOW: exact arithmetic exceeds 128 bits".to_string()
}

fn pow10(scale: u32) -> Result<i128, String> {
    10i128.checked_pow(scale).ok_or_else(overflow)
}

/// `value` rescaled to `scale` (which must not be smaller than its own).
fn rescale(unscaled: i128, from: u32, to: u32) -> Result<i128, String> {
    unscaled
        .checked_mul(pow10(to - from)?)
        .ok_or_else(overflow)
}

fn parse_decimal(text: &str) -> Result<Scalar, String> {
    let bad = || format!("ROW_EXPR_NOT_DECIMAL: `{text}` is not a decimal");
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    let mut unscaled: i128 = 0;
    for byte in whole.bytes().chain(fraction.bytes()) {
        unscaled = unscaled
            .checked_mul(10)
            .and_then(|value| value.checked_add(i128::from(byte - b'0')))
            .ok_or_else(overflow)?;
    }
    Ok(Scalar::Num {
        unscaled: if negative { -unscaled } else { unscaled },
        scale: u32::try_from(fraction.len()).map_err(|_| bad())?,
    })
}

fn format_decimal(unscaled: i128, scale: u32) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let sign = if unscaled < 0 { "-" } else { "" };
    let digits = unscaled.unsigned_abs().to_string();
    let scale = scale as usize;
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (whole, fraction) = padded.split_at(padded.len() - scale);
    format!("{sign}{whole}.{fraction}")
}

fn from_value(value: &Value) -> Result<Scalar, String> {
    match value {
        Value::Null => Ok(Scalar::Null),
        Value::Bool(flag) => Ok(Scalar::Bool(*flag)),
        Value::String(text) => Ok(Scalar::Text(text.clone())),
        Value::Number(number) => match (number.as_i64(), number.as_u64()) {
            (Some(int), _) => Ok(Scalar::Num { unscaled: i128::from(int), scale: 0 }),
            (None, Some(int)) => Ok(Scalar::Num { unscaled: i128::from(int), scale: 0 }),
            _ => Err("ROW_EXPR_FLOAT: a float is not an exact number".to_string()),
        },
        other => Err(format!("ROW_EXPR_UNSUPPORTED_VALUE: {other}")),
    }
}

/// The exact number of `scalar`, reading a text operand as a decimal. `None` is unknown.
fn number(scalar: Scalar) -> Result<Option<(i128, u32)>, String> {
    match scalar {
        Scalar::Null => Ok(None),
        Scalar::Num { unscaled, scale } => Ok(Some((unscaled, scale))),
        Scalar::Text(text) => match parse_decimal(&text)? {
            Scalar::Num { unscaled, scale } => Ok(Some((unscaled, scale))),
            _ => Err("ROW_EXPR_NOT_DECIMAL".to_string()),
        },
        Scalar::Bool(_) => Err("ROW_EXPR_TYPE: a boolean is not a number".to_string()),
    }
}

fn truth(scalar: Scalar) -> Result<Option<bool>, String> {
    match scalar {
        Scalar::Null => Ok(None),
        Scalar::Bool(flag) => Ok(Some(flag)),
        _ => Err("ROW_EXPR_TYPE: a boolean was expected".to_string()),
    }
}

fn compare_numbers(left: (i128, u32), right: (i128, u32)) -> Result<Ordering, String> {
    let scale = left.1.max(right.1);
    Ok(rescale(left.0, left.1, scale)?.cmp(&rescale(right.0, right.1, scale)?))
}

fn compare(left: Scalar, right: Scalar) -> Result<Option<Ordering>, String> {
    match (left, right) {
        (Scalar::Null, _) | (_, Scalar::Null) => Ok(None),
        (Scalar::Text(a), Scalar::Text(b)) => Ok(Some(a.cmp(&b))),
        (Scalar::Bool(a), Scalar::Bool(b)) => Ok(Some(a.cmp(&b))),
        (a, b) => match (number(a)?, number(b)?) {
            (Some(a), Some(b)) => compare_numbers(a, b).map(Some),
            _ => Ok(None),
        },
    }
}

fn scalar_of(unscaled: i128, scale: u32) -> Scalar {
    Scalar::Num { unscaled, scale }
}

impl RowExpr {
    /// The value of the expression over `row`, in SQL three-valued logic: an unknown operand
    /// gives `Value::Null`, a filter keeps a row only when this is `Value::Bool(true)`. An
    /// integer result is a JSON integer, a decimal result its canonical decimal string. A
    /// missing column is an error.
    pub fn eval(&self, row: &Row) -> Result<Value, String> {
        match self.scalar(row)? {
            Scalar::Null => Ok(Value::Null),
            Scalar::Bool(flag) => Ok(Value::Bool(flag)),
            Scalar::Text(text) => Ok(Value::String(text)),
            Scalar::Num { unscaled, scale: 0 } => i64::try_from(unscaled)
                .map(Value::from)
                .or_else(|_| Ok(Value::String(unscaled.to_string()))),
            Scalar::Num { unscaled, scale } => Ok(Value::String(format_decimal(unscaled, scale))),
        }
    }

    fn scalar(&self, row: &Row) -> Result<Scalar, String> {
        match self {
            Self::Column(name) => from_value(
                row.get(name)
                    .ok_or_else(|| format!("ROW_EXPR_UNKNOWN_COLUMN: {name}"))?,
            ),
            Self::Literal(value) => from_value(value),
            Self::Decimal(inner) => Ok(match number(inner.scalar(row)?)? {
                Some((unscaled, scale)) => scalar_of(unscaled, scale),
                None => Scalar::Null,
            }),
            Self::Add(a, b) => a.arithmetic(b, row, i128::checked_add),
            Self::Sub(a, b) => a.arithmetic(b, row, i128::checked_sub),
            Self::Mul(a, b) => {
                let (Some(a), Some(b)) = (number(a.scalar(row)?)?, number(b.scalar(row)?)?) else {
                    return Ok(Scalar::Null);
                };
                Ok(scalar_of(
                    a.0.checked_mul(b.0).ok_or_else(overflow)?,
                    a.1.checked_add(b.1).ok_or_else(overflow)?,
                ))
            }
            Self::Eq(a, b) => a.relation(b, row, |order| order == Ordering::Equal),
            Self::Ne(a, b) => a.relation(b, row, |order| order != Ordering::Equal),
            Self::Lt(a, b) => a.relation(b, row, |order| order == Ordering::Less),
            Self::Le(a, b) => a.relation(b, row, |order| order != Ordering::Greater),
            Self::Gt(a, b) => a.relation(b, row, |order| order == Ordering::Greater),
            Self::Ge(a, b) => a.relation(b, row, |order| order != Ordering::Less),
            Self::And(a, b) => {
                let (left, right) = (truth(a.scalar(row)?)?, truth(b.scalar(row)?)?);
                Ok(match (left, right) {
                    (Some(false), _) | (_, Some(false)) => Scalar::Bool(false),
                    (Some(true), Some(true)) => Scalar::Bool(true),
                    _ => Scalar::Null,
                })
            }
            Self::Or(a, b) => {
                let (left, right) = (truth(a.scalar(row)?)?, truth(b.scalar(row)?)?);
                Ok(match (left, right) {
                    (Some(true), _) | (_, Some(true)) => Scalar::Bool(true),
                    (Some(false), Some(false)) => Scalar::Bool(false),
                    _ => Scalar::Null,
                })
            }
            Self::Not(inner) => Ok(match truth(inner.scalar(row)?)? {
                Some(flag) => Scalar::Bool(!flag),
                None => Scalar::Null,
            }),
            Self::IsNull(inner) => Ok(Scalar::Bool(inner.scalar(row)? == Scalar::Null)),
            Self::IsNotNull(inner) => Ok(Scalar::Bool(inner.scalar(row)? != Scalar::Null)),
        }
    }

    fn arithmetic(
        &self,
        other: &Self,
        row: &Row,
        operation: fn(i128, i128) -> Option<i128>,
    ) -> Result<Scalar, String> {
        let (Some(a), Some(b)) = (number(self.scalar(row)?)?, number(other.scalar(row)?)?) else {
            return Ok(Scalar::Null);
        };
        let scale = a.1.max(b.1);
        let value = operation(rescale(a.0, a.1, scale)?, rescale(b.0, b.1, scale)?)
            .ok_or_else(overflow)?;
        Ok(scalar_of(value, scale))
    }

    fn relation(
        &self,
        other: &Self,
        row: &Row,
        holds: fn(Ordering) -> bool,
    ) -> Result<Scalar, String> {
        Ok(match compare(self.scalar(row)?, other.scalar(row)?)? {
            Some(order) => Scalar::Bool(holds(order)),
            None => Scalar::Null,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row() -> Row {
        let Value::Object(row) = json!({
            "a": 7, "b": null, "t": true, "f": false,
            "price": "12.50", "qty": 3, "name": "north"
        }) else {
            unreachable!()
        };
        row
    }

    fn column(name: &str) -> Box<RowExpr> {
        Box::new(RowExpr::Column(name.to_string()))
    }

    fn literal(value: Value) -> Box<RowExpr> {
        Box::new(RowExpr::Literal(value))
    }

    fn eval(expr: RowExpr) -> Value {
        expr.eval(&row()).unwrap()
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R009.1
    #[test]
    fn row_expr_three_valued_logic() {
        let unknown = || column("b");
        // An unknown operand makes a comparison and arithmetic unknown.
        assert_eq!(eval(RowExpr::Eq(unknown(), literal(json!(1)))), Value::Null);
        assert_eq!(eval(RowExpr::Add(unknown(), literal(json!(1)))), Value::Null);
        assert_eq!(eval(RowExpr::IsNull(unknown())), json!(true));
        assert_eq!(eval(RowExpr::IsNotNull(unknown())), json!(false));
        assert_eq!(eval(RowExpr::IsNotNull(column("a"))), json!(true));
        // AND and OR: a decisive operand wins over unknown, otherwise unknown propagates.
        let cmp = || Box::new(RowExpr::Gt(unknown(), literal(json!(0))));
        assert_eq!(eval(RowExpr::And(column("f"), cmp())), json!(false));
        assert_eq!(eval(RowExpr::And(column("t"), cmp())), Value::Null);
        assert_eq!(eval(RowExpr::Or(column("t"), cmp())), json!(true));
        assert_eq!(eval(RowExpr::Or(column("f"), cmp())), Value::Null);
        assert_eq!(eval(RowExpr::Not(cmp())), Value::Null);
        assert_eq!(eval(RowExpr::Not(column("t"))), json!(false));
        // Every comparison over integers.
        let seven = || column("a");
        for (expr, expected) in [
            (RowExpr::Eq(seven(), literal(json!(7))), true),
            (RowExpr::Ne(seven(), literal(json!(7))), false),
            (RowExpr::Lt(seven(), literal(json!(8))), true),
            (RowExpr::Le(seven(), literal(json!(7))), true),
            (RowExpr::Gt(seven(), literal(json!(7))), false),
            (RowExpr::Ge(seven(), literal(json!(7))), true),
        ] {
            assert_eq!(eval(expr), json!(expected));
        }
        // A filter keeps a row only when the filter is true: unknown drops it.
        let keeps = |expr: RowExpr| eval(expr) == json!(true);
        assert!(keeps(RowExpr::Gt(seven(), literal(json!(1)))));
        assert!(!keeps(RowExpr::Gt(unknown(), literal(json!(1)))));
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R009.1
    #[test]
    fn row_expr_arithmetic_is_exact() {
        let price = || Box::new(RowExpr::Decimal(column("price")));
        assert_eq!(eval(RowExpr::Add(column("a"), column("qty"))), json!(10));
        assert_eq!(eval(RowExpr::Sub(column("qty"), column("a"))), json!(-4));
        assert_eq!(eval(RowExpr::Mul(price(), column("qty"))), json!("37.50"));
        assert_eq!(eval(RowExpr::Sub(price(), literal(json!("0.75")))), json!("11.75"));
        assert_eq!(eval(RowExpr::Sub(literal(json!("0.25")), price())), json!("-12.25"));
        // Decimals compare by value, not as text.
        assert_eq!(
            eval(RowExpr::Gt(price(), literal(json!("9.5")))),
            json!(true)
        );
        assert_eq!(eval(RowExpr::Eq(price(), literal(json!("12.5")))), json!(true));
        // Text compares as text.
        assert_eq!(eval(RowExpr::Eq(column("name"), literal(json!("north")))), json!(true));
        // Refusals.
        assert!(RowExpr::Column("missing".into()).eval(&row()).is_err());
        assert!(RowExpr::Literal(json!(1.5)).eval(&row()).is_err());
        assert!(RowExpr::Add(column("name"), column("a")).eval(&row()).is_err());
        assert!(RowExpr::Mul(literal(json!(i64::MAX)), literal(json!(i64::MAX)))
            .eval(&row())
            .is_ok());
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R009.1
    #[test]
    fn program_round_trips_through_serde() {
        let program = IncrementalViewProgram {
            view: "by_region".into(),
            table: "orders".into(),
            join: None,
            filter: Some(RowExpr::Gt(column("amount"), literal(json!(0)))),
            output: ViewOutput::Groups {
                keys: vec![("region".into(), RowExpr::Column("region".into()))],
                aggs: vec![
                    ("n".into(), ExactAgg::Count),
                    ("total".into(), ExactAgg::Sum(RowExpr::Column("amount".into()))),
                    ("mean".into(), ExactAgg::Avg(RowExpr::Column("amount".into()))),
                ],
            },
        };
        let bytes = rmp_serde::to_vec_named(&program).unwrap();
        assert_eq!(rmp_serde::from_slice::<IncrementalViewProgram>(&bytes).unwrap(), program);
        let rows = IncrementalViewProgram {
            output: ViewOutput::Rows(vec![("x".into(), RowExpr::Column("a".into()))]),
            ..program
        };
        let json = serde_json::to_string(&rows).unwrap();
        assert_eq!(serde_json::from_str::<IncrementalViewProgram>(&json).unwrap(), rows);
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R013.1
    #[test]
    fn join_program_round_trips() {
        let program = IncrementalViewProgram {
            view: "order_regions".into(),
            table: "orders".into(),
            join: Some(EquiJoin {
                right: "customers".into(),
                left_key: "customer_id".into(),
                right_key: "id".into(),
            }),
            filter: None,
            output: ViewOutput::Rows(vec![("id".into(), RowExpr::Column("id".into()))]),
        };
        let bytes = rmp_serde::to_vec_named(&program).unwrap();
        assert_eq!(rmp_serde::from_slice::<IncrementalViewProgram>(&bytes).unwrap(), program);
        let json = serde_json::to_string(&program).unwrap();
        assert_eq!(serde_json::from_str::<IncrementalViewProgram>(&json).unwrap(), program);
        // A program stored before the join field existed reads as a single-table program.
        let mut value = serde_json::to_value(&program).unwrap();
        value.as_object_mut().unwrap().remove("join");
        let old: IncrementalViewProgram = serde_json::from_value(value).unwrap();
        assert_eq!(old.join, None);
    }
}
