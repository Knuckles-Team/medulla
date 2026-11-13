//! `ModalityContract` for [`BlobReference`] (MEDULLA-FEDERATED-QUERY-R721.1): the blob modality.

use medulla_modality::{
    encode_staged, ConformanceTestable, ModalityContract, RowSetShape, StagedWrite,
};

use crate::change_envelope::{BlobReference, MaterialOperation};

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

impl ConformanceTestable for BlobReference {
    fn conformance_sample() -> Self {
        BlobReference {
            blob_id: "b1".to_string(),
            operation: MaterialOperation::Upsert,
            digest_algorithm: "sha256".to_string(),
            digest: "00".to_string(),
            media_type: "application/octet-stream".to_string(),
            length: 4,
        }
    }
}

medulla_modality::modality_conformance_tests!(BlobReference);
