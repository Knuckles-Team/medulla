//! Raw catalog read of a live PostgreSQL server (MEDULLA-UNIFIED-DATA-PLANE-R013.1).
//!
//! One pool of at most `pool_size` connections reads the version and the tables, columns, primary
//! and unique keys and view definitions (`pg_views`) of the allowed schemas. Every value comes
//! back as plain text; the catalog assembly is `medulla_plan::postgres_catalog`. Errors carry no
//! DSN.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// One column of a base table; `udt` is the type name as `pg_catalog` spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgColumn {
    /// The `schema` value.
    pub schema: String,
    /// The `table` value.
    pub table: String,
    /// The `name` value.
    pub name: String,
    /// The `udt` value.
    pub udt: String,
    /// The `nullable` value.
    pub nullable: bool,
}

/// One column of a primary key or unique constraint, in key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgKeyColumn {
    /// The `schema` value.
    pub schema: String,
    /// The `table` value.
    pub table: String,
    /// The `constraint` value.
    pub constraint: String,
    /// The `primary` value.
    pub primary: bool,
    /// The `column` value.
    pub column: String,
}

/// One view and its definition SQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgView {
    /// The `schema` value.
    pub schema: String,
    /// The `name` value.
    pub name: String,
    /// The `definition` value.
    pub definition: String,
}

/// What one read of the server's catalog returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PgCatalogRows {
    /// The text of `SHOW server_version`.
    pub version: String,
    /// `(schema, name)` of every base table.
    pub tables: Vec<(String, String)>,
    /// The `columns` value.
    pub columns: Vec<PgColumn>,
    /// The `keys` value.
    pub keys: Vec<PgKeyColumn>,
    /// The `views` value.
    pub views: Vec<PgView>,
}

const SYSTEM_SCHEMAS: &str = "('pg_catalog', 'information_schema')";

async fn rows(
    pool: &PgPool,
    sql: &str,
    schemas: &[String],
) -> Result<Vec<sqlx::postgres::PgRow>, String> {
    let query = sqlx::query(sqlx::AssertSqlSafe(sql.to_string())).bind(schemas.to_vec());
    tokio::time::timeout(READ_TIMEOUT, query.fetch_all(pool))
        .await
        .map_err(|_| "federation: postgres catalog read timed out".to_string())?
        .map_err(|_| "federation: postgres catalog read failed".to_string())
}

fn text(row: &sqlx::postgres::PgRow, index: usize) -> Result<String, String> {
    row.try_get::<Option<String>, _>(index)
        .map(Option::unwrap_or_default)
        .map_err(|_| "federation: postgres catalog column undecodable".to_string())
}

/// The scope filter: the listed schemas, or every non-system schema when the list is empty.
fn scope(column: &str) -> String {
    format!(
        "(cardinality($1::text[]) = 0 AND {column} NOT IN {SYSTEM_SCHEMAS} AND {column} NOT LIKE 'pg\\_%' \
         OR {column} = ANY($1::text[]))"
    )
}

/// Read the catalog of `schemas` through a pool of at most `pool_size` connections.
pub async fn read_pg_catalog(
    dsn: &str,
    schemas: &[String],
    pool_size: u32,
) -> Result<PgCatalogRows, String> {
    let pool = tokio::time::timeout(
        CONNECT_TIMEOUT,
        PgPoolOptions::new()
            .max_connections(pool_size.max(1))
            .acquire_timeout(CONNECT_TIMEOUT)
            .connect(dsn),
    )
    .await
    .map_err(|_| "federation: postgres connection timed out".to_string())?
    .map_err(|_| "federation: postgres connection failed".to_string())?;
    let result = read_with(&pool, schemas).await;
    pool.close().await;
    result
}

async fn read_with(pool: &PgPool, schemas: &[String]) -> Result<PgCatalogRows, String> {
    let mut out = PgCatalogRows {
        version: sqlx::query_scalar::<_, String>("SHOW server_version")
            .fetch_one(pool)
            .await
            .map_err(|_| "federation: postgres version unavailable".to_string())?,
        ..PgCatalogRows::default()
    };
    for row in rows(
        pool,
        &format!(
            "SELECT table_schema::text, table_name::text FROM information_schema.tables \
             WHERE table_type = 'BASE TABLE' AND {} ORDER BY 1, 2",
            scope("table_schema")
        ),
        schemas,
    )
    .await?
    {
        out.tables.push((text(&row, 0)?, text(&row, 1)?));
    }
    for row in rows(
        pool,
        &format!(
            "SELECT table_schema::text, table_name::text, column_name::text, udt_name::text, \
             is_nullable::text FROM information_schema.columns WHERE {} \
             ORDER BY table_schema, table_name, ordinal_position",
            scope("table_schema")
        ),
        schemas,
    )
    .await?
    {
        out.columns.push(PgColumn {
            schema: text(&row, 0)?,
            table: text(&row, 1)?,
            name: text(&row, 2)?,
            udt: text(&row, 3)?,
            nullable: text(&row, 4)?.eq_ignore_ascii_case("YES"),
        });
    }
    for row in rows(
        pool,
        &format!(
            "SELECT k.table_schema::text, k.table_name::text, k.constraint_name::text, \
             t.constraint_type::text, k.column_name::text \
             FROM information_schema.key_column_usage k \
             JOIN information_schema.table_constraints t \
               ON t.constraint_schema = k.constraint_schema AND t.constraint_name = k.constraint_name \
              AND t.table_schema = k.table_schema AND t.table_name = k.table_name \
             WHERE t.constraint_type IN ('PRIMARY KEY', 'UNIQUE') AND {} \
             ORDER BY k.table_schema, k.table_name, k.constraint_name, k.ordinal_position",
            scope("k.table_schema")
        ),
        schemas,
    )
    .await?
    {
        out.keys.push(PgKeyColumn {
            schema: text(&row, 0)?,
            table: text(&row, 1)?,
            constraint: text(&row, 2)?,
            primary: text(&row, 3)? == "PRIMARY KEY",
            column: text(&row, 4)?,
        });
    }
    for row in rows(
        pool,
        &format!(
            "SELECT schemaname::text, viewname::text, definition::text FROM pg_views \
             WHERE {} ORDER BY 1, 2",
            scope("schemaname")
        ),
        schemas,
    )
    .await?
    {
        out.views.push(PgView {
            schema: text(&row, 0)?,
            name: text(&row, 1)?,
            definition: text(&row, 2)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R013.1
    #[tokio::test]
    async fn an_unreachable_server_is_an_error_without_the_dsn() {
        let error = read_pg_catalog("postgres://user:secret@127.0.0.1:1/db", &[], 1)
            .await
            .unwrap_err();
        assert!(error.starts_with("federation: postgres connection"));
        assert!(!error.contains("secret"));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R013.1
    #[test]
    fn scope_keeps_system_schemas_out_of_an_empty_list() {
        let filter = scope("s");
        assert!(filter.contains("cardinality($1::text[]) = 0"));
        assert!(filter.contains("'pg_catalog'"));
        assert!(filter.contains("s = ANY($1::text[])"));
    }
}
