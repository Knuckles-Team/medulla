//! `PlanModality` — the closed set of data modalities a federated plan can fold into
//! the cross-modal `RowSet` currency (MEDULLA-FEDERATED-QUERY-R721.1).
//!
//! Each variant has exactly one `medulla_modality::ModalityContract` implementation, in
//! the crate that owns the modality's value type (see [`PlanModality::contract_owner`]).
//! `medulla_modality::ModalityKind` (artifact kinds) is a different enum and stays separate.

use medulla_exec::rowset::{Row, RowSet};
use medulla_modality::ModalityContract;
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

/// A value handed to [`fold_modality`] that does not belong to the modality being folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalityFoldMismatch {
    pub modality: PlanModality,
    pub row_id: String,
    pub found_storage_kind: &'static str,
}

impl std::fmt::Display for ModalityFoldMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "row {} reports storage kind {} but the {} modality folds kind {}",
            self.row_id,
            self.found_storage_kind,
            self.modality.name(),
            self.modality.storage_kind()
        )
    }
}

impl std::error::Error for ModalityFoldMismatch {}

/// Fold one modality's output into the cross-modal `RowSet` currency
/// (MEDULLA-FEDERATED-QUERY-R721.2). Each `(id, value)` contributes the row its
/// `ModalityContract::to_rowset` names; a value whose `storage_kind` is not the
/// modality's is refused, so a modality can only contribute its own rows. The result is
/// built through `RowSet::from_epistemic_rows`, so order is kept and ids are deduplicated
/// (first wins).
///
/// `medulla-exec` cannot name `PlanModality` (this crate depends on it), so the fold
/// lives here, beside the enum it takes.
pub fn fold_modality(
    modality: PlanModality,
    output: &[(&str, &dyn ModalityContract)],
) -> Result<RowSet, ModalityFoldMismatch> {
    let mut rows = Vec::with_capacity(output.len());
    for (id, value) in output {
        let kind = value.storage_kind();
        if kind != modality.storage_kind() {
            return Err(ModalityFoldMismatch {
                modality,
                row_id: (*id).to_string(),
                found_storage_kind: kind,
            });
        }
        let shape = value.to_rowset(id);
        rows.push(Row::new(shape.id, shape.score));
    }
    Ok(RowSet::from_epistemic_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value that reports a fixed storage kind and contributes an unranked row.
    struct Probe(&'static str);

    impl ModalityContract for Probe {
        fn storage_kind(&self) -> &'static str {
            self.0
        }

        fn to_rowset(&self, id: &str) -> medulla_modality::RowSetShape {
            medulla_modality::RowSetShape::unranked(id)
        }

        fn txn_stage(&self, id: &str) -> medulla_modality::StagedWrite {
            medulla_modality::StagedWrite::put(id, Vec::new())
        }

        fn cdc_topic(&self) -> Option<&'static str> {
            None
        }
    }

    /// MEDULLA-FEDERATED-QUERY-R721.2: a fold keeps the contract's row and refuses a value
    /// of another modality.
    #[test]
    fn fold_modality_keeps_contract_rows_and_refuses_foreign_kinds() {
        let a = Probe("blob");
        let folded = fold_modality(PlanModality::Blob, &[("x", &a), ("x", &a), ("y", &a)])
            .expect("blob values fold under the blob modality");
        let ids: Vec<&str> = folded.rows().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["x", "y"], "first id wins, order kept");
        let err = fold_modality(PlanModality::Vector, &[("x", &a)])
            .expect_err("a blob value is not a vector row");
        assert_eq!(err.found_storage_kind, "blob");
        assert_eq!(err.modality, PlanModality::Vector);
    }

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
