//! `ModalityContract` for [`Binding`] (MEDULLA-FEDERATED-QUERY-R721.1): the RDF modality.
//!
//! A SPARQL binding is the value an RDF query contributes to a cross-modal result; a
//! node binding names the row it contributes, so the row id is the caller-supplied id.

use medulla_modality::{encode_staged, ModalityContract, RowSetShape, StagedWrite};

use crate::sparql::Binding;

impl ModalityContract for Binding {
    fn storage_kind(&self) -> &'static str {
        "rdf"
    }

    /// A binding has no query-relative rank of its own.
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
