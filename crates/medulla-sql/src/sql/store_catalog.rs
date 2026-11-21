//! The catalog objects of one table store that a SQL read registers beside the
//! graph relations: durable views, stored functions, SQL/PGQ property graphs,
//! pgvector ANN registrations and the managed-index status rows (EH-352). Read
//! once per statement and handed to the context builder as one value.

use medulla_graph_core::index::ManagedIndexStatus;

use medulla_table_store::tables::ann_plan::AnnIndexPlan;
use medulla_table_store::tables::embedding_binding::ColumnVectorAdvice;
use medulla_table_store::tables::PropertyGraphCatalogRecord;
use medulla_table_store::tables::TableStore;
use medulla_tables::index::SecondaryIndexSpec;
use medulla_tables::schema::{StoredFunction, TableSchema};
use medulla_tables::sequence::SequenceSource;
use std::sync::Arc;

/// The name of the managed-index status relation, `information_schema.medulla_index_status`.
pub(crate) const INDEX_STATUS_RELATION: &str = "medulla_index_status";

/// One store's catalog objects, as one statement sees them.
#[derive(Default)]
pub(crate) struct StoreCatalog {
    pub(crate) views: Vec<(String, String)>,
    pub(crate) functions: Vec<StoredFunction>,
    pub(crate) property_graphs: Vec<PropertyGraphCatalogRecord>,
    pub(crate) ann_indexes: Vec<AnnIndexPlan>,
    pub(crate) index_status: Vec<ManagedIndexStatus>,
    /// The graph the statement runs over; scopes `medulla_catalog.matviews`.
    pub(crate) statement_graph: Option<String>,
    /// The schema of every user table, for the constraint catalogs.
    pub(crate) tables: Vec<TableSchema>,
    /// Every ordinary secondary index, for `pg_index`.
    pub(crate) secondary_indexes: Vec<SecondaryIndexSpec>,
    /// The durable source `nextval`, `currval` and `setval` draw from; none when the
    /// statement runs without a table store.
    pub(crate) sequences: Option<Arc<dyn SequenceSource>>,
    /// The store the statement reads: the one whose usage counters record its
    /// text-search uses (MEDULLA-FEDERATED-QUERY-R077.6.3).
    pub(crate) source_store: Option<TableStore>,
    /// The rows of `information_schema.vector_advice`, read once for a
    /// statement that names the relation.
    pub(crate) vector_advice: Option<Vec<ColumnVectorAdvice>>,
    /// The sessions `pg_stat_activity` and `pg_stat_database` answer from
    /// (MEDULLA-DURABLE-KERNEL-R035.3.1); the caller supplies them filtered.
    pub(crate) sessions: Vec<super::catalog::PgStatActivityRow>,
}

impl StoreCatalog {
    /// Every catalog object of `store`; property graphs under `graph_scope`.
    pub(crate) fn read(store: &TableStore, graph_scope: &str) -> Result<Self, String> {
        let mut tables = Vec::new();
        for name in store.list_tables()? {
            if let Some(schema) = store.get_schema(&name)? {
                tables.push(schema);
            }
        }
        Ok(Self {
            views: store.list_views()?,
            functions: store.list_functions()?,
            property_graphs: store.list_property_graph_records(graph_scope)?,
            ann_indexes: store.list_ann_indexes()?,
            index_status: store.managed_index_status()?,
            statement_graph: None,
            tables,
            secondary_indexes: store.list_secondary_indexes(None)?,
            sequences: Some(Arc::new(store.clone())),
            source_store: Some(store.clone()),
            vector_advice: None,
            sessions: Vec::new(),
        })
    }
}

/// The schema of the view status relation, `medulla_catalog.matviews`.
pub(crate) const VIEW_STATUS_SCHEMA: &str = "medulla_catalog";

/// Whether `sql` may read the view status relation. View status moves without a
/// catalog write, so such a statement never uses a cached context. Over-approximates
/// on purpose: a false positive only skips the cache.
pub(crate) fn reads_view_status(sql: &str) -> bool {
    sql.to_ascii_lowercase().contains(VIEW_STATUS_SCHEMA)
}

/// Whether `sql` may read the managed-index status relation. Index state moves
/// without a catalog write, so such a statement never uses a cached context.
/// Over-approximates on purpose: a false positive only skips the cache.
pub(crate) fn reads_index_status(sql: &str) -> bool {
    sql.to_ascii_lowercase().contains(INDEX_STATUS_RELATION)
}
