//! Durable, bounded secondary-index contracts for user tables.
//!
//! The graph engine already has a separate pgvector/ANN catalog.  This module is
//! deliberately narrower: it describes ordinary B-tree-like indexes over scalar
//! user-table columns and supplies the deterministic key encoding used by the
//! redb catalog and entry directory.  The catalog is owner-scoped by the
//! [`TableStore`](crate::tables::store::TableStore) handle; the scope is still part of
//! every durable key so a shared physical store cannot accidentally resolve an
//! index belonging to another tenant.
//!
//! An index is an optimization only.  A schema digest/version is stored with
//! every definition and a reader ignores stale or malformed definitions.  A
//! caller therefore falls back to a normal table scan rather than trusting a
//! directory that no longer describes the current rows.

use serde::{Deserialize, Serialize};

use crate::schema::{Cell, CheckExpr, ColumnType, TableSchema};

/// Version of the durable secondary-index definition and key encoding.
pub const SECONDARY_INDEX_SCHEMA_VERSION: u16 = 2;

/// Refusal code for an index expression outside the closed [`IndexExpr`] set.
pub const INDEX_EXPR_NONDETERMINISTIC: &str = "INDEX_EXPR_NONDETERMINISTIC";
/// Keep index construction and lookup bounded even when a caller supplies a
/// pathological DDL request.
pub const MAX_SECONDARY_INDEX_COLUMNS: usize = 4;
pub const MAX_SECONDARY_INDEXES_PER_TABLE: usize = 64;
pub const MAX_SECONDARY_INDEX_BUILD_ROWS: usize = 100_000;
pub const MAX_SECONDARY_INDEX_CANDIDATES: usize = 100_000;
pub const MAX_SECONDARY_INDEX_KEY_BYTES: usize = 16 * 1024;

/// The supported ordinary index family.  Keeping the enum closed prevents a
/// vector/ANN request from silently entering this scalar directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecondaryIndexKind {
    BTree,
}

/// Direction used by the optional ordered-read contract.  Entry keys are
/// stored in ascending canonical order; descending callers reverse the
/// deterministic row-id result rather than changing the durable key format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecondaryIndexOrder {
    Asc,
    Desc,
}

/// The closed set of deterministic one-column expressions an index key may use
/// (MEDULLA-ENGINE-PARITY-R010.1). Anything outside it (`now`, `random`,
/// `nextval`, ...) is refused with `INDEX_EXPR_NONDETERMINISTIC`, because an
/// entry computed from a non-repeatable value could never be found again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexExpr {
    Lower,
    Upper,
    Abs,
    Length,
}

impl IndexExpr {
    /// Resolve a SQL function name. Names outside the closed set are refused.
    pub fn parse(function: &str) -> Result<Self, String> {
        match function.to_ascii_lowercase().as_str() {
            "lower" => Ok(Self::Lower),
            "upper" => Ok(Self::Upper),
            "abs" => Ok(Self::Abs),
            "length" => Ok(Self::Length),
            other => Err(format!(
                "{INDEX_EXPR_NONDETERMINISTIC}: function `{other}` is not an allowed index expression (lower, upper, abs, length)"
            )),
        }
    }

    /// The column type this expression produces from a column of type `input`,
    /// or `None` when the expression does not apply to that type.
    pub fn output_type(self, input: ColumnType) -> Option<ColumnType> {
        match (self, input) {
            (Self::Lower | Self::Upper, ColumnType::Text) => Some(ColumnType::Text),
            (Self::Abs, ColumnType::Int | ColumnType::BigInt) => Some(input),
            (Self::Abs, ColumnType::Float | ColumnType::Double) => Some(input),
            (Self::Length, ColumnType::Text | ColumnType::Bytes) => Some(ColumnType::BigInt),
            _ => None,
        }
    }

    /// Apply the expression to one cell. NULL stays NULL.
    pub fn apply(self, cell: &Cell) -> Cell {
        match (self, cell) {
            (_, Cell::Null) => Cell::Null,
            (Self::Lower, Cell::Text(v)) => Cell::Text(v.to_lowercase()),
            (Self::Upper, Cell::Text(v)) => Cell::Text(v.to_uppercase()),
            (Self::Abs, Cell::Int(v)) => Cell::Int(v.saturating_abs()),
            (Self::Abs, Cell::Float(v)) => Cell::Float(v.abs()),
            (Self::Length, Cell::Text(v)) => Cell::Int(v.chars().count() as i64),
            (Self::Length, Cell::Bytes(v)) => Cell::Int(v.len() as i64),
            (_, other) => other.clone(),
        }
    }
}

/// One column in a secondary-index definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondaryIndexColumn {
    pub name: String,
    pub order: SecondaryIndexOrder,
    /// Optional deterministic expression over this column; the key stores the
    /// expression's result instead of the raw value.
    #[serde(default)]
    pub expr: Option<IndexExpr>,
}

impl SecondaryIndexColumn {
    pub fn ascending(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            order: SecondaryIndexOrder::Asc,
            expr: None,
        }
    }

    /// An ascending key over `expr(name)`.
    pub fn expression(name: impl Into<String>, expr: IndexExpr) -> Self {
        Self {
            expr: Some(expr),
            ..Self::ascending(name)
        }
    }

    /// The type of the stored key component.
    fn key_type(&self, schema: &TableSchema) -> Option<ColumnType> {
        let ty = schema.column(&self.name)?.ty;
        match self.expr {
            Some(expr) => expr.output_type(ty),
            None => Some(ty),
        }
    }

    fn key_cell(&self, cell: &Cell) -> Cell {
        match self.expr {
            Some(expr) => expr.apply(cell),
            None => cell.clone(),
        }
    }
}

/// Durable identity and schema binding for an ordinary index.
///
/// `tenant_scope` is not inferred from a SQL index name.  It is supplied by the
/// owner-scoped store and is included in the catalog and entry keys.  A service
/// that multiplexes tenants in one redb file must open a scoped
/// `TableStore`; a scope mismatch is rejected before any catalog mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondaryIndexSpec {
    pub tenant_scope: String,
    pub table: String,
    pub name: String,
    pub kind: SecondaryIndexKind,
    pub columns: Vec<SecondaryIndexColumn>,
    pub schema_version: u16,
    pub schema_digest: String,
    /// Partial index: an entry exists only for rows where this holds
    /// (MEDULLA-ENGINE-PARITY-R010.1). `None` indexes every row.
    #[serde(default)]
    pub predicate: Option<CheckExpr>,
    /// Unique among the rows the index covers: a second covered row with an equal
    /// non-NULL key is refused with `UNIQUE_VIOLATION`.
    #[serde(default)]
    pub unique: bool,
}

impl SecondaryIndexSpec {
    /// Build a schema-bound B-tree definition.  Callers should use the
    /// `TableStore::secondary_index_spec` helper so the store's owner scope is
    /// selected rather than copied from untrusted SQL text.
    pub fn btree(
        tenant_scope: impl Into<String>,
        table: impl Into<String>,
        name: impl Into<String>,
        columns: Vec<SecondaryIndexColumn>,
        schema: &TableSchema,
    ) -> Result<Self, String> {
        let spec = Self {
            tenant_scope: tenant_scope.into(),
            table: table.into(),
            name: name.into(),
            kind: SecondaryIndexKind::BTree,
            columns,
            schema_version: SECONDARY_INDEX_SCHEMA_VERSION,
            schema_digest: schema.schema_digest()?,
            predicate: None,
            unique: false,
        };
        validate_spec(&spec, schema)?;
        Ok(spec)
    }

    /// The same definition restricted to rows where `predicate` holds.
    pub fn with_predicate(
        mut self,
        predicate: CheckExpr,
        schema: &TableSchema,
    ) -> Result<Self, String> {
        self.predicate = Some(predicate);
        validate_spec(&self, schema)?;
        Ok(self)
    }

    /// The same definition made unique among its covered rows.
    pub fn into_unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Whether a plain first-column lookup on `column` may be answered by this
    /// index: it must cover every row and key the raw column value.
    pub fn serves_plain_lookup(&self, column: &str) -> bool {
        self.predicate.is_none()
            && self
                .columns
                .first()
                .is_some_and(|first| first.name == column && first.expr.is_none())
    }

    /// Does this index keep an entry for `cells`? A row outside the predicate has
    /// none. A predicate that cannot be evaluated leaves the row uncovered.
    pub fn covers_row(&self, schema: &TableSchema, cells: &[Cell]) -> bool {
        let Some(predicate) = &self.predicate else {
            return true;
        };
        let mut map = serde_json::Map::with_capacity(schema.columns().len());
        for (ci, col) in schema.columns().iter().enumerate() {
            let cell = cells.get(ci).cloned().unwrap_or(Cell::Null);
            map.insert(col.name.clone(), cell.to_typed_json(col.ty));
        }
        // `Cmp` against NULL passes a CHECK; an index predicate must be strict,
        // so a NULL operand never covers the row.
        predicate_columns(predicate)
            .iter()
            .all(|name| map.get(name).is_some_and(|v| !v.is_null()))
            && predicate.holds(&map)
    }

    /// Whether the key of `cells` has a NULL component (exempt from uniqueness).
    pub fn key_has_null(&self, schema: &TableSchema, cells: &[Cell]) -> bool {
        self.columns.iter().any(|indexed| {
            schema
                .column_index(&indexed.name)
                .and_then(|i| cells.get(i))
                .is_none_or(|cell| matches!(cell, Cell::Null))
        })
    }
}

/// Every column name a predicate reads.
fn predicate_columns(expr: &CheckExpr) -> Vec<String> {
    match expr {
        CheckExpr::Cmp { column, .. } | CheckExpr::In { column, .. } => vec![column.clone()],
        CheckExpr::IsNull { .. } => Vec::new(),
        CheckExpr::ColCmp { left, right, .. } => vec![left.clone(), right.clone()],
        CheckExpr::And(a, b) | CheckExpr::Or(a, b) => {
            let mut out = predicate_columns(a);
            out.extend(predicate_columns(b));
            out
        }
    }
}

/// Predicate shape that can be narrowed by the first column of a B-tree
/// definition.  DataFusion still re-applies the original expression above the
/// provider, so returning a candidate superset remains safe.
#[derive(Debug, Clone, PartialEq)]
pub enum SecondaryIndexLookup {
    Eq { column: String, value: Cell },
    Lt { column: String, value: Cell },
    Le { column: String, value: Cell },
    Gt { column: String, value: Cell },
    Ge { column: String, value: Cell },
}

impl SecondaryIndexLookup {
    /// The `(column, value)` a lookup binds — every comparison carries both.
    fn parts(&self) -> (&str, &Cell) {
        match self {
            Self::Eq { column, value }
            | Self::Lt { column, value }
            | Self::Le { column, value }
            | Self::Gt { column, value }
            | Self::Ge { column, value } => (column, value),
        }
    }

    pub fn column(&self) -> &str {
        self.parts().0
    }

    fn value(&self) -> &Cell {
        self.parts().1
    }
}

/// Validate a durable definition against the current schema.  This is called
/// both on CREATE and on every read; a corrupt/stale persisted definition is
/// therefore fail-closed to a scan.
pub fn validate_spec(spec: &SecondaryIndexSpec, schema: &TableSchema) -> Result<(), String> {
    schema.validate()?;
    validate_spec_binding(spec, schema)?;
    validate_spec_columns(spec, schema)
}

/// The index's identity and its binding to one schema version: format version, table,
/// non-empty NUL-free names, the column-count bound, and the schema digest it was built
/// against. A stale digest is fail-closed, so a changed table falls back to a scan.
fn validate_spec_binding(spec: &SecondaryIndexSpec, schema: &TableSchema) -> Result<(), String> {
    if spec.schema_version != SECONDARY_INDEX_SCHEMA_VERSION {
        return Err(format!(
            "secondary index `{}` uses unsupported schema version {}",
            spec.name, spec.schema_version
        ));
    }
    if spec.table != schema.name {
        return Err(format!(
            "secondary index `{}` belongs to table `{}`, not `{}`",
            spec.name, spec.table, schema.name
        ));
    }
    // Scope, table, and name are stored verbatim in the catalog key.
    if [&spec.tenant_scope, &spec.table, &spec.name]
        .iter()
        .any(|value| value.is_empty() || value.contains('\0'))
    {
        return Err("secondary index scope, table, and name must be non-empty and NUL-free".into());
    }
    if spec.columns.is_empty() || spec.columns.len() > MAX_SECONDARY_INDEX_COLUMNS {
        return Err(format!(
            "secondary index `{}` must contain 1..={} columns",
            spec.name, MAX_SECONDARY_INDEX_COLUMNS
        ));
    }
    if spec.schema_digest != schema.schema_digest()? {
        return Err(format!(
            "secondary index `{}` is stale for table `{}` (schema digest mismatch)",
            spec.name, spec.table
        ));
    }
    Ok(())
}

/// Every key column: ascending, distinct, present in the schema, and of an indexable
/// type.
fn validate_spec_columns(spec: &SecondaryIndexSpec, schema: &TableSchema) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for indexed in &spec.columns {
        if indexed.order != SecondaryIndexOrder::Asc {
            return Err(format!(
                "secondary index `{}` only supports ascending key columns; use ordered read DESC for reverse pagination",
                spec.name
            ));
        }
        if !seen.insert(indexed.name.as_str()) {
            return Err(format!(
                "secondary index `{}` repeats column `{}`",
                spec.name, indexed.name
            ));
        }
        let column = schema.column(&indexed.name).ok_or_else(|| {
            format!(
                "secondary index `{}` references unknown column `{}`",
                spec.name, indexed.name
            )
        })?;
        if !is_indexable(column.ty) {
            return Err(format!(
                "secondary index `{}` cannot index vector, JSON, or array column `{}`",
                spec.name, indexed.name
            ));
        }
        if indexed.key_type(schema).is_none() {
            return Err(format!(
                "{INDEX_EXPR_NONDETERMINISTIC}: expression does not apply to column `{}` of secondary index `{}`",
                indexed.name, spec.name
            ));
        }
    }
    if let Some(predicate) = &spec.predicate {
        predicate
            .validate_limits(0)
            .map_err(|error| format!("secondary index `{}` predicate: {error}", spec.name))?;
        let mut referenced = Vec::new();
        predicate.referenced_columns(&mut referenced);
        if let Some(unknown) = referenced.iter().find(|name| schema.column(name).is_none()) {
            return Err(format!(
                "secondary index `{}` predicate references unknown column `{unknown}`",
                spec.name
            ));
        }
    }
    Ok(())
}

/// Scalar types have a stable total ordering and a bounded key representation.
/// Vectors remain exclusively in the ANN catalog; JSON/arrays are intentionally
/// unsupported rather than inventing a partial ordering.
pub fn is_indexable(ty: ColumnType) -> bool {
    matches!(
        ty,
        ColumnType::Int
            | ColumnType::BigInt
            | ColumnType::Float
            | ColumnType::Double
            | ColumnType::Text
            | ColumnType::Bool
            | ColumnType::Timestamp
            | ColumnType::TimestampTz
            | ColumnType::Bytes
            | ColumnType::Uuid
            | ColumnType::Numeric(_)
    )
}

/// Stable catalog key: scope + table + index name.  Each component is validated
/// by `validate_spec` before it reaches redb.
pub fn catalog_key(spec: &SecondaryIndexSpec) -> String {
    format!("{}\0{}\0{}", spec.tenant_scope, spec.table, spec.name)
}

/// Prefix of all physical entries belonging to a definition.
pub fn entry_prefix(spec: &SecondaryIndexSpec) -> String {
    format!("{}\0", catalog_key(spec))
}

/// Read the physical row id encoded at the end of an entry key.
pub fn rowid_from_entry_key(key: &str) -> Option<u64> {
    key.rsplit('\0').next()?.parse().ok()
}

/// Encode a row's indexed values into an order-preserving byte string.  The
/// store converts this to hex before using it as part of a redb string key.
pub fn row_key(
    spec: &SecondaryIndexSpec,
    schema: &TableSchema,
    cells: &[Cell],
) -> Result<Vec<u8>, String> {
    validate_spec(spec, schema)?;
    let mut out = Vec::new();
    for indexed in &spec.columns {
        let index = schema
            .column_index(&indexed.name)
            .ok_or_else(|| format!("indexed column `{}` is absent", indexed.name))?;
        let cell = cells.get(index).unwrap_or(&Cell::Null);
        let ty = indexed
            .key_type(schema)
            .ok_or_else(|| format!("indexed column `{}` has no key type", indexed.name))?;
        encode_cell(&mut out, &indexed.key_cell(cell), ty)?;
    }
    if out.len() > MAX_SECONDARY_INDEX_KEY_BYTES {
        return Err(format!(
            "secondary index `{}` key exceeds {} bytes",
            spec.name, MAX_SECONDARY_INDEX_KEY_BYTES
        ));
    }
    Ok(out)
}

/// Construct a physical entry key.  A fixed-width row id makes iteration
/// deterministic for equal indexed values while the encoded value remains the
/// primary B-tree order key.
pub fn entry_key(
    spec: &SecondaryIndexSpec,
    schema: &TableSchema,
    cells: &[Cell],
    rowid: u64,
) -> Result<String, String> {
    let encoded = hex::encode(row_key(spec, schema, cells)?);
    Ok(format!("{}{}\0{rowid:020}", entry_prefix(spec), encoded))
}

/// Compute an inclusive/exclusive lexical range over entry keys for a lookup on
/// the first indexed column.  A high Unicode sentinel includes all composite
/// suffixes for an inclusive bound while remaining above the NUL row-id
/// separator.  Unsupported shapes return `None`, which is the explicit scan
/// fallback contract.
pub fn entry_range(
    spec: &SecondaryIndexSpec,
    schema: &TableSchema,
    lookup: &SecondaryIndexLookup,
) -> Result<Option<(String, String)>, String> {
    validate_spec(spec, schema)?;
    if !spec.serves_plain_lookup(lookup.column()) {
        return Ok(None);
    }
    let ty = schema
        .column(lookup.column())
        .ok_or_else(|| format!("indexed column `{}` is absent", lookup.column()))?
        .ty;
    let mut component = Vec::new();
    encode_cell(&mut component, lookup.value(), ty)?;
    let encoded = hex::encode(component);
    let prefix = entry_prefix(spec);
    let low = format!("{prefix}{encoded}");
    let high = format!("{prefix}{encoded}\u{10ffff}");
    let all_low = prefix.clone();
    let all_high = format!("{prefix}\u{10ffff}");
    let bounds = match lookup {
        SecondaryIndexLookup::Eq { .. } => (low, high),
        SecondaryIndexLookup::Lt { .. } => (all_low, low),
        SecondaryIndexLookup::Le { .. } => (all_low, high),
        SecondaryIndexLookup::Gt { .. } => (high, all_high),
        SecondaryIndexLookup::Ge { .. } => (low, all_high),
    };
    Ok(Some(bounds))
}

/// The order-preserving big-endian form of a float key: the sign bit is flipped for a
/// positive value and every bit inverted for a negative one, so byte order is numeric
/// order. SQL equality treats -0.0 and +0.0 as one value, so the sign is normalized first.
fn sortable_float_bits(value: f64) -> Result<u64, String> {
    if !value.is_finite() {
        return Err("non-finite floating values are not indexable".into());
    }
    let bits = if value == 0.0 { 0.0f64 } else { value }.to_bits();
    Ok(if bits & (1u64 << 63) != 0 {
        !bits
    } else {
        bits ^ (1u64 << 63)
    })
}

/// Append a NUL-escaped, NUL-NUL-terminated component. The escape preserves lexical order
/// for strings and gives a prefix range for composite indexes.
fn push_escaped(out: &mut Vec<u8>, payload: &[u8]) {
    for &byte in payload {
        if byte == 0 {
            out.extend_from_slice(&[0, 0xff]);
        } else {
            out.push(byte);
        }
    }
    out.extend_from_slice(&[0, 0]);
}

fn encode_cell(out: &mut Vec<u8>, cell: &Cell, ty: ColumnType) -> Result<(), String> {
    // Null is indexed but no SQL NULL predicate is planned here; it remains available to
    // future IS NULL support without changing the durable encoding.
    let (tag, payload): (u8, Vec<u8>) = match (cell, ty) {
        (Cell::Null, _) => (0x00, Vec::new()),
        (Cell::Int(value), ColumnType::Int | ColumnType::BigInt) => (
            0x10,
            ((*value as u64) ^ (1u64 << 63)).to_be_bytes().to_vec(),
        ),
        (Cell::Float(value), ColumnType::Float | ColumnType::Double | ColumnType::Numeric(_)) => {
            (0x11, sortable_float_bits(*value)?.to_be_bytes().to_vec())
        }
        (Cell::Timestamp(value), ColumnType::Timestamp | ColumnType::TimestampTz) => (
            0x12,
            ((*value as u64) ^ (1u64 << 63)).to_be_bytes().to_vec(),
        ),
        (Cell::Text(value), ColumnType::Text | ColumnType::Uuid) => {
            (0x20, value.as_bytes().to_vec())
        }
        (Cell::Bool(value), ColumnType::Bool) => (0x30, vec![u8::from(*value)]),
        (Cell::Bytes(value), ColumnType::Bytes) => (0x40, value.clone()),
        _ => {
            return Err(format!(
                "cell shape does not match indexable column type {ty:?}"
            ))
        }
    };
    out.push(tag);
    push_escaped(out, &payload);
    Ok(())
}
