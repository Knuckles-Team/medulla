//! `ModalityContract` for [`TableSnapshotRow`] (MEDULLA-FEDERATED-QUERY-R721.1): the SQL modality.

use medulla_modality::{encode_staged, ModalityContract, RowSetShape, StagedWrite};

use crate::tables::store::TableSnapshotRow;

impl ModalityContract for TableSnapshotRow {
    fn storage_kind(&self) -> &'static str {
        "sql"
    }

    /// A table row has no query-relative rank of its own; the caller supplies the id
    /// derived from the row's declared primary key.
    fn to_rowset(&self, id: &str) -> RowSetShape {
        RowSetShape::unranked(id)
    }

    fn txn_stage(&self, id: &str) -> StagedWrite {
        StagedWrite::put(id, encode_staged(self))
    }

    fn cdc_topic(&self) -> Option<&'static str> {
        None
    }
}
