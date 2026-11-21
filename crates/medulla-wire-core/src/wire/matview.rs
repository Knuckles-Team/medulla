//! Materialized views over the wire (MEDULLA-ENGINE-PARITY-R014.1).
//!
//! `CREATE MATERIALIZED VIEW name AS select`, `REFRESH MATERIALIZED VIEW name`
//! and `DROP MATERIALIZED VIEW [IF EXISTS] name` build one [`TableTxn`] each
//! with the builders of the table store and commit it through
//! `commit_table_txn`. The select runs through the caller's authorized read path
//! ([`WireSession::run_read`]), so the stored rows are exactly what the caller
//! may read. A read of the view is a plain read of its result table.

use medulla_federation::sql_analysis::referenced_tables;
use medulla_sql_lang::sql::classify::CreateMatViewPlan;
use medulla_sql_lang::sql::typed_result::{matview_result_schema, PgColType};
use medulla_table_store::tables::{
    create_matview_txn, drop_matview_txn, refresh_matview_txn, MatViewSpec, TableStore,
    MATVIEW_EXISTS, MATVIEW_UNKNOWN,
};
use medulla_tables::schema::ColumnType;

use super::{user_err, WireOutcome, WireProtocol, WireResult, WireSession};

/// The stored column type of a result column (R014.1): `Text` → `Text`,
/// `Int8` → `BigInt`, `Float8` → `Double`, `Bool` → `Bool`, `Vector` →
/// `Vector(None)`.
pub(crate) fn matview_column_type(ty: PgColType) -> ColumnType {
    ty.stored_column_type()
}

/// The user tables `select_sql` reads, by leaf name. Every one must be a user
/// table of the tenant store: a graph input (`nodes`, `edges`) or an unknown
/// relation is refused `FEATURE_NOT_SUPPORTED`.
fn matview_inputs(store: &TableStore, select_sql: &str) -> WireResult<Vec<String>> {
    let referenced = referenced_tables(select_sql, medulla_exec::sql_text::SqlDialect::Postgres)
        .map_err(|e| user_err(format!("FEATURE_NOT_SUPPORTED: materialized view select: {e}")))?;
    let mut inputs = Vec::new();
    for qualified in referenced {
        let leaf = qualified
            .rsplit('.')
            .next()
            .unwrap_or(qualified.as_str())
            .trim_matches('"')
            .to_string();
        let is_user_table = store.get_schema(&leaf).map_err(user_err)?.is_some();
        if !is_user_table {
            return Err(user_err(format!(
                "FEATURE_NOT_SUPPORTED: materialized view input `{leaf}` is not a user table"
            )));
        }
        if !inputs.contains(&leaf) {
            inputs.push(leaf);
        }
    }
    Ok(inputs)
}

impl WireSession {
    /// The materialized views `sql` reads that are stale (MEDULLA-ENGINE-PARITY-R014.2),
    /// sorted by name. A statement that does not parse as a read, or a store that cannot
    /// be opened, reports none: the read itself already succeeded or refused on its own.
    pub(super) async fn stale_matviews_read(&self, sql: &str) -> Vec<String> {
        let Ok(referenced) =
            referenced_tables(sql, medulla_exec::sql_text::SqlDialect::Postgres)
        else {
            return Vec::new();
        };
        let Ok(store) = self.user_table_store().await else {
            return Vec::new();
        };
        let mut stale: Vec<String> = Vec::new();
        for qualified in referenced {
            let leaf = qualified
                .rsplit('.')
                .next()
                .unwrap_or(qualified.as_str())
                .trim_matches('"')
                .to_string();
            if stale.contains(&leaf) {
                continue;
            }
            if matches!(store.get_matview(&leaf), Ok(Some(spec)) if spec.stale) {
                stale.push(leaf);
            }
        }
        stale.sort();
        stale
    }

    /// Refuse materialized-view DDL inside an explicit transaction block: the
    /// result table is built from one read and committed in one transaction.
    fn refuse_matview_in_txn(&self) -> WireResult<()> {
        if self.in_txn() {
            return Err(user_err(
                "FEATURE_NOT_SUPPORTED: materialized view DDL inside a transaction block",
            ));
        }
        Ok(())
    }

    /// `CREATE MATERIALIZED VIEW`.
    pub(super) async fn run_create_matview(
        &self,
        graph: &str,
        sql: &str,
        plan: CreateMatViewPlan,
    ) -> WireResult<WireOutcome> {
        self.refuse_matview_in_txn()?;
        let store = self.user_table_store().await?;
        let name = plan.name;
        let exists = store.get_matview(&name).map_err(user_err)?.is_some()
            || store.get_view(&name).map_err(user_err)?.is_some()
            || store.get_schema(&name).map_err(user_err)?.is_some();
        if exists {
            return Err(user_err(format!("{MATVIEW_EXISTS}: {name}")));
        }
        let inputs = matview_inputs(&store, &plan.select_sql)?;
        // MEDULLA-INCREMENTAL-VIEWS-R009.2: a defining query outside the incremental forms stays a
        // recompute view of R014.2 and publishes the first construct that keeps it there.
        let column_type = |table: &str, column: &str| {
            store
                .get_schema(table)
                .ok()
                .flatten()
                .and_then(|schema| schema.column(column).map(|found| found.ty.clone()))
        };
        if let Err(refusal) = medulla_sql_lang::sql::matview_compile::compile_incremental_sql(
            &name,
            &plan.select_sql,
            &column_type,
        ) {
            medulla_core::view_status::publish(medulla_core::view_status::ViewStatus {
                name: name.clone(),
                family: medulla_core::view_status::ViewFamily::Sql,
                graph: graph.to_string(),
                mode: medulla_core::view_status::ViewMode::Recompute,
                reason_code: Some(refusal.code().as_str().to_string()),
                reason: Some(refusal.construct),
                state_bytes: 0,
                applied_changes: 0,
                shadow_checks: 0,
                shadow_mismatches: 0,
            });
        }
        let seq = store.commit_seq();
        let result = self.run_read(graph, plan.select_sql.clone()).await?;
        let (authority, _) = self.catalog_authority().await?;
        let spec = MatViewSpec {
            name: name.clone(),
            select_sql: plan.select_sql,
            inputs,
            owner: authority.actor_scope().to_string(),
            refreshed_seq: seq,
            stale: false,
        };
        let schema = matview_result_schema(&name, &result.columns);
        let col_order = result.columns.iter().map(|c| c.name.clone()).collect();
        let txn = create_matview_txn(spec, schema, col_order, result.rows);
        self.commit_table_txn(graph, sql, txn).await?;
        Ok(WireOutcome::command("CREATE MATERIALIZED VIEW"))
    }

    /// `REFRESH MATERIALIZED VIEW`.
    pub(super) async fn run_refresh_matview(
        &self,
        graph: &str,
        sql: &str,
        name: String,
    ) -> WireResult<WireOutcome> {
        self.refuse_matview_in_txn()?;
        let store = self.user_table_store().await?;
        let Some(mut spec) = store.get_matview(&name).map_err(user_err)? else {
            return Err(user_err(format!("{MATVIEW_UNKNOWN}: {name}")));
        };
        let seq = store.commit_seq();
        let result = self.run_read(graph, spec.select_sql.clone()).await?;
        spec.refreshed_seq = seq;
        spec.stale = false;
        let schema = matview_result_schema(&name, &result.columns);
        let col_order = result.columns.iter().map(|c| c.name.clone()).collect();
        let txn = refresh_matview_txn(spec, schema, col_order, result.rows);
        self.commit_table_txn(graph, sql, txn).await?;
        Ok(WireOutcome::command("REFRESH MATERIALIZED VIEW"))
    }

    /// `DROP MATERIALIZED VIEW [IF EXISTS]`.
    pub(super) async fn run_drop_matview(
        &self,
        graph: &str,
        sql: &str,
        name: String,
        if_exists: bool,
    ) -> WireResult<WireOutcome> {
        self.refuse_matview_in_txn()?;
        let store = self.user_table_store().await?;
        if store.get_matview(&name).map_err(user_err)?.is_none() {
            if if_exists {
                return Ok(WireOutcome::command("DROP MATERIALIZED VIEW"));
            }
            return Err(user_err(format!("{MATVIEW_UNKNOWN}: {name}")));
        }
        self.commit_table_txn(graph, sql, drop_matview_txn(&name))
            .await?;
        medulla_core::view_status::remove(&name);
        Ok(WireOutcome::command("DROP MATERIALIZED VIEW"))
    }
}
