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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExactAgg {
    Count,
    Sum(RowExpr),
    Avg(RowExpr),
    /// The smallest non-NULL value (R012.1). It needs no subtraction, so it accepts every
    /// ordered column type, floats included.
    Min(RowExpr),
    /// The largest non-NULL value (R012.1).
    Max(RowExpr),
}

/// What the view stores per source row or per group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewOutput {
    Rows(Vec<(String, RowExpr)>),
    Groups {
        keys: Vec<(String, RowExpr)>,
        aggs: Vec<(String, ExactAgg)>,
    },
    /// One stored row per distinct output tuple, with `__mv_count` the number of source rows
    /// (R012.1).
    Distinct(Vec<(String, RowExpr)>),
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    Num {
        unscaled: i128,
        scale: u32,
    },
}

fn overflow() -> String {
    "ROW_EXPR_OVERFLOW: exact arithmetic exceeds 128 bits".to_string()
}

fn pow10(scale: u32) -> Result<i128, String> {
    10i128.checked_pow(scale).ok_or_else(overflow)
}

/// `value` rescaled to `scale` (which must not be smaller than its own).
fn rescale(unscaled: i128, from: u32, to: u32) -> Result<i128, String> {
    unscaled.checked_mul(pow10(to - from)?).ok_or_else(overflow)
}

fn parse_decimal(text: &str) -> Result<Scalar, String> {
    let bad = || format!("ROW_EXPR_NOT_DECIMAL: `{text}` is not a decimal");
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
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
            (Some(int), _) => Ok(Scalar::Num {
                unscaled: i128::from(int),
                scale: 0,
            }),
            (None, Some(int)) => Ok(Scalar::Num {
                unscaled: i128::from(int),
                scale: 0,
            }),
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
        let value =
            operation(rescale(a.0, a.1, scale)?, rescale(b.0, b.1, scale)?).ok_or_else(overflow)?;
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

/// The hidden primary-key column of an incremental view's result table (R009.3).
pub const MV_KEY: &str = "__mv_key";
/// The hidden row-count column of a grouped view (R009.4).
pub const MV_COUNT: &str = "__mv_count";

/// The hidden exact-sum column of the aggregate named `agg` (R009.4).
pub fn mv_sum_column(agg: &str) -> String {
    format!("__mv_sum_{agg}")
}

/// The hidden non-null counter column of the aggregate named `agg`. `sum` and `avg` ignore NULL
/// arguments, so a group needs the number of arguments that were not NULL to give NULL for an
/// all-NULL sum and the right divisor for an average.
pub fn mv_nonnull_column(agg: &str) -> String {
    format!("__mv_nn_{agg}")
}

/// Whether `column` is hidden from the visible view.
pub fn is_hidden_column(column: &str) -> bool {
    column.starts_with("__mv_")
}

/// The rows a view changed in one commit (MEDULLA-INCREMENTAL-VIEWS-R010.2). Output columns
/// only; the hidden `__mv_*` columns are not part of the visible view.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ViewRowChanges {
    pub view: String,
    pub deleted: Vec<Row>,
    pub inserted: Vec<Row>,
}

impl ViewRowChanges {
    /// The rows as the row-change sink takes them: every deleted row as a `Delete` then every
    /// inserted row as an `Insert`, with `table` set to the view name. The key is the row
    /// rendered cell by cell, the rendering of a table without a primary key.
    pub fn into_view_row_change(self) -> Result<medulla_types::row_change::ViewRowChange, String> {
        use medulla_types::row_change::{RowChange, RowChangeKind};
        let view = self.view;
        let image = |row: &Row| rmp_serde::to_vec_named(row).map_err(|error| error.to_string());
        let mut changes = Vec::with_capacity(self.deleted.len() + self.inserted.len());
        for row in &self.deleted {
            changes.push(RowChange {
                table: view.clone(),
                kind: RowChangeKind::Delete,
                key: row_key_text(&[], row),
                before: Some(image(row)?),
                after: None,
            });
        }
        for row in &self.inserted {
            changes.push(RowChange {
                table: view.clone(),
                kind: RowChangeKind::Insert,
                key: row_key_text(&[], row),
                before: None,
                after: Some(image(row)?),
            });
        }
        Ok(medulla_types::row_change::ViewRowChange { view, changes })
    }
}

/// The primary key of `row` rendered as text: the `pk_columns` values joined by `,`, or every
/// cell in column order when the table has no primary key. It is the rendering of
/// `RowChange.key`.
pub fn row_key_text(pk_columns: &[String], row: &Row) -> String {
    let render = |value: &Value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if pk_columns.is_empty() {
        return row.values().map(render).collect::<Vec<_>>().join(",");
    }
    pk_columns
        .iter()
        .filter_map(|column| row.get(column))
        .map(render)
        .collect::<Vec<_>>()
        .join(",")
}

fn weight_overflow() -> String {
    "DELTA_WEIGHT_OVERFLOW: a grouped view accumulator exceeds its exact range".to_string()
}

fn round_half_even_div(numerator: i128, denominator: i128) -> Result<i128, String> {
    if denominator == 0 {
        return Err("ROW_EXPR_DIVIDE_BY_ZERO".to_string());
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let twice = remainder
        .unsigned_abs()
        .checked_mul(2)
        .ok_or_else(weight_overflow)?;
    let divisor = denominator.unsigned_abs();
    let away = if numerator.signum() * denominator.signum() < 0 {
        -1
    } else {
        1
    };
    let bump = match twice.cmp(&divisor) {
        Ordering::Greater => true,
        Ordering::Equal => quotient % 2 != 0,
        Ordering::Less => false,
    };
    Ok(if bump { quotient + away } else { quotient })
}

impl RowExpr {
    /// The exact number the expression has over `row`; `None` for NULL. A text operand is read
    /// as a decimal.
    pub fn eval_number(&self, row: &Row) -> Result<Option<(i128, u32)>, String> {
        number(self.scalar(row)?)
    }

    /// Whether the expression is true over `row` (a filter keeps a row only then).
    pub fn holds(&self, row: &Row) -> Result<bool, String> {
        Ok(self.eval(row)? == Value::Bool(true))
    }
}

/// An exact decimal accumulator: `unscaled / 10^scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExactSum {
    unscaled: i128,
    scale: u32,
}

impl ExactSum {
    /// Adds `weight * value`.
    pub fn add_weighted(&mut self, weight: i64, value: (i128, u32)) -> Result<(), String> {
        let scale = self.scale.max(value.1);
        let current = rescale(self.unscaled, self.scale, scale).map_err(|_| weight_overflow())?;
        let term = rescale(value.0, value.1, scale)
            .map_err(|_| weight_overflow())?
            .checked_mul(i128::from(weight))
            .ok_or_else(weight_overflow)?;
        self.unscaled = current.checked_add(term).ok_or_else(weight_overflow)?;
        self.scale = scale;
        Ok(())
    }

    /// The canonical decimal string of the sum.
    pub fn canonical(&self) -> String {
        format_decimal(self.unscaled, self.scale)
    }

    /// The sum as a JSON value: an integer when it has no fraction, else its decimal string.
    pub fn to_value(&self) -> Value {
        if self.scale == 0 {
            if let Ok(int) = i64::try_from(self.unscaled) {
                return Value::from(int);
            }
        }
        Value::String(self.canonical())
    }

    /// Reads a stored canonical decimal string.
    pub fn parse(text: &str) -> Result<Self, String> {
        match parse_decimal(text)? {
            Scalar::Num { unscaled, scale } => Ok(Self { unscaled, scale }),
            _ => Err("ROW_EXPR_NOT_DECIMAL".to_string()),
        }
    }

    /// `self / count` with 6 fractional digits, rounded half to even.
    pub fn average(&self, count: i64) -> Result<String, String> {
        let numerator = self
            .unscaled
            .checked_mul(1_000_000)
            .ok_or_else(weight_overflow)?;
        let denominator = pow10(self.scale)?
            .checked_mul(i128::from(count))
            .ok_or_else(weight_overflow)?;
        Ok(format_decimal(
            round_half_even_div(numerator, denominator)?,
            6,
        ))
    }
}

/// The accumulated state of one group of a grouped view (R009.4).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupState {
    pub keys: Vec<Value>,
    pub count: i64,
    /// One entry per aggregate: the exact sum and the number of non-NULL arguments.
    pub sums: Vec<(ExactSum, i64)>,
    /// One entry per aggregate: the current `min` or `max` (`None` for NULL or another aggregate).
    pub extremes: Vec<Option<Value>>,
    /// Aggregates whose stored extreme was retracted and must be recomputed from the group's rows.
    pub stale_extremes: Vec<bool>,
}

impl GroupState {
    /// An empty group with the key values `keys`.
    pub fn new(keys: Vec<Value>, aggs: usize) -> Self {
        Self {
            keys,
            count: 0,
            sums: vec![(ExactSum::default(), 0); aggs],
            extremes: vec![None; aggs],
            stale_extremes: vec![false; aggs],
        }
    }

    /// Reads the group the stored result `row` holds.
    pub fn from_stored(
        keys: &[(String, RowExpr)],
        aggs: &[(String, ExactAgg)],
        row: &Row,
    ) -> Result<Self, String> {
        let stored = |column: &str| {
            row.get(column)
                .ok_or_else(|| format!("MATVIEW_STATE_CORRUPT: `{column}` missing"))
        };
        let count = stored(MV_COUNT)?
            .as_i64()
            .ok_or_else(|| "MATVIEW_STATE_CORRUPT: count".to_string())?;
        let mut group = Self::new(
            keys.iter()
                .map(|(name, _)| stored(name).cloned())
                .collect::<Result<_, _>>()?,
            aggs.len(),
        );
        group.count = count;
        for (index, (name, agg)) in aggs.iter().enumerate() {
            if let ExactAgg::Min(_) | ExactAgg::Max(_) = agg {
                group.extremes[index] = match stored(name)? {
                    Value::Null => None,
                    other => Some(other.clone()),
                };
                continue;
            }
            if matches!(agg, ExactAgg::Count) {
                continue;
            }
            let slot = &mut group.sums[index];
            let sum = stored(&mv_sum_column(name))?;
            *slot = (
                match sum {
                    Value::String(text) => ExactSum::parse(text)?,
                    Value::Null => ExactSum::default(),
                    other => ExactSum::parse(&other.to_string())?,
                },
                stored(&mv_nonnull_column(name))?
                    .as_i64()
                    .ok_or_else(|| "MATVIEW_STATE_CORRUPT: non-null count".to_string())?,
            );
        }
        Ok(group)
    }

    /// Adds one source row of `weight` (`+1` entering, `-1` leaving).
    pub fn apply(
        &mut self,
        aggs: &[(String, ExactAgg)],
        weight: i64,
        row: &Row,
    ) -> Result<(), String> {
        self.count = self.count.checked_add(weight).ok_or_else(weight_overflow)?;
        for (index, (_, agg)) in aggs.iter().enumerate() {
            match agg {
                ExactAgg::Sum(expr) | ExactAgg::Avg(expr) => {
                    if let Some(value) = expr.eval_number(row)? {
                        let slot = &mut self.sums[index];
                        slot.0.add_weighted(weight, value)?;
                        slot.1 = slot.1.checked_add(weight).ok_or_else(weight_overflow)?;
                    }
                }
                ExactAgg::Min(expr) | ExactAgg::Max(expr) => {
                    let value = expr.eval(row)?;
                    if value.is_null() {
                        continue;
                    }
                    let decimal = matches!(expr, RowExpr::Decimal(_));
                    let is_min = matches!(agg, ExactAgg::Min(_));
                    let current = self.extremes[index].clone();
                    if weight > 0 {
                        let better = match &current {
                            None => true,
                            Some(held) => {
                                let order = compare_values(decimal, &value, held)?;
                                if is_min {
                                    order == Ordering::Less
                                } else {
                                    order == Ordering::Greater
                                }
                            }
                        };
                        if better {
                            self.extremes[index] = Some(value);
                        }
                    } else if current.as_ref().is_some_and(|held| {
                        compare_values(decimal, &value, held).is_ok_and(|o| o == Ordering::Equal)
                    }) {
                        self.stale_extremes[index] = true;
                    }
                }
                ExactAgg::Count => {}
            }
        }
        Ok(())
    }

    /// Whether a retraction removed a stored extreme that must be recomputed.
    pub fn needs_recompute(&self) -> bool {
        self.stale_extremes.iter().any(|stale| *stale)
    }

    /// Recomputes every retracted extreme from `members`, the rows of this group in the
    /// post-commit table (R012.2 `recompute_group_extreme`).
    pub fn recompute_extremes(
        &mut self,
        aggs: &[(String, ExactAgg)],
        members: &[&Row],
    ) -> Result<(), String> {
        for (index, (_, agg)) in aggs.iter().enumerate() {
            if !self.stale_extremes[index] {
                continue;
            }
            let (ExactAgg::Min(expr) | ExactAgg::Max(expr)) = agg else {
                continue;
            };
            let decimal = matches!(expr, RowExpr::Decimal(_));
            let is_min = matches!(agg, ExactAgg::Min(_));
            let mut best: Option<Value> = None;
            for row in members {
                let value = expr.eval(row)?;
                if value.is_null() {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some(held) => {
                        let order = compare_values(decimal, &value, held)?;
                        if is_min {
                            order == Ordering::Less
                        } else {
                            order == Ordering::Greater
                        }
                    }
                };
                if better {
                    best = Some(value);
                }
            }
            self.extremes[index] = best;
            self.stale_extremes[index] = false;
        }
        Ok(())
    }

    /// The stored result row of the group: the key values, the aggregate outputs, the hidden
    /// accumulators and `__mv_key`.
    pub fn to_row(
        &self,
        keys: &[(String, RowExpr)],
        aggs: &[(String, ExactAgg)],
        key: &str,
    ) -> Result<Row, String> {
        let mut row = Row::new();
        for ((name, _), value) in keys.iter().zip(&self.keys) {
            row.insert(name.clone(), value.clone());
        }
        for (index, ((name, agg), (sum, nonnull))) in aggs.iter().zip(&self.sums).enumerate() {
            let output = match agg {
                ExactAgg::Count => Value::from(self.count),
                ExactAgg::Sum(_) if *nonnull == 0 => Value::Null,
                ExactAgg::Sum(_) => sum.to_value(),
                ExactAgg::Avg(_) if *nonnull == 0 => Value::Null,
                ExactAgg::Avg(_) => Value::String(sum.average(*nonnull)?),
                ExactAgg::Min(_) | ExactAgg::Max(_) => {
                    self.extremes[index].clone().unwrap_or(Value::Null)
                }
            };
            row.insert(name.clone(), output);
            if matches!(agg, ExactAgg::Sum(_) | ExactAgg::Avg(_)) {
                row.insert(mv_sum_column(name), Value::String(sum.canonical()));
                row.insert(mv_nonnull_column(name), Value::from(*nonnull));
            }
        }
        row.insert(MV_COUNT.to_string(), Value::from(self.count));
        row.insert(MV_KEY.to_string(), Value::String(key.to_string()));
        Ok(row)
    }
}

/// Orders two non-NULL values of one column. A `decimal` operand compares by exact value, other
/// numbers by value (floats included), text as text and booleans false before true.
fn compare_values(decimal: bool, left: &Value, right: &Value) -> Result<Ordering, String> {
    if decimal {
        let read = |value: &Value| -> Result<(i128, u32), String> {
            number(from_value(value)?)?.ok_or_else(|| "ROW_EXPR_NOT_DECIMAL".to_string())
        };
        return compare_numbers(read(left)?, read(right)?);
    }
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => Ok(a.cmp(&b)),
            _ => a
                .as_f64()
                .partial_cmp(&b.as_f64())
                .ok_or_else(|| "ROW_EXPR_TYPE: a NaN does not order".to_string()),
        },
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        _ => Err("ROW_EXPR_TYPE: values of different types do not order".to_string()),
    }
}

/// The `__mv_key` of a group: the compact JSON array of its key values, `null` for a NULL.
pub fn group_key_text(values: &[Value]) -> String {
    Value::Array(values.to_vec()).to_string()
}

/// The row a join output expression sees: every column under `<table>.<column>`, plus the bare
/// column name where the two tables do not share it (MEDULLA-INCREMENTAL-VIEWS-R013.2).
pub fn merged_row(left_table: &str, left: &Row, right_table: &str, right: &Row) -> Row {
    let mut merged = Row::new();
    for (name, value) in left {
        merged.insert(format!("{left_table}.{name}"), value.clone());
        merged.insert(name.clone(), value.clone());
    }
    for (name, value) in right {
        merged.insert(format!("{right_table}.{name}"), value.clone());
        if !left.contains_key(name) {
            merged.insert(name.clone(), value.clone());
        }
    }
    merged
}

/// The key columns and the aggregates of a grouped view.
pub type GroupShape = (Vec<(String, RowExpr)>, Vec<(String, ExactAgg)>);

impl IncrementalViewProgram {
    /// The stored rows of a join view over the full source tables (R013.2): one per matching
    /// pair that the filter admits, keyed `<left key>|<right key>`.
    pub fn initial_join_rows(
        &self,
        left: &[(String, Row)],
        right: &[(String, Row)],
    ) -> Result<Vec<Row>, String> {
        let join = self
            .join
            .as_ref()
            .ok_or_else(|| "MATVIEW_PROGRAM: not a join view".to_string())?;
        let mut out = Vec::new();
        for (left_key, a) in left {
            let key = a.get(&join.left_key).unwrap_or(&Value::Null);
            if key.is_null() {
                continue;
            }
            for (right_key, b) in right {
                if b.get(&join.right_key) != Some(key) {
                    continue;
                }
                let merged = merged_row(&self.table, a, &join.right, b);
                if self.admits(&merged)? {
                    out.push(self.project(&merged, &format!("{left_key}|{right_key}"))?);
                }
            }
        }
        Ok(out)
    }

    /// Whether a source `row` enters the view: the filter is true over it.
    pub fn admits(&self, row: &Row) -> Result<bool, String> {
        match &self.filter {
            Some(filter) => filter.holds(row),
            None => Ok(true),
        }
    }

    /// The visible output row of a `Rows` view over source `row`.
    pub fn project(&self, row: &Row, key: &str) -> Result<Row, String> {
        let ViewOutput::Rows(columns) = &self.output else {
            return Err("MATVIEW_PROGRAM: not a rows view".to_string());
        };
        let mut out = Row::new();
        for (name, expr) in columns {
            out.insert(name.clone(), expr.eval(row)?);
        }
        out.insert(MV_KEY.to_string(), Value::String(key.to_string()));
        Ok(out)
    }

    /// The key columns and aggregates of a grouped view; a `Distinct` view is a grouped view
    /// of its output tuple with no aggregate.
    pub fn group_shape(&self) -> GroupShape {
        match &self.output {
            ViewOutput::Groups { keys, aggs } => (keys.clone(), aggs.clone()),
            ViewOutput::Distinct(columns) => (columns.clone(), Vec::new()),
            ViewOutput::Rows(_) => (Vec::new(), Vec::new()),
        }
    }

    /// The group key values of a `Groups` view over source `row`.
    pub fn group_values(&self, row: &Row) -> Result<Vec<Value>, String> {
        let (ViewOutput::Groups { keys, .. } | ViewOutput::Distinct(keys)) = &self.output else {
            return Err("MATVIEW_PROGRAM: not a groups view".to_string());
        };
        keys.iter().map(|(_, expr)| expr.eval(row)).collect()
    }

    /// Folds the source `rows` (each as `(primary key text, row)`) into the stored result rows
    /// of a view that starts empty. It is the same arithmetic as the delta path, applied as one
    /// batch of `Insert` changes (R009.5).
    pub fn initial_rows(&self, rows: &[(String, Row)]) -> Result<Vec<Row>, String> {
        match &self.output {
            ViewOutput::Rows(_) => {
                let mut out = Vec::new();
                for (key, row) in rows {
                    if self.admits(row)? {
                        out.push(self.project(row, key)?);
                    }
                }
                Ok(out)
            }
            ViewOutput::Groups { .. } | ViewOutput::Distinct(_) => {
                let (keys, aggs) = self.group_shape();
                let (keys, aggs) = (&keys[..], &aggs[..]);
                let mut groups: std::collections::BTreeMap<String, GroupState> =
                    std::collections::BTreeMap::new();
                for (_, row) in rows {
                    if !self.admits(row)? {
                        continue;
                    }
                    let values = self.group_values(row)?;
                    groups
                        .entry(group_key_text(&values))
                        .or_insert_with(|| GroupState::new(values, aggs.len()))
                        .apply(aggs, 1, row)?;
                }
                groups
                    .iter()
                    .map(|(key, group)| group.to_row(keys, aggs, key))
                    .collect()
            }
        }
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

    // proves R009.1: Model: the incremental view program
    // spec: MEDULLA-INCREMENTAL-VIEWS-R009.1
    #[test]
    fn row_expr_three_valued_logic() {
        let unknown = || column("b");
        // An unknown operand makes a comparison and arithmetic unknown.
        assert_eq!(eval(RowExpr::Eq(unknown(), literal(json!(1)))), Value::Null);
        assert_eq!(
            eval(RowExpr::Add(unknown(), literal(json!(1)))),
            Value::Null
        );
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
        assert_eq!(
            eval(RowExpr::Sub(price(), literal(json!("0.75")))),
            json!("11.75")
        );
        assert_eq!(
            eval(RowExpr::Sub(literal(json!("0.25")), price())),
            json!("-12.25")
        );
        // Decimals compare by value, not as text.
        assert_eq!(
            eval(RowExpr::Gt(price(), literal(json!("9.5")))),
            json!(true)
        );
        assert_eq!(
            eval(RowExpr::Eq(price(), literal(json!("12.5")))),
            json!(true)
        );
        // Text compares as text.
        assert_eq!(
            eval(RowExpr::Eq(column("name"), literal(json!("north")))),
            json!(true)
        );
        // Refusals.
        assert!(RowExpr::Column("missing".into()).eval(&row()).is_err());
        assert!(RowExpr::Literal(json!(1.5)).eval(&row()).is_err());
        assert!(RowExpr::Add(column("name"), column("a"))
            .eval(&row())
            .is_err());
        assert!(
            RowExpr::Mul(literal(json!(i64::MAX)), literal(json!(i64::MAX)))
                .eval(&row())
                .is_ok()
        );
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
                    (
                        "total".into(),
                        ExactAgg::Sum(RowExpr::Column("amount".into())),
                    ),
                    (
                        "mean".into(),
                        ExactAgg::Avg(RowExpr::Column("amount".into())),
                    ),
                ],
            },
        };
        let bytes = rmp_serde::to_vec_named(&program).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<IncrementalViewProgram>(&bytes).unwrap(),
            program
        );
        let rows = IncrementalViewProgram {
            output: ViewOutput::Rows(vec![("x".into(), RowExpr::Column("a".into()))]),
            ..program
        };
        let json = serde_json::to_string(&rows).unwrap();
        assert_eq!(
            serde_json::from_str::<IncrementalViewProgram>(&json).unwrap(),
            rows
        );
    }

    // proves R013.1: Model: join program
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
        assert_eq!(
            rmp_serde::from_slice::<IncrementalViewProgram>(&bytes).unwrap(),
            program
        );
        let json = serde_json::to_string(&program).unwrap();
        assert_eq!(
            serde_json::from_str::<IncrementalViewProgram>(&json).unwrap(),
            program
        );
        // A program stored before the join field existed reads as a single-table program.
        let mut value = serde_json::to_value(&program).unwrap();
        value.as_object_mut().unwrap().remove("join");
        let old: IncrementalViewProgram = serde_json::from_value(value).unwrap();
        assert_eq!(old.join, None);
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R012.1
    #[test]
    fn distinct_and_extremes_serialize() {
        let extremes = IncrementalViewProgram {
            view: "bounds".into(),
            table: "orders".into(),
            join: None,
            filter: None,
            output: ViewOutput::Groups {
                keys: vec![("region".into(), RowExpr::Column("region".into()))],
                aggs: vec![
                    ("lo".into(), ExactAgg::Min(RowExpr::Column("amount".into()))),
                    ("hi".into(), ExactAgg::Max(RowExpr::Column("amount".into()))),
                ],
            },
        };
        let distinct = IncrementalViewProgram {
            view: "regions".into(),
            output: ViewOutput::Distinct(vec![("region".into(), RowExpr::Column("region".into()))]),
            ..extremes.clone()
        };
        for program in [extremes, distinct] {
            let bytes = rmp_serde::to_vec_named(&program).unwrap();
            assert_eq!(
                rmp_serde::from_slice::<IncrementalViewProgram>(&bytes).unwrap(),
                program
            );
            let json = serde_json::to_string(&program).unwrap();
            assert_eq!(
                serde_json::from_str::<IncrementalViewProgram>(&json).unwrap(),
                program
            );
        }
        // A distinct view stores one group per output tuple with the hidden weight column.
        assert_eq!(MV_COUNT, "__mv_count");
    }
}
