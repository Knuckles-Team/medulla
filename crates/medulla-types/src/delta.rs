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
//! Lives in `medulla-types` so the table store and the circuit share one delta type.

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
}

impl From<Vec<ZRow>> for Delta {
    fn from(rows: Vec<ZRow>) -> Self {
        Delta { rows }
    }
}

/// Refusal raised when consolidating a [`Delta`] cannot represent the summed weight
/// (MEDULLA-INCREMENTAL-VIEWS-R002.2). Wire code `DELTA_WEIGHT_OVERFLOW`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeltaError {
    #[error("DELTA_WEIGHT_OVERFLOW: weight sum for row `{id}` overflows i64")]
    WeightOverflow { id: String },
}

impl Delta {
    /// Merge rows with equal `id` and equal `props` by adding weights with
    /// `i64::checked_add`, drop rows whose weight is zero, keep first-seen order.
    /// On overflow the delta is left unchanged and `WeightOverflow { id }` is returned.
    /// O(n log n): one sorted index over the rows.
    pub fn consolidate(&mut self) -> Result<(), DeltaError> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&self.rows[a], &self.rows[b]);
            x.id.cmp(&y.id)
                .then_with(|| cmp_props(&x.props, &y.props))
                .then(a.cmp(&b))
        });
        // (first-seen index, summed weight) per group.
        let mut groups: Vec<(usize, i64)> = Vec::new();
        let mut prev: Option<usize> = None;
        for &i in &order {
            let row = &self.rows[i];
            let same = prev.is_some_and(|p| {
                let q = &self.rows[p];
                q.id == row.id && q.props == row.props
            });
            if same {
                let last = groups.last_mut().expect("group exists when same");
                last.1 = last
                    .1
                    .checked_add(row.weight)
                    .ok_or_else(|| DeltaError::WeightOverflow { id: row.id.clone() })?;
            } else {
                groups.push((i, row.weight));
            }
            prev = Some(i);
        }
        groups.retain(|&(_, w)| w != 0);
        groups.sort_by_key(|&(first, _)| first);
        let mut rows: Vec<Option<ZRow>> = self.rows.drain(..).map(Some).collect();
        self.rows = groups
            .into_iter()
            .map(|(first, w)| {
                let mut r = rows[first].take().expect("first-seen row present");
                r.weight = w;
                r
            })
            .collect();
        Ok(())
    }
}

/// Total order over property maps (key order, then canonical JSON text of the value).
fn cmp_props(a: &Map<String, Value>, b: &Map<String, Value>) -> std::cmp::Ordering {
    let mut ia = a.iter();
    let mut ib = b.iter();
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((ka, va)), Some((kb, vb))) => {
                let o = ka.cmp(kb).then_with(|| va.to_string().cmp(&vb.to_string()));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.1
    #[test]
    fn insert_and_retract_carry_unit_weights() {
        let i = ZRow::insert("a", props(json!({"k": 1})));
        let r = ZRow::retract("a", props(json!({"k": 1})));
        assert_eq!((i.weight, r.weight), (1i64, -1i64));
        assert_eq!(i.num("k"), Some(1.0));
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.2
    #[test]
    fn consolidation_sums_weights_and_drops_zero() {
        let p = props(json!({"k": 1}));
        let q = props(json!({"k": 2}));
        let mut d = Delta::from(vec![
            ZRow::insert("b", p.clone()),
            ZRow::insert("a", p.clone()),
            ZRow::retract("a", p.clone()),
            ZRow::insert("c", q.clone()),
            ZRow::insert("b", p.clone()),
            ZRow::insert("c", p.clone()),
        ]);
        d.consolidate().unwrap();
        let got: Vec<(&str, i64)> = d.rows.iter().map(|r| (r.id.as_str(), r.weight)).collect();
        // `a` nets to zero and is dropped; same id with different props stays separate;
        // first-seen order is kept.
        assert_eq!(got, vec![("b", 2), ("c", 1), ("c", 1)]);
        assert_eq!(d.rows[1].props, q);
        assert_eq!(d.rows[2].props, p);
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.2
    #[test]
    fn weight_overflow_is_refused() {
        let p = props(json!({}));
        let mut big = ZRow::insert("x", p.clone());
        big.weight = i64::MAX;
        let mut d = Delta::from(vec![big, ZRow::insert("x", p)]);
        let err = d.consolidate().unwrap_err();
        assert_eq!(err, DeltaError::WeightOverflow { id: "x".into() });
        assert!(err.to_string().starts_with("DELTA_WEIGHT_OVERFLOW"));
        assert_eq!(
            d.rows.len(),
            2,
            "a refused consolidation leaves the delta intact"
        );
    }
}
