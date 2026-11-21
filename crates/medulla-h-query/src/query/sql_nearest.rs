//! The `ORDER BY NEAREST(column, 'text')` leg of the served SQL read
//! (MEDULLA-FEDERATED-QUERY-R077.5).
//!
//! The statement is recognized once, the column's semantic binding is resolved under the
//! caller's own authority, and the binding's live generation answers the query text. The probe
//! it returns is handed to the authorized read, which keeps only the rows the caller may see.

#![cfg(feature = "query")]

use medulla_server_api::carrier::CarrierAuthority;
use medulla_table_store::tables::nearest::NearestProbe;

/// Resolve the probe of a `NEAREST` statement, or `None` for any other statement.
///
/// Refusals: `ACCESS_DENIED` for a table the caller may not `SELECT`; `COLUMN_NOT_BOUND` for a
/// column with no binding, a binding whose selector is not this column, or no live
/// generation; `QUERY_ENCODER_UNAVAILABLE` and `SEARCH_PROBE_MODEL_MISMATCH` from the live
/// search; `INVALID_ARGUMENT` for a malformed form.
#[cfg(feature = "ann-redb")]
pub(crate) fn plan_nearest(
    authority: &CarrierAuthority,
    persist_dir: &std::path::Path,
    sql: &str,
) -> Result<Option<NearestProbe>, String> {
    use medulla_index_types::semantic_index::{
        SemanticSourceSelector, SqlColumnRef, SEMANTIC_SQL_CATALOG_ID, SEMANTIC_SQL_SCHEMA_ID,
    };
    use medulla_server_auth::sql_catalog_acl::{open_authorized_table, SqlPrivilege};
    use medulla_types::contract::ServerErrorCode;

    let Some(form) = medulla_sql::sql::nearest::nearest_form(sql)? else {
        return Ok(None);
    };
    let table = open_authorized_table(authority, persist_dir, &form.table, SqlPrivilege::Select)?;
    let schema = table.schema()?;
    let column = schema.column(&form.column).ok_or_else(|| {
        format!(
            "INVALID_ARGUMENT: NEAREST column `{}` is not in table `{}`",
            form.column, schema.name
        )
    })?;
    let selector = SqlColumnRef {
        catalog_id: SEMANTIC_SQL_CATALOG_ID.to_string(),
        schema_id: SEMANTIC_SQL_SCHEMA_ID.to_string(),
        table_id: schema.name.clone(),
        column_id: column.name.clone(),
    };
    let binding_id = selector.binding_id();
    let unbound = |why: &str| format!("{}: {why}", ServerErrorCode::ColumnNotBound);
    let service = medulla_h_retrieve::semantic_index_runtime::existing_semantic_service(
        persist_dir,
        authority.tenant_scope(),
        &binding_id,
    )
    .map_err(|_| unbound("the column has no semantic binding"))?;
    let binding = service
        .binding()
        .map_err(|_| unbound("the column's semantic binding is unreadable"))?
        .ok_or_else(|| unbound("the column has no semantic binding"))?;
    if binding.source_selector != SemanticSourceSelector::SqlColumnRef(selector.clone()) {
        return Err(unbound("the binding does not vectorize this column"));
    }
    let live = medulla_h_retrieve::semantic_index::search::search_live(
        &service,
        &binding,
        &form.text,
        form.fetch,
    )?;
    Ok(Some(NearestProbe {
        table: selector.table_id,
        column: selector.column_id,
        binding_id,
        generation: live.generation,
        space: binding.model_digest.clone(),
        hits: live.hits,
    }))
}

/// Without the durable semantic owner no column has a binding to probe.
#[cfg(not(feature = "ann-redb"))]
pub(crate) fn plan_nearest(
    _authority: &CarrierAuthority,
    _persist_dir: &std::path::Path,
    sql: &str,
) -> Result<Option<NearestProbe>, String> {
    match medulla_sql::sql::nearest::nearest_form(sql)? {
        Some(_) => Err("METHOD_NOT_YET_SERVED: this build has no semantic index owner".to_string()),
        None => Ok(None),
    }
}
