//! Registration of the seven plan modalities into `ModalityRegistry` at server bootstrap
//! (MEDULLA-FEDERATED-QUERY-R721.2).
//!
//! Each `PlanModality` has one `ModalityContract` implementation in the crate that owns
//! its value type (R721.1). The registry is process-wide, so this runs once from
//! `compose_server_state`, not only from the conformance test batteries.

use medulla_modality::{register_modality, tck_report, ConformanceTestable, ModalityDescriptor};
use medulla_plan::modality::PlanModality;

fn descriptor<T: ConformanceTestable>(modality: PlanModality) -> ModalityDescriptor {
    ModalityDescriptor {
        name: modality.storage_kind(),
        tck_report: tck_report::<T>,
    }
}

/// The descriptor of each plan modality, in `PlanModality::ALL` order. The match is
/// exhaustive, so a new `PlanModality` variant fails to compile until it is registered.
pub fn plan_modality_descriptor(modality: PlanModality) -> ModalityDescriptor {
    match modality {
        PlanModality::Graph => descriptor::<medulla_graph_core::index::NodeChange>(modality),
        PlanModality::Vector => descriptor::<medulla_ann::flat::FlatIndex>(modality),
        PlanModality::Sql => {
            descriptor::<medulla_table_store::tables::store::TableSnapshotRow>(modality)
        }
        PlanModality::Rdf => descriptor::<medulla_rdf::sparql::Binding>(modality),
        PlanModality::TimeSeries => descriptor::<medulla_tsdb::store::SeriesMeta>(modality),
        PlanModality::KeyValue => descriptor::<medulla_kvcache::StoredBlock>(modality),
        PlanModality::Blob => {
            descriptor::<medulla_protocol_types::change_envelope::BlobReference>(modality)
        }
    }
}

/// Register every plan modality and check that each names one durable-store authority
/// (MEDULLA-FEDERATED-QUERY-R721.2). Registration is idempotent by name; an inconsistent
/// authority map is a refusal to boot, not a warning.
pub fn register_plan_modalities() -> Result<(), String> {
    medulla_persistence::durable_stores::verify_modality_authority()?;
    for modality in PlanModality::ALL {
        register_modality(plan_modality_descriptor(modality));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R721.2
    #[test]
    fn bootstrap_registers_every_plan_modality() {
        register_plan_modalities().expect("modality authority map is consistent");
        let registered = medulla_modality::registered_modalities();
        for modality in PlanModality::ALL {
            assert!(
                registered.iter().any(|d| d.name == modality.storage_kind()),
                "{} is not registered at bootstrap",
                modality.name()
            );
        }
    }
}
