//! Profile reads against an attached Postgres or MySQL-family source
//! (MEDULLA-UNIFIED-DATA-PLANE-R761.2, R761.3).
//!
//! The `DatasetProfile` job pushes one aggregate statement per column and reads a bounded
//! sample of the column's values. Both statements cast every value to text on the
//! source, so a profile records the source's own rendering of each value. A connection
//! that does not open within `timeout` is a refusal, never a hang.

use std::time::Duration;

/// The four values of one pushed aggregate: `COUNT(*)`, `COUNT(column)`, and the
/// text-cast `MIN` and `MAX`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileAggregate {
    /// Rows of the dataset.
    pub rows: u64,
    /// Non-null values of the column.
    pub non_null: u64,
    /// Smallest value, cast to text by the source.
    pub min: Option<String>,
    /// Largest value, cast to text by the source.
    pub max: Option<String>,
}

fn is_postgres(dsn: &str) -> bool {
    dsn.starts_with("postgres://") || dsn.starts_with("postgresql://")
}

fn is_mysql(dsn: &str) -> bool {
    dsn.starts_with("mysql://") || dsn.starts_with("mariadb://")
}

fn unavailable<E>(_: E) -> String {
    "PROFILE_UNSUPPORTED: the source cannot be read".to_string()
}

fn count(value: i64) -> u64 {
    value.max(0) as u64
}

/// Run one aggregate statement whose result row is `(COUNT(*), COUNT(col),
/// text MIN, text MAX)`.
pub async fn read_profile_aggregate(
    dsn: &str,
    statement: &str,
    timeout: Duration,
) -> Result<ProfileAggregate, String> {
    use sqlx::Row;
    if is_postgres(dsn) {
        let pool = tokio::time::timeout(
            timeout,
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(timeout)
                .connect(dsn),
        )
        .await
        .map_err(unavailable)?
        .map_err(unavailable)?;
        let read = sqlx::query(statement).fetch_one(&pool).await;
        pool.close().await;
        let row = read.map_err(unavailable)?;
        return Ok(ProfileAggregate {
            rows: count(row.try_get::<i64, _>(0).map_err(unavailable)?),
            non_null: count(row.try_get::<i64, _>(1).map_err(unavailable)?),
            min: row.try_get(2).map_err(unavailable)?,
            max: row.try_get(3).map_err(unavailable)?,
        });
    }
    if is_mysql(dsn) {
        let pool = tokio::time::timeout(
            timeout,
            sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(timeout)
                .connect(dsn),
        )
        .await
        .map_err(unavailable)?
        .map_err(unavailable)?;
        let read = sqlx::query(statement).fetch_one(&pool).await;
        pool.close().await;
        let row = read.map_err(unavailable)?;
        return Ok(ProfileAggregate {
            rows: count(row.try_get::<i64, _>(0).map_err(unavailable)?),
            non_null: count(row.try_get::<i64, _>(1).map_err(unavailable)?),
            min: row.try_get(2).map_err(unavailable)?,
            max: row.try_get(3).map_err(unavailable)?,
        });
    }
    Err("PROFILE_UNSUPPORTED: the source has no aggregate pushdown".to_string())
}

/// Run one sample statement whose rows hold one text-cast value each (`None` = SQL NULL).
pub async fn read_column_sample(
    dsn: &str,
    statement: &str,
    timeout: Duration,
) -> Result<Vec<Option<String>>, String> {
    use sqlx::Row;
    if is_postgres(dsn) {
        let pool = tokio::time::timeout(
            timeout,
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(timeout)
                .connect(dsn),
        )
        .await
        .map_err(unavailable)?
        .map_err(unavailable)?;
        let read = sqlx::query(statement).fetch_all(&pool).await;
        pool.close().await;
        return read
            .map_err(unavailable)?
            .iter()
            .map(|row| row.try_get::<Option<String>, _>(0).map_err(unavailable))
            .collect();
    }
    if is_mysql(dsn) {
        let pool = tokio::time::timeout(
            timeout,
            sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(timeout)
                .connect(dsn),
        )
        .await
        .map_err(unavailable)?
        .map_err(unavailable)?;
        let read = sqlx::query(statement).fetch_all(&pool).await;
        pool.close().await;
        return read
            .map_err(unavailable)?
            .iter()
            .map(|row| row.try_get::<Option<String>, _>(0).map_err(unavailable))
            .collect();
    }
    Err("PROFILE_UNSUPPORTED: the source cannot be sampled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R761.3
    #[tokio::test]
    async fn a_source_without_pushdown_refuses_unsupported() {
        let timeout = Duration::from_millis(50);
        let refused = read_profile_aggregate("http://x.example.test/r", "SELECT 1", timeout)
            .await
            .unwrap_err();
        assert!(refused.starts_with("PROFILE_UNSUPPORTED"));
        let refused = read_column_sample("http://x.example.test/r", "SELECT 1", timeout)
            .await
            .unwrap_err();
        assert!(refused.starts_with("PROFILE_UNSUPPORTED"));
    }
}
