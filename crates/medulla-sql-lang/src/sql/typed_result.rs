//! Typed SQL result shapes: the pg-mappable column type, typed column and typed query result.

/// A coarse, Postgres-mappable column type derived from the Arrow result schema.
/// The pgwire shim (CONCEPT:AU-KG.query.raw-python) maps each to a wire type OID; the variants
/// cover exactly the Arrow types the `nodes`/`edges` schema-on-read inference and
/// ordinary SELECT projections produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgColType {
    Text,
    Int8,
    Float8,
    Bool,
    /// A pgvector `vector` column (CONCEPT:MEDULLA-KG.query.view-pgvector-operators) — a `List<Float32>` result column.
    /// The pgwire shim maps it to a stable float-array wire OID and renders each value
    /// as the pgvector text form `[1,2,3]`.
    Vector,
}

/// One typed result column: its name plus the pg-mappable type inferred from the
/// Arrow result schema.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedColumn {
    pub name: String,
    pub ty: PgColType,
}

/// The SQL result with per-column types and rows as decoded JSON values — the
/// shape the pgwire shim needs to emit a Postgres `RowDescription` (type OIDs) +
/// `DataRow`s. Reuses the SAME DataFusion exec path as [`exec_sql`]; the only
/// difference is it surfaces the Arrow column types and hands back JSON cells
/// instead of MessagePack blobs (no wire-protocol re-encode needed downstream).
/// `Debug`/`PartialEq` (CONCEPT:MEDULLA-KG.query.served-context-cache) let a test assert two runs —
/// e.g. the cached vs. the uncached SQL context path — produced an IDENTICAL result.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedQueryResult {
    pub columns: Vec<TypedColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

impl PgColType {
    /// The user-table column type that stores a result column of this type
    /// (MEDULLA-ENGINE-PARITY-R014.1): `Text` → `Text`, `Int8` → `BigInt`,
    /// `Float8` → `Double`, `Bool` → `Bool`, `Vector` → `Vector(None)`.
    pub fn stored_column_type(self) -> medulla_tables::schema::ColumnType {
        use medulla_tables::schema::ColumnType;
        match self {
            PgColType::Text => ColumnType::Text,
            PgColType::Int8 => ColumnType::BigInt,
            PgColType::Float8 => ColumnType::Double,
            PgColType::Bool => ColumnType::Bool,
            PgColType::Vector => ColumnType::Vector(None),
        }
    }
}

/// The schema of the result table of a materialized view named `name`: one
/// nullable, non-key column per result column (MEDULLA-ENGINE-PARITY-R014.1).
pub fn matview_result_schema(
    name: &str,
    columns: &[TypedColumn],
) -> medulla_tables::schema::TableSchema {
    medulla_tables::schema::TableSchema::new(
        name.to_string(),
        columns
            .iter()
            .map(|c| {
                medulla_tables::schema::Column::new(
                    c.name.clone(),
                    c.ty.stored_column_type(),
                    true,
                    false,
                )
            })
            .collect(),
    )
}
