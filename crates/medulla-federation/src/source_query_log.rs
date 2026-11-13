//! Query logs of attached SQL sources (MEDULLA-UNIFIED-DATA-PLANE-R764.7).
//!
//! A source's own record of the statements it ran tells which tables people read, join and
//! filter on. [`SourceQueryLog`] reads that record: the Postgres adapter reads
//! `pg_stat_statements`, the MySQL and MariaDB adapter reads
//! `performance_schema.events_statements_history_long`, every other source answers
//! `QUERY_LOG_UNSUPPORTED`. [`tally_entries`] turns entries into counted reads, joins and
//! filters with the shared parser. Errors carry no DSN.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::time::Duration;

use medulla_exec::sql_text::SqlDialect;
use sqlparser::ast::{
    Expr, ObjectName, ObjectNamePart, Query, SetExpr, Statement, TableFactor, Visit, Visitor,
};

use crate::sql_analysis::{parse_one_statement, referenced_tables};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Refusal code: the source kind keeps no readable query log.
pub const QUERY_LOG_UNSUPPORTED: &str = "QUERY_LOG_UNSUPPORTED";
/// Refusal code: the source keeps a log but it cannot be read now.
pub const QUERY_LOG_UNAVAILABLE: &str = "QUERY_LOG_UNAVAILABLE";

/// One statement (or one counted group of identical statements) from a source's query log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryLogEntry {
    /// The database user that ran the statement.
    pub source_user: String,
    /// The statement text.
    pub statement: String,
    /// When it ran; for a cumulative log, when the log was read.
    pub executed_at_ms: u64,
    /// How many times it ran. For a cumulative entry this is the running total.
    pub calls: u64,
    /// Present on a cumulative entry: the stable identity its running total belongs to. The
    /// caller counts only the growth since the total it saw last.
    pub cumulative_key: Option<String>,
}

/// Why a query log could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryLogError {
    /// `QUERY_LOG_UNSUPPORTED`.
    Unsupported,
    /// `QUERY_LOG_UNAVAILABLE`.
    Unavailable,
}

impl QueryLogError {
    /// The refusal code.
    pub fn code(self) -> &'static str {
        match self {
            Self::Unsupported => QUERY_LOG_UNSUPPORTED,
            Self::Unavailable => QUERY_LOG_UNAVAILABLE,
        }
    }
}

impl std::fmt::Display for QueryLogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// A boxed future, so the trait stays object safe without a macro crate.
pub type LogFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<QueryLogEntry>, QueryLogError>> + Send + 'a>>;

/// The readable query log of one attached source.
pub trait SourceQueryLog: Send + Sync {
    /// Entries newer than `watermark_ms` (a cumulative log returns its whole current state),
    /// at most `limit` statements.
    fn read_since(&self, watermark_ms: u64, limit: usize) -> LogFuture<'_>;
}

/// A source kind with no readable log.
pub struct UnsupportedQueryLog;

impl SourceQueryLog for UnsupportedQueryLog {
    fn read_since(&self, _watermark_ms: u64, _limit: usize) -> LogFuture<'_> {
        Box::pin(async { Err(QueryLogError::Unsupported) })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// The Postgres adapter: reads `pg_stat_statements`, a cumulative log.
pub struct PostgresQueryLog {
    dsn: String,
}

impl SourceQueryLog for PostgresQueryLog {
    fn read_since(&self, _watermark_ms: u64, limit: usize) -> LogFuture<'_> {
        Box::pin(async move {
            use sqlx::postgres::PgPoolOptions;
            use sqlx::Row;
            let pool = tokio::time::timeout(
                CONNECT_TIMEOUT,
                PgPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(CONNECT_TIMEOUT)
                    .connect(&self.dsn),
            )
            .await
            .map_err(|_| QueryLogError::Unavailable)?
            .map_err(|_| QueryLogError::Unavailable)?;
            let read = sqlx::query(
                "SELECT r.rolname::text, s.query::text, s.calls::bigint, \
                 (s.userid::text || ':' || s.dbid::text || ':' || s.queryid::text) \
                 FROM pg_stat_statements s JOIN pg_roles r ON r.oid = s.userid \
                 WHERE s.query IS NOT NULL ORDER BY s.calls DESC LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&pool)
            .await;
            pool.close().await;
            let rows = read.map_err(|_| QueryLogError::Unavailable)?;
            let at = now_ms();
            rows.iter()
                .map(|row| {
                    Ok(QueryLogEntry {
                        source_user: row.try_get(0).map_err(|_| QueryLogError::Unavailable)?,
                        statement: row.try_get(1).map_err(|_| QueryLogError::Unavailable)?,
                        executed_at_ms: at,
                        calls: row
                            .try_get::<i64, _>(2)
                            .map_err(|_| QueryLogError::Unavailable)?
                            .max(0) as u64,
                        cumulative_key: Some(
                            row.try_get(3).map_err(|_| QueryLogError::Unavailable)?,
                        ),
                    })
                })
                .collect()
        })
    }
}

/// The MySQL and MariaDB adapter: reads `events_statements_history_long`, one row per run.
pub struct MysqlQueryLog {
    dsn: String,
}

impl SourceQueryLog for MysqlQueryLog {
    fn read_since(&self, watermark_ms: u64, limit: usize) -> LogFuture<'_> {
        Box::pin(async move {
            use sqlx::mysql::MySqlPoolOptions;
            use sqlx::Row;
            let pool = tokio::time::timeout(
                CONNECT_TIMEOUT,
                MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(CONNECT_TIMEOUT)
                    .connect(&self.dsn),
            )
            .await
            .map_err(|_| QueryLogError::Unavailable)?
            .map_err(|_| QueryLogError::Unavailable)?;
            // TIMER_START counts picoseconds since server start; the server's uptime anchors it
            // to the wall clock.
            let read = sqlx::query(
                "SELECT COALESCE(t.PROCESSLIST_USER, '') AS u, h.SQL_TEXT AS q, \
                 CAST(UNIX_TIMESTAMP(NOW(3)) * 1000 - ((SELECT CAST(VARIABLE_VALUE AS UNSIGNED) \
                 FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Uptime') * 1000 \
                 - h.TIMER_START / 1000000000) AS SIGNED) AS at \
                 FROM performance_schema.events_statements_history_long h \
                 LEFT JOIN performance_schema.threads t ON t.THREAD_ID = h.THREAD_ID \
                 WHERE h.SQL_TEXT IS NOT NULL ORDER BY h.TIMER_START DESC LIMIT ?",
            )
            .bind(limit as i64)
            .fetch_all(&pool)
            .await;
            pool.close().await;
            let rows = read.map_err(|_| QueryLogError::Unavailable)?;
            let mut out = Vec::new();
            for row in &rows {
                let at = row
                    .try_get::<i64, _>(2)
                    .map_err(|_| QueryLogError::Unavailable)?
                    .max(0) as u64;
                if at <= watermark_ms {
                    continue;
                }
                out.push(QueryLogEntry {
                    source_user: row.try_get(0).map_err(|_| QueryLogError::Unavailable)?,
                    statement: row.try_get(1).map_err(|_| QueryLogError::Unavailable)?,
                    executed_at_ms: at,
                    calls: 1,
                    cumulative_key: None,
                });
            }
            Ok(out)
        })
    }
}

/// The adapter for a source DSN: Postgres and MySQL-family DSNs have one, anything else is
/// unsupported.
pub fn query_log_for_dsn(dsn: &str) -> Box<dyn SourceQueryLog> {
    if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
        Box::new(PostgresQueryLog {
            dsn: dsn.to_string(),
        })
    } else if dsn.starts_with("mysql://") || dsn.starts_with("mariadb://") {
        Box::new(MysqlQueryLog {
            dsn: dsn.to_string(),
        })
    } else {
        Box::new(UnsupportedQueryLog)
    }
}

/// The SQL dialect a source DSN speaks.
pub fn dialect_for_dsn(dsn: &str) -> SqlDialect {
    if dsn.starts_with("mysql://") || dsn.starts_with("mariadb://") {
        SqlDialect::MySql
    } else {
        SqlDialect::Postgres
    }
}

/// Counted usage of one run of a source's log. Keys are `(source_user, ...)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageTally {
    /// `(user, table)` to reads.
    pub reads: BTreeMap<(String, String), u64>,
    /// `(user, table, joined table)` to joins.
    pub joins: BTreeMap<(String, String, String), u64>,
    /// `(user, table, column)` to filters.
    pub filters: BTreeMap<(String, String, String), u64>,
    /// Statements that did not parse.
    pub unparsed: u64,
}

fn dotted(name: &ObjectName) -> String {
    name.0
        .iter()
        .filter_map(|part| match part {
            ObjectNamePart::Identifier(ident) => Some(ident.value.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Joins and filters of the plain `SELECT` bodies of a statement.
#[derive(Default)]
struct ShapeCollector {
    ctes: BTreeSet<String>,
    /// `(table, joined table)` pairs, both directions.
    joins: Vec<(String, String)>,
    /// `(table, column)` pairs from `WHERE` clauses.
    filters: Vec<(String, String)>,
}

struct ColumnRefs(Vec<Vec<String>>);

impl Visitor for ColumnRefs {
    type Break = ();
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        match expr {
            Expr::Identifier(id) => self.0.push(vec![id.value.clone()]),
            Expr::CompoundIdentifier(parts) => {
                self.0.push(parts.iter().map(|p| p.value.clone()).collect())
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

impl Visitor for ShapeCollector {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.ctes.insert(cte.alias.name.value.to_lowercase());
            }
        }
        if let SetExpr::Select(select) = query.body.as_ref() {
            self.select(select);
        }
        ControlFlow::Continue(())
    }
}

impl ShapeCollector {
    fn select(&mut self, select: &sqlparser::ast::Select) {
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        let mut tables: Vec<String> = Vec::new();
        for from in &select.from {
            let mut group: Vec<String> = Vec::new();
            let factors =
                std::iter::once(&from.relation).chain(from.joins.iter().map(|j| &j.relation));
            for factor in factors {
                if let TableFactor::Table { name, alias, .. } = factor {
                    let table = dotted(name);
                    if self.ctes.contains(&table.to_lowercase()) {
                        continue;
                    }
                    if let Some(alias) = alias {
                        aliases.insert(alias.name.value.to_lowercase(), table.clone());
                    }
                    aliases.insert(table.to_lowercase(), table.clone());
                    if let Some((_, last)) = table.rsplit_once('.') {
                        aliases.insert(last.to_lowercase(), table.clone());
                    }
                    group.push(table.clone());
                    tables.push(table);
                }
            }
            if from.joins.is_empty() {
                continue;
            }
            for (i, a) in group.iter().enumerate() {
                for b in group.iter().skip(i + 1) {
                    if a != b {
                        self.joins.push((a.clone(), b.clone()));
                        self.joins.push((b.clone(), a.clone()));
                    }
                }
            }
        }
        let Some(selection) = &select.selection else {
            return;
        };
        let mut refs = ColumnRefs(Vec::new());
        let _ = selection.visit(&mut refs);
        for parts in refs.0 {
            match parts.as_slice() {
                [column] if tables.len() == 1 => {
                    self.filters.push((tables[0].clone(), column.clone()))
                }
                [qualifier, column] => {
                    if let Some(table) = aliases.get(&qualifier.to_lowercase()) {
                        self.filters.push((table.clone(), column.clone()));
                    }
                }
                _ => {}
            }
        }
    }
}

fn is_system_table(name: &str) -> bool {
    let lower = name.to_lowercase();
    let last = lower.rsplit('.').next().unwrap_or(&lower);
    lower.starts_with("pg_catalog.")
        || lower.starts_with("information_schema.")
        || lower.starts_with("performance_schema.")
        || lower.starts_with("mysql.")
        || lower.starts_with("sys.")
        || last.starts_with("pg_")
}

/// Count the reads, joins and filters of `statements`, each with the number of times it ran.
/// Only read statements (`SELECT`) count; system catalog tables are not assets; a statement
/// that does not parse adds one to `unparsed`.
pub fn tally_entries<'a, I>(entries: I, dialect: SqlDialect) -> UsageTally
where
    I: IntoIterator<Item = (&'a QueryLogEntry, u64)>,
{
    let mut tally = UsageTally::default();
    for (entry, times) in entries {
        if times == 0 {
            continue;
        }
        let statement = match parse_one_statement(&entry.statement, dialect) {
            Ok(statement) => statement,
            Err(_) => {
                tally.unparsed += times;
                continue;
            }
        };
        if !matches!(statement, Statement::Query(_)) {
            continue;
        }
        let user = entry.source_user.clone();
        let Ok(tables) = referenced_tables(&entry.statement, dialect) else {
            tally.unparsed += times;
            continue;
        };
        for table in tables.iter().filter(|t| !is_system_table(t)) {
            *tally
                .reads
                .entry((user.clone(), table.clone()))
                .or_default() += times;
        }
        let mut shape = ShapeCollector::default();
        let _ = statement.visit(&mut shape);
        for (table, other) in shape.joins {
            if is_system_table(&table) || is_system_table(&other) {
                continue;
            }
            *tally.joins.entry((user.clone(), table, other)).or_default() += times;
        }
        for (table, column) in shape.filters {
            if is_system_table(&table) {
                continue;
            }
            *tally
                .filters
                .entry((user.clone(), table, column))
                .or_default() += times;
        }
    }
    tally
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(user: &str, sql: &str) -> QueryLogEntry {
        QueryLogEntry {
            source_user: user.to_string(),
            statement: sql.to_string(),
            executed_at_ms: 1,
            calls: 1,
            cumulative_key: None,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.7
    #[test]
    fn tally_counts_reads_joins_filters_and_unparsed() {
        let join = entry(
            "alice",
            "SELECT o.id FROM public.orders o JOIN public.customers c ON c.id = o.cid \
             WHERE o.status = $1 AND c.region = $2",
        );
        let plain = entry("bob", "SELECT id FROM public.orders WHERE total > $1");
        let junk = entry("bob", "FROBNICATE nope");
        let ddl = entry("bob", "CREATE TABLE public.t (id int)");
        let catalog = entry("bob", "SELECT * FROM pg_stat_statements");
        let all = [
            (&join, 3),
            (&plain, 2),
            (&junk, 4),
            (&ddl, 5),
            (&catalog, 1),
        ];
        let tally = tally_entries(all, SqlDialect::Postgres);
        let key = |u: &str, t: &str| (u.to_string(), t.to_string());
        assert_eq!(tally.reads[&key("alice", "public.orders")], 3);
        assert_eq!(tally.reads[&key("alice", "public.customers")], 3);
        assert_eq!(tally.reads[&key("bob", "public.orders")], 2);
        assert!(!tally.reads.contains_key(&key("bob", "pg_stat_statements")));
        assert_eq!(
            tally.joins[&(
                "alice".into(),
                "public.orders".into(),
                "public.customers".into()
            )],
            3
        );
        assert_eq!(
            tally.joins[&(
                "alice".into(),
                "public.customers".into(),
                "public.orders".into()
            )],
            3
        );
        assert_eq!(
            tally.filters[&("alice".into(), "public.orders".into(), "status".into())],
            3
        );
        assert_eq!(
            tally.filters[&("alice".into(), "public.customers".into(), "region".into())],
            3
        );
        assert_eq!(
            tally.filters[&("bob".into(), "public.orders".into(), "total".into())],
            2
        );
        assert_eq!(tally.unparsed, 4);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.7
    #[tokio::test]
    async fn other_adapters_are_unsupported() {
        let log = query_log_for_dsn("sqlite:///tmp/x.db");
        let refused = log.read_since(0, 10).await.unwrap_err();
        assert_eq!(refused.code(), "QUERY_LOG_UNSUPPORTED");
        assert_eq!(QueryLogError::Unavailable.code(), "QUERY_LOG_UNAVAILABLE");
    }
}
