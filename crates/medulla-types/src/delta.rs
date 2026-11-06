//! The Z-set delta model for incremental materialized views
//! (MEDULLA-INCREMENTAL-VIEWS-R001, R002).
//!
//! `RowSet` is an **id-deduplicated ordered set** (`rowset.rs`), so the Z-set this
//! engine needs is the SPECIALIZATION of a general DBSP multiset to **signed
//! indicator weights**: a row is present with weight `+1` (insert) or `-1` (retract);
//! weight `0` is never stored. This is exactly the "update = retract the old value,
//! insert the new value" idiom the engine already captures in CDC
//! (`src/server/cdc.rs`'s `CdcPre`/`emit_for_method` carry a before/after image per
//! mutation), so translating one `CdcEvent` into a [`Delta`] needs no new capture code:
//!
//! | `CdcEvent.kind` | `Delta`                                   |
//! |-----------------|-------------------------------------------|
//! | `AddNode`       | `[(+1, id, props=after)]`                 |
//! | `RemoveNode`    | `[(-1, id, props=before)]`                |
//! | `UpdateNode`    | `[(-1, id, props=before), (+1, id, after)]` |
//!
//! A [`ZRow`] carries the row's decoded property map so the LINEAR stages
//! (`Scan`/`Filter`/`AsOf`) can evaluate their per-row predicate against the
//! before/after image WITHOUT a re-lookup into the graph — the delta is
//! self-describing, the DBSP property that makes a linear stage `∂L(ΔD) = L(ΔD)`.
//!
//! The type lives in `medulla-types` (MEDULLA-INCREMENTAL-VIEWS-R002.1) so every crate that
//! produces or folds a delta shares one definition. Weights are `i64`; [`Delta::consolidate`]
//! sums equal rows with checked arithmetic and refuses an overflow
//! (MEDULLA-INCREMENTAL-VIEWS-R002.2).

use serde_json::{Map, Value};

/// One signed row in a [`Delta`]: an id, an optional carried score, the row's decoded
/// property map (used by the linear stages' per-row predicates), and a signed
/// indicator `weight` (`+1` insert / `-1` retract). Weight `0` is never constructed.
#[derive(Clone, Debug, PartialEq)]
pub struct ZRow {
    pub id: String,
    pub score: Option<f32>,
    pub props: Map<String, Value>,
    pub weight: i64,
}

impl ZRow {
    /// A `+1` insert row for `id` carrying `props`.
    pub fn insert(id: impl Into<String>, props: Map<String, Value>) -> Self {
        ZRow {
            id: id.into(),
            score: None,
            props,
            weight: 1,
        }
    }

    /// A `-1` retract row for `id` carrying its pre-image `props`.
    pub fn retract(id: impl Into<String>, props: Map<String, Value>) -> Self {
        ZRow {
            id: id.into(),
            score: None,
            props,
            weight: -1,
        }
    }

    /// Read a numeric property (`None` if absent / non-numeric) — the shared primitive
    /// the `Filter`/`AsOf`/`WindowAgg` stages evaluate against.
    pub fn num(&self, field: &str) -> Option<f64> {
        self.props.get(field).and_then(Value::as_f64)
    }
}

/// A batch of signed rows applied to a circuit in one step. For a single mutation this
/// is at most two entries (an update's retract+insert of the same id); a seeding batch
/// may carry one `+1` per current node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delta {
    pub rows: Vec<ZRow>,
}

impl Delta {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, row: ZRow) {
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Merge rows with equal `id` and equal `props` by adding their weights with
    /// `i64::checked_add`, drop rows whose weight sums to zero, and keep first-seen order
    /// (MEDULLA-INCREMENTAL-VIEWS-R002.2). An overflow returns
    /// [`DeltaError::WeightOverflow`] and leaves `self` unchanged.
    pub fn consolidate(&mut self) -> Result<(), DeltaError> {
        let mut merged: Vec<ZRow> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            match merged
                .iter_mut()
                .find(|m| m.id == row.id && m.props == row.props)
            {
                Some(m) => {
                    m.weight = m
                        .weight
                        .checked_add(row.weight)
                        .ok_or_else(|| DeltaError::WeightOverflow { id: row.id.clone() })?;
                }
                None => merged.push(row.clone()),
            }
        }
        merged.retain(|r| r.weight != 0);
        self.rows = merged;
        Ok(())
    }
}

/// A refused delta operation (MEDULLA-INCREMENTAL-VIEWS-R002.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeltaError {
    /// Summing the weights of row `id` overflowed `i64`.
    WeightOverflow { id: String },
}

impl DeltaError {
    /// The wire refusal code of this error.
    pub fn code(&self) -> crate::contract::ServerErrorCode {
        match self {
            DeltaError::WeightOverflow { .. } => {
                crate::contract::ServerErrorCode::DeltaWeightOverflow
            }
        }
    }
}

impl std::fmt::Display for DeltaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeltaError::WeightOverflow { id } => {
                write!(f, "DELTA_WEIGHT_OVERFLOW: weight of row {id} overflows i64")
            }
        }
    }
}

impl std::error::Error for DeltaError {}

impl From<Vec<ZRow>> for Delta {
    fn from(rows: Vec<ZRow>) -> Self {
        Delta { rows }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    /// MEDULLA-INCREMENTAL-VIEWS-R002.1: insert and retract carry unit `i64` weights.
    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.1
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R002.1: Move and widen the delta type.
    #[test]
    fn insert_and_retract_carry_unit_weights() {
        let ins = ZRow::insert("a", props(json!({"year": 2001})));
        let ret = ZRow::retract("a", props(json!({"year": 2001})));
        assert_eq!((ins.weight, ret.weight), (1_i64, -1_i64));
        assert_eq!(ins.num("year"), Some(2001.0));
        let d = Delta::from(vec![ins, ret]);
        assert_eq!(d.len(), 2);
    }

    /// MEDULLA-INCREMENTAL-VIEWS-R002.2: equal rows sum, zero-weight rows drop, order is
    /// first-seen.
    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.2
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R002.2: Consolidate and refuse overflow.
    #[test]
    fn consolidation_sums_weights_and_drops_zero() {
        let p1 = props(json!({"v": 1}));
        let p2 = props(json!({"v": 2}));
        let mut d = Delta::from(vec![
            ZRow::insert("b", p1.clone()),
            ZRow::insert("a", p1.clone()),
            ZRow::retract("a", p2.clone()),
            ZRow::insert("b", p1.clone()),
            ZRow::retract("a", p1.clone()),
        ]);
        d.consolidate().expect("no overflow");
        let got: Vec<(&str, i64)> = d.rows.iter().map(|r| (r.id.as_str(), r.weight)).collect();
        assert_eq!(got, vec![("b", 2), ("a", -1)]);
        assert_eq!(d.rows[1].props, p2);
    }

    /// MEDULLA-INCREMENTAL-VIEWS-R002.2: an overflowing sum is refused with
    /// `DELTA_WEIGHT_OVERFLOW` and the delta is left unchanged.
    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.2
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R002.2: Consolidate and refuse overflow.
    #[test]
    fn weight_overflow_is_refused() {
        let mut big = ZRow::insert("a", Map::new());
        big.weight = i64::MAX;
        let mut d = Delta::from(vec![big.clone(), ZRow::insert("a", Map::new())]);
        let before = d.clone();
        let err = d.consolidate().expect_err("overflow refused");
        assert_eq!(err, DeltaError::WeightOverflow { id: "a".into() });
        assert_eq!(err.code().as_str(), "DELTA_WEIGHT_OVERFLOW");
        assert_eq!(d, before);
    }
}
