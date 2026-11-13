//! MEDULLA-FEDERATED-QUERY-R721.1: every `PlanModality` has exactly one
//! `ModalityContract` implementation, in the crate that owns the modality's value type,
//! and each implementation contributes the row the caller names to the `RowSet` currency.

use medulla_modality::ModalityContract;
use medulla_plan::modality::PlanModality;

/// The one sample value and its contract per modality. The match is exhaustive, so a new
/// `PlanModality` variant fails to compile until it has an implementation here.
fn sample(modality: PlanModality) -> Box<dyn ModalityContract> {
    match modality {
        PlanModality::Graph => Box::new(medulla_graph_core::index::NodeChange::with_properties(
            "node-1".to_string(),
            vec![1, 2, 3],
        )),
        PlanModality::Vector => {
            let mut idx = medulla_ann::flat::FlatIndex::new(2);
            idx.add(&[(10, vec![0.0, 0.0]), (20, vec![1.0, 0.0])]);
            Box::new(idx)
        }
        PlanModality::Sql => Box::new(medulla_table_store::tables::store::TableSnapshotRow {
            row_id: 1,
            cells: Vec::new(),
        }),
        PlanModality::Rdf => Box::new(medulla_rdf::sparql::Binding::Node("<urn:a>".to_string())),
        PlanModality::TimeSeries => Box::new(medulla_tsdb::store::SeriesMeta {
            n_fields: 1,
            bucket_ns: 3_600_000_000_000,
            field_names: vec!["price".to_string()],
            count: 1,
            min_ts: 1,
            max_ts: 2,
            legal_hold: false,
            derived: None,
            dependents: Vec::new(),
        }),
        PlanModality::KeyValue => Box::new(medulla_kvcache::StoredBlock::encode(b"kv block bytes")),
        PlanModality::Blob => Box::new(medulla_protocol_types::change_envelope::BlobReference {
            blob_id: "b1".to_string(),
            operation: medulla_protocol_types::change_envelope::MaterialOperation::Upsert,
            digest_algorithm: "sha256".to_string(),
            digest: "00".to_string(),
            media_type: "application/octet-stream".to_string(),
            length: 4,
        }),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R721.1
#[test]
fn seven_modalities_implement_contract() {
    assert_eq!(
        PlanModality::ALL.len(),
        7,
        "the plan names seven modalities"
    );
    let mut kinds = Vec::new();
    for modality in PlanModality::ALL {
        let value = sample(modality);
        assert_eq!(
            value.storage_kind(),
            modality.storage_kind(),
            "{} implementation reports its declared storage kind",
            modality.name()
        );
        let row = value.to_rowset("row-1");
        assert_eq!(row.id, "row-1", "{} echoes the caller id", modality.name());
        assert_eq!(value.txn_stage("row-1").id, "row-1");
        kinds.push(modality.storage_kind());
    }
    kinds.sort();
    kinds.dedup();
    assert_eq!(
        kinds.len(),
        7,
        "one implementation per modality, no sharing"
    );
}
