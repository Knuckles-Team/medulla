//! SQL `CREATE [UNIQUE] INDEX` / `DROP INDEX` over ordinary scalar indexes
//! (MEDULLA-ENGINE-PARITY-R010.2). Both run inside the SQL write transaction, so
//! a table created earlier in the same batch can be indexed and a rollback leaves
//! no catalog row behind.

use medulla_tables::index::{
    SecondaryIndexColumn, SecondaryIndexOrder, SecondaryIndexSpec, SECONDARY_INDEX_SCHEMA_VERSION,
};

use super::super::index_plan::IndexPlan;
use super::{
    create_secondary_index_in, decode_stored, drop_ann_index_in, drop_secondary_index_in,
    get_schema_in, map_err, SqlWrite, TableStore, SECONDARY_INDEXES,
};

/// The table owning the secondary index called `name` in `tenant_scope`, if any.
fn secondary_index_table_write(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let indexes = wtx.open_table(SECONDARY_INDEXES)?;
    for row in indexes.iter().map_err(map_err)? {
        let (_, value) = row.map_err(map_err)?;
        let Ok(spec) = decode_stored::<SecondaryIndexSpec>(value.value(), "secondary index") else {
            continue;
        };
        if spec.tenant_scope == tenant_scope && spec.name == name {
            return Ok(Some(spec.table));
        }
    }
    Ok(None)
}

/// Build the schema-bound definition a [`IndexPlan`] describes.
fn spec_from_plan(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    plan: &IndexPlan,
) -> Result<SecondaryIndexSpec, String> {
    let schema = get_schema_in(wtx, &plan.table)?
        .ok_or_else(|| format!("table `{}` does not exist", plan.table))?;
    let columns = plan
        .columns
        .iter()
        .map(|column| SecondaryIndexColumn {
            name: column.column.clone(),
            order: if column.descending {
                SecondaryIndexOrder::Desc
            } else {
                SecondaryIndexOrder::Asc
            },
            expr: column.expr,
        })
        .collect();
    let mut spec = SecondaryIndexSpec::btree(
        tenant_scope,
        &plan.table,
        plan.resolved_name(),
        columns,
        &schema,
    )?;
    debug_assert_eq!(spec.schema_version, SECONDARY_INDEX_SCHEMA_VERSION);
    if let Some(predicate) = &plan.predicate {
        spec = spec.with_predicate(predicate.clone(), &schema)?;
    }
    if plan.unique {
        spec = spec.into_unique();
    }
    Ok(spec)
}

/// `CREATE [UNIQUE] INDEX` inside the write transaction. An index name is unique
/// across the owner's tables, as in Postgres.
pub(super) fn put_secondary_index_in(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    plan: &IndexPlan,
) -> Result<usize, String> {
    let spec = spec_from_plan(wtx, tenant_scope, plan)?;
    if let Some(table) = secondary_index_table_write(wtx, tenant_scope, &spec.name)? {
        if table != spec.table {
            if plan.if_not_exists {
                return Ok(0);
            }
            return Err(format!("index `{}` already exists", spec.name));
        }
    }
    create_secondary_index_in(wtx, tenant_scope, &spec, plan.if_not_exists)?;
    Ok(0)
}

/// `DROP INDEX name` inside the write transaction: the secondary index when one
/// of that name exists on `table`, otherwise the ANN registration.
pub(super) fn drop_index_in(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    table: &str,
    name: &str,
) -> Result<usize, String> {
    if secondary_index_table_write(wtx, tenant_scope, name)?.as_deref() == Some(table) {
        drop_secondary_index_in(wtx, tenant_scope, table, name, false)?;
        return Ok(1);
    }
    drop_ann_index_in(wtx, table, name)
}

impl TableStore {
    /// The table that owns the ordinary index `name`, if the owner has one. The
    /// caller still authorizes the table before disclosing it.
    pub fn secondary_index_table(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .list_secondary_indexes(None)?
            .into_iter()
            .find(|spec| spec.name == name)
            .map(|spec| spec.table))
    }
}
