//! `WireApi` implementation: SQL wire behaviour that crates below the wire crate reach
//! through the seam registry (MEDULLA-CONTRACT-R056.8).

use medulla_query::{TableSchema, TableStore, TableTxn};

/// Wire behaviour installed on the seam registry's `wire` slot.
pub struct WireSeam;

impl medulla_server_api::api::query_txn::WireApi for WireSeam {
    fn authorize_table_txn(
        &self,
        source: &medulla_server_api::sql_catalog_acl::SqlSourceAuthorityWrite<'_, '_>,
        table_store: &TableStore,
        txn: &mut TableTxn,
        committed_replay: bool,
    ) -> Result<Vec<TableSchema>, String> {
        super::authorize_table_txn(source, table_store, txn, committed_replay)
            .map_err(|error| error.message)
    }

    fn committed_sql_replay_receipt(
        &self,
        store: &TableStore,
        authority: &medulla_server_api::access::CarrierAuthority,
        batch: &medulla_protocol_types::mutation_batch::MutationBatch,
    ) -> Result<Option<medulla_protocol_types::mutation_batch::MutationBatchRecord>, String> {
        super::committed_sql_replay_receipt(store, authority, batch)
    }
}
