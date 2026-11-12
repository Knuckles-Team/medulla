//! `ModalityContract` for [`BlobReference`] (MEDULLA-FEDERATED-QUERY-R721.1): the blob modality.

use medulla_modality::{encode_staged, ModalityContract, RowSetShape, StagedWrite};

use crate::change_envelope::BlobReference;

impl ModalityContract for BlobReference {
    fn storage_kind(&self) -> &'static str {
        "blob"
    }

    /// A blob reference has no query-relative rank of its own.
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
