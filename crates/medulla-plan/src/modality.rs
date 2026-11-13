//! `PlanModality` — the closed set of data modalities a federated plan can fold into
//! the cross-modal `RowSet` currency (MEDULLA-FEDERATED-QUERY-R721.1).
//!
//! Each variant has exactly one `medulla_modality::ModalityContract` implementation, in
//! the crate that owns the modality's value type (see [`PlanModality::contract_owner`]).
//! `medulla_modality::ModalityKind` (artifact kinds) is a different enum and stays separate.

use serde::{Deserialize, Serialize};

/// The seven modalities a plan can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanModality {
    Graph,
    Vector,
    Sql,
    Rdf,
    TimeSeries,
    KeyValue,
    Blob,
}

impl PlanModality {
    /// Every modality, in declaration order.
    pub const ALL: [PlanModality; 7] = [
        PlanModality::Graph,
        PlanModality::Vector,
        PlanModality::Sql,
        PlanModality::Rdf,
        PlanModality::TimeSeries,
        PlanModality::KeyValue,
        PlanModality::Blob,
    ];

    /// Stable lowercase name, as used in UQL and error messages.
    pub fn name(self) -> &'static str {
        match self {
            PlanModality::Graph => "graph",
            PlanModality::Vector => "vector",
            PlanModality::Sql => "sql",
            PlanModality::Rdf => "rdf",
            PlanModality::TimeSeries => "time_series",
            PlanModality::KeyValue => "key_value",
            PlanModality::Blob => "blob",
        }
    }

    /// The `ModalityContract::storage_kind` string the owning implementation reports.
    pub fn storage_kind(self) -> &'static str {
        match self {
            PlanModality::Graph => "core",
            PlanModality::Vector => "ann",
            PlanModality::Sql => "sql",
            PlanModality::Rdf => "rdf",
            PlanModality::TimeSeries => "tsdb",
            PlanModality::KeyValue => "kvcache",
            PlanModality::Blob => "blob",
        }
    }

    /// The crate that owns this modality's `ModalityContract` implementation.
    pub fn contract_owner(self) -> &'static str {
        match self {
            PlanModality::Graph => "medulla-graph-core",
            PlanModality::Vector => "medulla-ann",
            PlanModality::Sql => "medulla-table-store",
            PlanModality::Rdf => "medulla-rdf",
            PlanModality::TimeSeries => "medulla-tsdb",
            PlanModality::KeyValue => "medulla-kvcache",
            PlanModality::Blob => "medulla-protocol-types",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_kinds_are_unique() {
        let mut names: Vec<_> = PlanModality::ALL.iter().map(|m| m.name()).collect();
        let mut kinds: Vec<_> = PlanModality::ALL.iter().map(|m| m.storage_kind()).collect();
        names.sort();
        names.dedup();
        kinds.sort();
        kinds.dedup();
        assert_eq!(names.len(), 7);
        assert_eq!(kinds.len(), 7);
    }
}
