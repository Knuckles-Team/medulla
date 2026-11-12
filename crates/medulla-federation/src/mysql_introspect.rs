//! Raw `information_schema` read of a live MySQL or MariaDB server (MEDULLA-UNIFIED-DATA-PLANE-R014.2.2).
//!
//! One pool of at most `pool_size` connections reads the version and the TABLES, COLUMNS,
//! KEY_COLUMN_USAGE, STATISTICS and VIEWS of the allowed schemas (plus the CHECK_CONSTRAINTS a
//! MariaDB server uses to mark JSON columns). Every value comes back as plain text or an integer;
//! the type map and the catalog assembly are `medulla_plan::mysql_catalog`. Errors carry no DSN.

use std::time::Duration;

use sqlx::mysql::MySqlPoolOptions;
use sqlx::{MySqlPool, Row};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// One COLUMNS row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsColumn {
    pub schema: String,
    pub table: String,
    pub name: String,
    pub data_type: String,
    pub column_type: String,
    pub nullable: bool,
}

/// One KEY_COLUMN_USAGE row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsKeyColumn {
    pub schema: String,
    pub table: String,
    pub constraint: String,
    pub column: String,
    pub referenced: Option<(String, String, String)>,
}

/// One STATISTICS row of a non-unique index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsIndexColumn {
    pub schema: String,
    pub table: String,
    pub index: String,
    pub column: String,
}

/// One VIEWS row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsView {
    pub schema: String,
    pub name: String,
    pub definition: String,
}

/// What one read of the server's `information_schema` returned. Columns, key columns and index
/// columns are in ordinal order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InformationSchema {
    /// The text of `SELECT VERSION()`.
    pub version: String,
    /// `(schema, name)` of every base table.
    pub tables: Vec<(String, String)>,
    pub columns: Vec<IsColumn>,
    pub keys: Vec<IsKeyColumn>,
    pub indexes: Vec<IsIndexColumn>,
    pub views: Vec<IsView>,
    /// `(schema, table, check clause)`; read on MariaDB only.
    pub checks: Vec<(String, String, String)>,
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

async fn rows(
    pool: &MySqlPool,
    sql: &str,
    schemas: &[String],
) -> Result<Vec<sqlx::mysql::MySqlRow>, String> {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(
        sql.replace("{IN}", &placeholders(schemas.len())),
    ));
    for schema in schemas {
        query = query.bind(schema.clone());
    }
    tokio::time::timeout(READ_TIMEOUT, query.fetch_all(pool))
        .await
        .map_err(|_| "federation: mysql catalog read timed out".to_string())?
        .map_err(|_| "federation: mysql catalog read failed".to_string())
}

fn text(row: &sqlx::mysql::MySqlRow, index: usize) -> Result<String, String> {
    row.try_get::<Option<String>, _>(index)
        .map(Option::unwrap_or_default)
        .map_err(|_| "federation: mysql catalog column undecodable".to_string())
}

/// Read the catalog of `schemas` (the connection's own database when empty) through a pool of
/// at most `pool_size` connections.
pub async fn read_information_schema(
    dsn: &str,
    schemas: &[String],
    pool_size: u32,
) -> Result<InformationSchema, String> {
    let pool = tokio::time::timeout(
        CONNECT_TIMEOUT,
        MySqlPoolOptions::new()
            .max_connections(pool_size.max(1))
            .acquire_timeout(CONNECT_TIMEOUT)
            .connect(dsn),
    )
    .await
    .map_err(|_| "federation: mysql connection timed out".to_string())?
    .map_err(|_| "federation: mysql connection failed".to_string())?;
    let result = read_with(&pool, schemas).await;
    pool.close().await;
    result
}

async fn read_with(pool: &MySqlPool, schemas: &[String]) -> Result<InformationSchema, String> {
    let mut out = InformationSchema::default();
    out.version = sqlx::query_scalar::<_, String>("SELECT CAST(VERSION() AS CHAR)")
        .fetch_one(pool)
        .await
        .map_err(|_| "federation: mysql version unavailable".to_string())?;
    let scope: Vec<String> = if schemas.is_empty() {
        let current: Option<String> = sqlx::query_scalar("SELECT CAST(DATABASE() AS CHAR)")
            .fetch_one(pool)
            .await
            .map_err(|_| "federation: mysql database unavailable".to_string())?;
        current.into_iter().collect()
    } else {
        schemas.to_vec()
    };
    if scope.is_empty() {
        return Ok(out);
    }
    for row in rows(
        pool,
        "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR) FROM information_schema.tables \
         WHERE table_schema IN ({IN}) AND table_type <> 'VIEW' ORDER BY 1, 2",
        &scope,
    )
    .await?
    {
        out.tables.push((text(&row, 0)?, text(&row, 1)?));
    }
    for row in rows(
        pool,
        "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR), CAST(column_name AS CHAR), \
         CAST(data_type AS CHAR), CAST(column_type AS CHAR), CAST(is_nullable AS CHAR) \
         FROM information_schema.columns WHERE table_schema IN ({IN}) \
         ORDER BY table_schema, table_name, ordinal_position",
        &scope,
    )
    .await?
    {
        out.columns.push(IsColumn {
            schema: text(&row, 0)?,
            table: text(&row, 1)?,
            name: text(&row, 2)?,
            data_type: text(&row, 3)?,
            column_type: text(&row, 4)?,
            nullable: text(&row, 5)?.eq_ignore_ascii_case("YES"),
        });
    }
    for row in rows(
        pool,
        "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR), CAST(constraint_name AS CHAR), \
         CAST(column_name AS CHAR), CAST(referenced_table_schema AS CHAR), \
         CAST(referenced_table_name AS CHAR), CAST(referenced_column_name AS CHAR) \
         FROM information_schema.key_column_usage WHERE table_schema IN ({IN}) \
         ORDER BY table_schema, table_name, constraint_name, ordinal_position",
        &scope,
    )
    .await?
    {
        let referenced_schema = row
            .try_get::<Option<String>, _>(4)
            .map_err(|_| "federation: mysql catalog column undecodable".to_string())?;
        let referenced = match referenced_schema {
            Some(schema) => Some((schema, text(&row, 5)?, text(&row, 6)?)),
            None => None,
        };
        out.keys.push(IsKeyColumn {
            schema: text(&row, 0)?,
            table: text(&row, 1)?,
            constraint: text(&row, 2)?,
            column: text(&row, 3)?,
            referenced,
        });
    }
    for row in rows(
        pool,
        "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR), CAST(index_name AS CHAR), \
         CAST(column_name AS CHAR) FROM information_schema.statistics \
         WHERE table_schema IN ({IN}) AND non_unique = 1 AND column_name IS NOT NULL \
         ORDER BY table_schema, table_name, index_name, seq_in_index",
        &scope,
    )
    .await?
    {
        out.indexes.push(IsIndexColumn {
            schema: text(&row, 0)?,
            table: text(&row, 1)?,
            index: text(&row, 2)?,
            column: text(&row, 3)?,
        });
    }
    for row in rows(
        pool,
        "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR), CAST(view_definition AS CHAR) \
         FROM information_schema.views WHERE table_schema IN ({IN}) ORDER BY 1, 2",
        &scope,
    )
    .await?
    {
        out.views.push(IsView {
            schema: text(&row, 0)?,
            name: text(&row, 1)?,
            definition: text(&row, 2)?,
        });
    }
    if out.version.contains("MariaDB") {
        for row in rows(
            pool,
            "SELECT CAST(constraint_schema AS CHAR), CAST(table_name AS CHAR), \
             CAST(check_clause AS CHAR) FROM information_schema.check_constraints \
             WHERE constraint_schema IN ({IN}) ORDER BY 1, 2",
            &scope,
        )
        .await?
        {
            out.checks
                .push((text(&row, 0)?, text(&row, 1)?, text(&row, 2)?));
        }
    }
    Ok(out)
}

/// Run fixture statements on the server at `dsn` (test fixtures only).
#[cfg(feature = "test-support")]
pub async fn run_statements(dsn: &str, statements: &[&str]) -> Result<(), String> {
    use sqlx::Connection;
    let mut conn = sqlx::mysql::MySqlConnection::connect(dsn)
        .await
        .map_err(|e| format!("fixture connect failed: {e}"))?;
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe((*statement).to_string()))
            .execute(&mut conn)
            .await
            .map_err(|e| format!("fixture statement failed: {statement}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
    #[test]
    fn placeholders_match_the_scope_size() {
        assert_eq!(placeholders(1), "?");
        assert_eq!(placeholders(3), "?,?,?");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
    #[tokio::test]
    async fn an_unreachable_server_is_an_error_without_the_dsn() {
        let error = read_information_schema("mysql://user:secret@127.0.0.1:1/db", &[], 1)
            .await
            .unwrap_err();
        assert!(error.starts_with("federation: mysql connection"));
        assert!(!error.contains("secret"));
    }
}
