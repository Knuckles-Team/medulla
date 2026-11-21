//! `RowSet` — the shared intermediate that flows between cross-modal operators.
//!
//! **The load-bearing design choice (CONCEPT:AU-KG.compute.vector).** Graph traversals,
//! relational filters and vector kNN all *produce and consume the same shape* — an
//! ordered set of node ids, each optionally carrying a score. So instead of three
//! incompatible result types (Arrow `RecordBatch` ↔ `Vec<NodeIndex>` ↔
//! `Vec<(id, f32)>`) every operator normalizes its output to a `RowSet`. That is
//! what makes the operators a *closed algebra*: the output of any op is a legal
//! input to any other op, so a plan is just `RowSet -> RowSet -> RowSet`.
//!
//! A `RowSet` is intentionally minimal — id + optional score, in order.
//! Order matters because RANK produces a meaningful order a downstream LIMIT must
//! respect; FILTER/TRAVERSE produce a *set* (discovery order, not semantically
//! meaningful until a RANK imposes one).
//!
//! Projected columns (carrying full property rows across the boundary instead of
//! bare ids) are an EXPLICIT later increment — see the crate docs. Graph rows carry
//! ids+scores only and re-materialize from the snapshot when an operator needs a column
//! (the FILTER leg decodes the blob itself).
//!
//! **Value channels (EH-521).** A series row (`TSSCAN`, `DERIVE`) also carries exact
//! `f64` values under names (`v0..vk`, a `DERIVE … AS name` alias) in a side table keyed
//! by row id — the `f32` score stays the canonical rank currency and is untouched. Ops
//! that keep their input rows (`limit`, `intersect_keep_order`, `DERIVE`) keep the
//! table; ops that rebuild rows from scratch start empty, and the UQL serve path has
//! already recorded the values it may `RETURN` by then.
//!
//! **One typed tabular channel (MEDULLA-FEDERATED-QUERY-R737).** Value and text columns
//! are not two side tables: every column a row carries is one typed [`Cell`] in one
//! table keyed by row id, in the order the columns were first written. The exact `f64`
//! channels are its `Float` cells and the text columns its `Text` cells
//! ([`RowSet::values`] and [`RowSet::texts`] are typed views of the same table), and
//! `GROUP` writes its keys and aggregates there too. [`RowSet::columns`] and
//! [`RowSet::typed_rows`] read the whole result as named, typed columns with `id` and
//! `score` as ordinary leading columns; `PROJECT` narrows that same table
//! ([`RowSet::project`]). There is no second result shape.

use std::collections::{BTreeMap, HashSet};

/// Row id → channel name → exact value.
pub type ValueChannels = BTreeMap<String, BTreeMap<String, f64>>;

/// Row id → column name → text value (the text columns of a typed source row, such as a web
/// result's title and engine; MEDULLA-UNIFIED-DATA-PLANE-R038.7.5).
pub type TextChannels = BTreeMap<String, BTreeMap<String, String>>;

/// One typed value of the tabular channel (MEDULLA-FEDERATED-QUERY-R737).
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
}

impl Cell {
    /// The column type this cell belongs to.
    pub fn column_type(&self) -> ColumnType {
        match self {
            Self::Int(_) => ColumnType::Int,
            Self::Float(_) => ColumnType::Float,
            Self::Text(_) => ColumnType::Text,
            Self::Bool(_) => ColumnType::Bool,
        }
    }

    /// The cell as a number, when it is one (an `Int` widens to `f64`).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(v) => Some(*v as f64),
            Self::Float(v) => Some(*v),
            Self::Text(_) | Self::Bool(_) => None,
        }
    }
}

/// The closed set of column types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnType {
    Int,
    Float,
    Text,
    Bool,
}

/// One named, typed column of a [`RowSet`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
}

/// The name of the leading id column of [`RowSet::columns`].
pub const ID_COLUMN: &str = "id";
/// The name of the score column of [`RowSet::columns`].
pub const SCORE_COLUMN: &str = "score";

/// Row id → column name → typed cell: the one tabular channel.
pub type CellTable = BTreeMap<String, BTreeMap<String, Cell>>;

/// The row id of point `ts_ns` of `series` (EH-521): `series@ts`. Per-series ids keep two
/// series' points at one timestamp distinct (a bare-ts id let the first series win).
pub fn series_row_id(series: &str, ts_ns: i64) -> String {
    format!("{series}@{ts_ns}")
}

/// Split a series row id into `(series, ts)`. The ts is the text after the LAST `@`, so a
/// series name may itself contain `@`. A bare integer id — a `WINDOW` bucket start — is
/// `("", ts)`. `None` for anything else (a graph node id).
pub fn parse_series_row_id(id: &str) -> Option<(&str, i64)> {
    match id.rsplit_once('@') {
        Some((series, ts)) => ts.parse().ok().map(|ts| (series, ts)),
        None => id.parse().ok().map(|ts| ("", ts)),
    }
}

/// The row id of a series EVENT at `ts_ns` (EH-529: `MOTIF`/`DISCORD` hits and `EVENTS`
/// shape rows): `series#key@ts`. `key` is the hit kind (`motif`/`discord`) or the
/// triggering `DERIVE` channel name — a later `CEP` stage matches on it.
pub fn series_event_row_id(series: &str, key: &str, ts_ns: i64) -> String {
    format!("{series}#{key}@{ts_ns}")
}

/// Split a series event row id into `(series, key, ts)` ([`series_event_row_id`]'s
/// inverse). `None` for anything else, including a plain [`parse_series_row_id`] id
/// (no `#`).
pub fn parse_series_event_row_id(id: &str) -> Option<(&str, &str, i64)> {
    let (head, ts) = id.rsplit_once('@')?;
    let (series, key) = head.rsplit_once('#')?;
    Some((series, key, ts.parse().ok()?))
}

/// One row: a node id and an optional score (similarity, pagerank, etc.). When a
/// `RowSet` has not been ranked, `score` is `None` for every row.
///
/// The epistemic fields (MEDULLA-FEDERATED-QUERY-R720.1) carry a row's belief
/// `confidence`, its `valid_time_ms` and its `tx_time_ms`. A stage that reads none of
/// them passes them through unchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub score: Option<f32>,
    pub confidence: Option<f32>,
    pub valid_time_ms: Option<i64>,
    pub tx_time_ms: Option<i64>,
}

impl Row {
    /// A row with `id` and `score` and no epistemic fields.
    pub fn new(id: String, score: Option<f32>) -> Self {
        Self {
            id,
            score,
            confidence: None,
            valid_time_ms: None,
            tx_time_ms: None,
        }
    }
}

/// An ordered set of rows. Deduplicated by id (a node appears at most once); the
/// retained occurrence is the FIRST inserted (or, after a rank, the ranked order).
///
/// This is the cross-modal currency: every operator returns a `RowSet`, so a plan
/// is just `RowSet -> RowSet -> RowSet`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowSet {
    rows: Vec<Row>,
    /// The typed cells of every row, keyed by row id (R737).
    cells: CellTable,
    /// Column names in the order they were first written.
    order: Vec<String>,
    /// MEDULLA-FEDERATED-QUERY-R039: set by a top-`k` rank that could not return `k`
    /// rows. `limit` keeps it; any step that rebuilds the rows starts without one.
    shortfall: Option<medulla_types::retrieval::Shortfall>,
}

impl RowSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from ids with no scores (a FILTER / TRAVERSE result — an unranked set).
    pub fn from_ids<I: IntoIterator<Item = String>>(ids: I) -> Self {
        let mut seen = HashSet::new();
        let rows = ids
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .map(|id| Row::new(id, None))
            .collect();
        Self::of_rows(rows)
    }

    fn of_rows(rows: Vec<Row>) -> Self {
        Self {
            rows,
            cells: CellTable::new(),
            order: Vec::new(),
            shortfall: None,
        }
    }

    /// Record why a top-`k` rank holds fewer than `k` rows (MEDULLA-FEDERATED-QUERY-R039).
    pub fn with_shortfall(mut self, shortfall: Option<medulla_types::retrieval::Shortfall>) -> Self {
        self.shortfall = shortfall;
        self
    }

    /// The typed shortfall of a top-`k` rank, when it could not return `k` rows.
    pub fn shortfall(&self) -> Option<&medulla_types::retrieval::Shortfall> {
        self.shortfall.as_ref()
    }

    /// Replace every cell of type `ty` with `incoming`, keeping cells of other types.
    /// Cells of ids not in the set are dropped, so the table never names a row the set
    /// does not hold.
    fn replace_typed(mut self, ty: ColumnType, incoming: CellTable) -> Self {
        for row in self.cells.values_mut() {
            row.retain(|_, cell| cell.column_type() != ty);
        }
        let ids: HashSet<String> = self.rows.iter().map(|r| r.id.clone()).collect();
        for (id, row) in incoming {
            if !ids.contains(&id) {
                continue;
            }
            for (name, cell) in row {
                self.note_column(&name);
                self.cells.entry(id.clone()).or_default().insert(name, cell);
            }
        }
        self.cells.retain(|_, row| !row.is_empty());
        self.prune_order();
        self
    }

    fn note_column(&mut self, name: &str) {
        if !self.order.iter().any(|known| known == name) {
            self.order.push(name.to_string());
        }
    }

    /// Drop column names no row carries any more.
    fn prune_order(&mut self) {
        let cells = &self.cells;
        self.order
            .retain(|name| cells.values().any(|row| row.contains_key(name)));
    }

    /// The cells of type `ty`, mapped out of the one table.
    fn typed_view<T>(
        &self,
        pick: impl Fn(&Cell) -> Option<T>,
    ) -> BTreeMap<String, BTreeMap<String, T>> {
        self.cells
            .iter()
            .filter_map(|(id, row)| {
                let picked: BTreeMap<String, T> = row
                    .iter()
                    .filter_map(|(name, cell)| pick(cell).map(|value| (name.clone(), value)))
                    .collect();
                (!picked.is_empty()).then(|| (id.clone(), picked))
            })
            .collect()
    }

    /// Attach text columns (row id → name → value) as `Text` cells, replacing the text
    /// cells already held. Columns of ids not in the set are dropped.
    pub fn with_texts(self, texts: TextChannels) -> Self {
        let incoming = texts
            .into_iter()
            .map(|(id, row)| {
                let row = row
                    .into_iter()
                    .map(|(name, value)| (name, Cell::Text(value)))
                    .collect();
                (id, row)
            })
            .collect();
        self.replace_typed(ColumnType::Text, incoming)
    }

    /// Row `id`'s text under column `name`.
    pub fn text(&self, id: &str, name: &str) -> Option<&str> {
        match self.cell(id, name) {
            Some(Cell::Text(value)) => Some(value.as_str()),
            _ => None,
        }
    }

    /// The text cells of the table (row id → name → value).
    pub fn texts(&self) -> TextChannels {
        self.typed_view(|cell| match cell {
            Cell::Text(value) => Some(value.clone()),
            _ => None,
        })
    }

    /// Row `id`'s text cells (name → value).
    pub fn row_texts(&self, id: &str) -> BTreeMap<String, String> {
        self.cells
            .get(id)
            .map(|row| {
                row.iter()
                    .filter_map(|(name, cell)| match cell {
                        Cell::Text(value) => Some((name.clone(), value.clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Attach exact value channels (row id → name → value) as `Float` cells, replacing
    /// the float cells already held. Channels of ids not in the set are dropped, so the
    /// table never names a row the set does not hold.
    pub fn with_values(self, values: ValueChannels) -> Self {
        let incoming = values
            .into_iter()
            .map(|(id, row)| {
                let row = row
                    .into_iter()
                    .map(|(name, value)| (name, Cell::Float(value)))
                    .collect();
                (id, row)
            })
            .collect();
        self.replace_typed(ColumnType::Float, incoming)
    }

    /// Row `id`'s exact value under channel `name` (a `Float` cell).
    pub fn value(&self, id: &str, name: &str) -> Option<f64> {
        match self.cell(id, name) {
            Some(Cell::Float(value)) => Some(*value),
            _ => None,
        }
    }

    /// The float cells of the table (row id → channel → value).
    pub fn values(&self) -> ValueChannels {
        self.typed_view(|cell| match cell {
            Cell::Float(value) => Some(*value),
            _ => None,
        })
    }

    /// The numeric cells of the table (`Int` and `Float`) as `f64`, row id → column →
    /// value: what a numeric result channel reads.
    pub fn numeric_values(&self) -> ValueChannels {
        self.typed_view(Cell::as_f64)
    }

    /// The names of the numeric (`Int` or `Float`) cell columns, in column order.
    pub fn numeric_columns(&self) -> Vec<String> {
        self.columns()
            .into_iter()
            .skip(2)
            .filter(|column| matches!(column.ty, ColumnType::Int | ColumnType::Float))
            .map(|column| column.name)
            .collect()
    }

    /// Split into rows and the value channels (an op that keeps its rows re-attaches
    /// them with [`RowSet::with_values`]).
    pub fn into_parts(self) -> (Vec<Row>, ValueChannels) {
        let values = self.values();
        (self.rows, values)
    }

    /// Row `id`'s cell under column `name`.
    pub fn cell(&self, id: &str, name: &str) -> Option<&Cell> {
        self.cells.get(id).and_then(|row| row.get(name))
    }

    /// Write one typed cell (the GROUP stage writes its keys and aggregates this way).
    /// A cell for an id the set does not hold is ignored.
    pub fn with_cell(mut self, id: &str, name: &str, cell: Cell) -> Self {
        if self.rows.iter().any(|r| r.id == id) {
            self.note_column(name);
            self.cells
                .entry(id.to_string())
                .or_default()
                .insert(name.to_string(), cell);
        }
        self
    }

    /// The named, typed columns of the result in order: `id` (text) and `score` (float)
    /// first, as ordinary columns, then every cell column in the order it was first
    /// written. A column's type is the type of its cells.
    pub fn columns(&self) -> Vec<Column> {
        let mut columns = vec![
            Column {
                name: ID_COLUMN.to_string(),
                ty: ColumnType::Text,
            },
            Column {
                name: SCORE_COLUMN.to_string(),
                ty: ColumnType::Float,
            },
        ];
        for name in &self.order {
            let ty = self
                .cells
                .values()
                .find_map(|row| row.get(name).map(Cell::column_type));
            if let Some(ty) = ty {
                columns.push(Column {
                    name: name.clone(),
                    ty,
                });
            }
        }
        columns
    }

    /// Every row as typed values in [`RowSet::columns`] order: the id, the score, then
    /// each column's cell (`None` where the row has no value).
    pub fn typed_rows(&self) -> Vec<Vec<Option<Cell>>> {
        let columns = self.columns();
        self.rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| match column.name.as_str() {
                        ID_COLUMN => Some(Cell::Text(row.id.clone())),
                        SCORE_COLUMN => row.score.map(|s| Cell::Float(f64::from(s))),
                        name => self.cell(&row.id, name).cloned(),
                    })
                    .collect()
            })
            .collect()
    }

    /// Keep only the named cell columns (PROJECT). `id` and `score` are always kept, and
    /// so are the text columns, which are the row's source provenance the encoder returns
    /// beside the channels; a name no row carries is ignored.
    pub fn project(mut self, names: &[String]) -> Self {
        for row in self.cells.values_mut() {
            row.retain(|name, cell| {
                cell.column_type() == ColumnType::Text || names.iter().any(|keep| keep == name)
            });
        }
        self.cells.retain(|_, row| !row.is_empty());
        self.prune_order();
        self
    }

    /// Keep the cells of the ids still in the set (after a truncation or a filter).
    fn retain_cells(mut self) -> Self {
        let ids: HashSet<String> = self.rows.iter().map(|r| r.id.clone()).collect();
        self.cells.retain(|id, _| ids.contains(id));
        self.prune_order();
        self
    }

    /// Build from scored ids (a RANK result — already in score order). Dedup keeps
    /// the first (highest-scoring, since the caller passes them ranked) occurrence.
    pub fn from_scored<I: IntoIterator<Item = (String, f32)>>(scored: I) -> Self {
        let mut seen = HashSet::new();
        let rows = scored
            .into_iter()
            .filter(|(id, _)| seen.insert(id.clone()))
            .map(|(id, s)| Row::new(id, Some(s)))
            .collect();
        Self::of_rows(rows)
    }

    /// Build from `(id, score?)` pairs preserving order, deduping by id (first wins).
    /// The inverse of reading `rows()` out — used by the WASM `Udf` op to rebuild the
    /// RowSet from a UDF's output rows (CONCEPT:MEDULLA-KG.query.rowset-execution).
    pub fn from_rows<I: IntoIterator<Item = (String, Option<f32>)>>(rows: I) -> Self {
        let mut seen = HashSet::new();
        let rows = rows
            .into_iter()
            .filter(|(id, _)| seen.insert(id.clone()))
            .map(|(id, score)| Row::new(id, score))
            .collect();
        Self::of_rows(rows)
    }

    /// Build from full rows, epistemic fields included, preserving order and deduping by
    /// id (first wins) (MEDULLA-FEDERATED-QUERY-R720.1).
    pub fn from_epistemic_rows<I: IntoIterator<Item = Row>>(rows: I) -> Self {
        let mut seen = HashSet::new();
        let rows = rows
            .into_iter()
            .filter(|r| seen.insert(r.id.clone()))
            .collect();
        Self::of_rows(rows)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The id set (membership), order-independent — used by TRAVERSE/RANK to know
    /// "which nodes are still candidates".
    pub fn id_set(&self) -> HashSet<&str> {
        self.rows.iter().map(|r| r.id.as_str()).collect()
    }

    pub fn ids(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.id.clone()).collect()
    }

    /// Keep only rows whose id is in `keep` (the cross-modal AND: e.g. "vector-ranked
    /// rows that ALSO passed the relational filter"). Preserves *self*'s order, so a
    /// vector-first plan that later intersects the filter set stays in rank order.
    /// This is the predicate-pushdown-across-a-modality-boundary primitive.
    pub fn intersect_keep_order(&self, keep: &HashSet<&str>) -> RowSet {
        let rows = self
            .rows
            .iter()
            .filter(|r| keep.contains(r.id.as_str()))
            .cloned()
            .collect();
        RowSet {
            rows,
            cells: self.cells.clone(),
            order: self.order.clone(),
            shortfall: None,
        }
        .retain_cells()
    }

    /// Drop the first `n` rows (SKIP, MEDULLA-FEDERATED-QUERY-R734).
    pub fn skip(mut self, n: usize) -> RowSet {
        self.rows.drain(..n.min(self.rows.len()));
        self.retain_cells()
    }

    /// Reorder the rows by `cmp` with a stable sort (ORDER, MEDULLA-FEDERATED-QUERY-R734).
    pub fn sorted_by<F>(mut self, cmp: F) -> RowSet
    where
        F: FnMut(&Row, &Row) -> std::cmp::Ordering,
    {
        self.rows.sort_by(cmp);
        self
    }

    /// Truncate to the top-k (LIMIT). Order-respecting: after a RANK this is top-k by
    /// score; on an unranked set it is the first k discovered.
    pub fn limit(mut self, k: usize) -> RowSet {
        self.rows.truncate(k);
        self.retain_cells()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MEDULLA-FEDERATED-QUERY-R720.1: the epistemic fields round-trip through
    /// `from_epistemic_rows` and survive the traversal (membership) and vector (top-k)
    /// stages; `from_rows` keeps its tuple shape and sets them to `None`.
    // spec: MEDULLA-FEDERATED-QUERY-R720.1
    #[test]
    fn epistemic_fields_survive_traversal_and_vector() {
        let a = Row {
            id: "a".into(),
            score: Some(0.9),
            confidence: Some(0.75),
            valid_time_ms: Some(1_000),
            tx_time_ms: Some(2_000),
        };
        let b = Row {
            id: "b".into(),
            score: Some(0.4),
            confidence: Some(0.5),
            valid_time_ms: Some(3_000),
            tx_time_ms: None,
        };
        let rs = RowSet::from_epistemic_rows([a.clone(), b.clone(), a.clone()]);
        assert_eq!(rs.rows(), &[a.clone(), b.clone()]);
        let keep: HashSet<&str> = ["a", "b"].into_iter().collect();
        let traversed = rs.intersect_keep_order(&keep);
        assert_eq!(traversed.rows(), &[a.clone(), b]);
        let top = traversed.limit(1);
        assert_eq!(top.rows(), &[a]);

        let plain = RowSet::from_rows([("x".to_string(), Some(1.0))]);
        let x = &plain.rows()[0];
        assert_eq!(
            (x.confidence, x.valid_time_ms, x.tx_time_ms),
            (None, None, None)
        );
    }

    /// MEDULLA-UNIFIED-DATA-PLANE-R038.7.8: text columns follow their rows through intersect and
    /// limit, and a column of an id outside the set is dropped.
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.7.8
    #[test]
    fn text_columns_survive_limit_and_intersect() {
        let texts: TextChannels = [
            ("a", "title", "A"),
            ("b", "title", "B"),
            ("z", "title", "Z"),
        ]
        .into_iter()
        .map(|(id, name, value)| {
            (
                id.to_string(),
                BTreeMap::from([(name.to_string(), value.to_string())]),
            )
        })
        .collect();
        let rows = RowSet::from_ids(["a".into(), "b".into()]).with_texts(texts);
        assert_eq!(rows.text("a", "title"), Some("A"));
        assert_eq!(
            rows.text("z", "title"),
            None,
            "a column of an absent row is dropped"
        );
        let keep: HashSet<&str> = ["b"].into_iter().collect();
        assert_eq!(
            rows.intersect_keep_order(&keep).text("b", "title"),
            Some("B")
        );
        let top = rows.limit(1);
        assert_eq!(top.text("a", "title"), Some("A"));
        assert_eq!(top.text("b", "title"), None);
    }

    #[test]
    fn dedup_keeps_first_occurrence() {
        let rs = RowSet::from_ids(["a".into(), "b".into(), "a".into()]);
        assert_eq!(rs.ids(), vec!["a", "b"]);
    }

    #[test]
    fn intersect_preserves_self_order() {
        let ranked = RowSet::from_scored([("b".into(), 0.9), ("a".into(), 0.5), ("c".into(), 0.1)]);
        let keep: HashSet<&str> = ["a", "b"].into_iter().collect();
        // Filter set is {a,b} but the RANK order (b,a) must be preserved.
        assert_eq!(ranked.intersect_keep_order(&keep).ids(), vec!["b", "a"]);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R030
    #[test]
    fn series_row_ids_round_trip() {
        assert_eq!(series_row_id("cpu@host", 7), "cpu@host@7");
        assert_eq!(parse_series_row_id("cpu@host@7"), Some(("cpu@host", 7)));
        assert_eq!(parse_series_row_id("-5"), Some(("", -5)));
        assert_eq!(parse_series_row_id("node-1"), None);
        assert_eq!(parse_series_row_id("a@b"), None);
    }

    #[test]
    fn series_event_row_ids_round_trip() {
        assert_eq!(
            series_event_row_id("svc.p95", "discord", 7),
            "svc.p95#discord@7"
        );
        assert_eq!(
            parse_series_event_row_id("svc.p95#discord@7"),
            Some(("svc.p95", "discord", 7))
        );
        assert_eq!(parse_series_event_row_id("svc.p95@7"), None, "no `#` key");
        assert_eq!(parse_series_event_row_id("node-1"), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R030
    #[test]
    fn value_channels_follow_kept_rows_only() {
        let values: ValueChannels = [("a", 1.5), ("b", 2.5), ("z", 9.0)]
            .into_iter()
            .map(|(id, v)| (id.to_string(), BTreeMap::from([("v0".to_string(), v)])))
            .collect();
        let rs = RowSet::from_ids(["a".into(), "b".into()]).with_values(values);
        assert_eq!(
            rs.value("z", "v0"),
            None,
            "a channel of an absent row is dropped"
        );
        let limited = rs.limit(1);
        assert_eq!(limited.value("a", "v0"), Some(1.5));
        assert_eq!(
            limited.value("b", "v0"),
            None,
            "LIMIT drops the cut row's channels"
        );
    }

    #[test]
    fn limit_truncates_in_order() {
        let rs = RowSet::from_ids(["a".into(), "b".into(), "c".into()]).limit(2);
        assert_eq!(rs.ids(), vec!["a", "b"]);
    }
}
