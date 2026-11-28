use super::*;

use medulla_sql_lang::sql::classify::StatementKind;
use medulla_table_store::tables::{
    create_matview_txn, drop_matview_txn, refresh_matview_txn, MatViewSpec, TableStore, TableTxn,
    MATVIEW_EXISTS, MATVIEW_UNKNOWN,
};

/// The user tables `select_sql` reads, by leaf name. A graph input or an unknown
/// relation is refused `FEATURE_NOT_SUPPORTED` (MEDULLA-ENGINE-PARITY-R014.1).
#[cfg(feature = "query")]
fn matview_inputs(store: &TableStore, select_sql: &str) -> Result<Vec<String>, String> {
    let referenced = medulla_federation::sql_analysis::referenced_tables(
        select_sql,
        medulla_exec::sql_text::SqlDialect::Postgres,
    )
    .map_err(|e| format!("FEATURE_NOT_SUPPORTED: materialized view select: {e}"))?;
    let mut inputs = Vec::new();
    for qualified in referenced {
        let leaf = qualified
            .rsplit('.')
            .next()
            .unwrap_or(qualified.as_str())
            .trim_matches('"')
            .to_string();
        if store.get_schema(&leaf)?.is_none() {
            return Err(format!(
                "FEATURE_NOT_SUPPORTED: materialized view input `{leaf}` is not a user table"
            ));
        }
        if !inputs.contains(&leaf) {
            inputs.push(leaf);
        }
    }
    Ok(inputs)
}

/// The transaction of one materialized-view statement, or `None` when the
/// statement is a no-op (`DROP … IF EXISTS` of an absent view). The select runs
/// against the caller's authorized read store.
#[cfg(feature = "query")]
fn matview_txn(
    kind: StatementKind,
    owner: &str,
    snap: &medulla_graph_core::graph::GraphView,
    store: &TableStore,
    read_store: &TableStore,
) -> Result<Option<(TableTxn, &'static str)>, String> {
    match kind {
        StatementKind::CreateMatView(plan) => {
            let name = plan.name;
            let exists = store.get_matview(&name)?.is_some()
                || store.get_view(&name)?.is_some()
                || store.get_schema(&name)?.is_some();
            if exists {
                return Err(format!("{MATVIEW_EXISTS}: {name}"));
            }
            let inputs = matview_inputs(store, &plan.select_sql)?;
            let seq = store.commit_seq();
            let read =
                medulla_sql::sql::exec_sql_typed_with_tables(snap, read_store, &plan.select_sql)?;
            let spec = MatViewSpec {
                name: name.clone(),
                select_sql: plan.select_sql,
                inputs,
                owner: owner.to_string(),
                refreshed_seq: seq,
                stale: false,
                program: None,
            };
            let schema =
                medulla_sql_lang::sql::typed_result::matview_result_schema(&name, &read.columns);
            let col_order = read.columns.iter().map(|c| c.name.clone()).collect();
            Ok(Some((
                create_matview_txn(spec, schema, col_order, read.rows),
                "CREATE MATERIALIZED VIEW",
            )))
        }
        StatementKind::RefreshMatView { name } => {
            let Some(mut spec) = store.get_matview(&name)? else {
                return Err(format!("{MATVIEW_UNKNOWN}: {name}"));
            };
            let seq = store.commit_seq();
            let read =
                medulla_sql::sql::exec_sql_typed_with_tables(snap, read_store, &spec.select_sql)?;
            spec.refreshed_seq = seq;
            spec.stale = false;
            let schema =
                medulla_sql_lang::sql::typed_result::matview_result_schema(&name, &read.columns);
            let col_order = read.columns.iter().map(|c| c.name.clone()).collect();
            Ok(Some((
                refresh_matview_txn(spec, schema, col_order, read.rows),
                "REFRESH MATERIALIZED VIEW",
            )))
        }
        StatementKind::DropMatView { name, if_exists } => {
            if store.get_matview(&name)?.is_none() {
                if if_exists {
                    return Ok(None);
                }
                return Err(format!("{MATVIEW_UNKNOWN}: {name}"));
            }
            Ok(Some((drop_matview_txn(&name), "DROP MATERIALIZED VIEW")))
        }
        _ => Err("INVALID_ARGUMENT: SQL error: not a materialized view statement".to_string()),
    }
}

/// Execute `Method::Sql`'s `CREATE`/`REFRESH`/`DROP MATERIALIZED VIEW`
/// (MEDULLA-ENGINE-PARITY-R014.1): the same builders and the same single commit
/// the pgwire session uses.
#[cfg(feature = "query")]
pub async fn exec_sql_matview(
    req_id: u64,
    scope: SqlWriteScope<'_>,
    sql_method: Method,
    read_core: Arc<GraphCore>,
    store: &TableStore,
    read_store: &TableStore,
    kind: StatementKind,
) -> Response {
    let owner = scope.authority.actor_scope().to_string();
    let snap = read_core.analysis_snapshot();
    let work_store = store.clone();
    let work_read_store = read_store.clone();
    let built = compute_off_lock(req_id, move || {
        matview_txn(kind, &owner, &snap, &work_store, &work_read_store)
    })
    .await;
    match built {
        Ok(Ok(Some((txn, tag)))) => {
            commit_sql_catalog_txn(req_id, scope, sql_method, store, txn, tag).await
        }
        Ok(Ok(None)) => sql_write_ack(req_id, "DROP MATERIALIZED VIEW", Ok(Ok(0))),
        Ok(Err(error)) => Response::err(req_id, error),
        Err(response) => response,
    }
}
