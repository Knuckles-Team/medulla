//! Fold one modality's output into the cross-modal `RowSet` currency
//! (MEDULLA-FEDERATED-QUERY-R721.2).
//!
//! Each `(id, value)` contributes the row its [`ModalityContract::to_rowset`] names. A value
//! whose `storage_kind` is not the kind the caller folds is refused, so one modality
//! contributes only its own rows. The result is built through
//! `RowSet::from_epistemic_rows`: order is kept and ids are deduplicated (first wins).
//!
//! The kind is the `ModalityContract::storage_kind` string of the modality
//! (`medulla_plan::modality::PlanModality::storage_kind`). `medulla-exec` cannot name
//! `PlanModality` (`medulla-plan` depends on this crate), so the fold takes the kind.

use medulla_modality::ModalityContract;

use crate::rowset::{Row, RowSet};

/// A value handed to [`fold_modality`] that does not belong to the modality being folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalityFoldMismatch {
    pub expected_kind: String,
    pub row_id: String,
    pub found_storage_kind: &'static str,
}

impl std::fmt::Display for ModalityFoldMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MODALITY_FOLD_MISMATCH: row {} reports storage kind {} but the fold takes kind {}",
            self.row_id, self.found_storage_kind, self.expected_kind
        )
    }
}

impl std::error::Error for ModalityFoldMismatch {}

/// Fold `output` (values of the modality whose storage kind is `expected_kind`) into a
/// `RowSet`.
pub fn fold_modality(
    expected_kind: &str,
    output: &[(&str, &dyn ModalityContract)],
) -> Result<RowSet, ModalityFoldMismatch> {
    let mut rows = Vec::with_capacity(output.len());
    for (id, value) in output {
        let kind = value.storage_kind();
        if kind != expected_kind {
            return Err(ModalityFoldMismatch {
                expected_kind: expected_kind.to_string(),
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
        let folded = fold_modality("blob", &[("x", &a), ("x", &a), ("y", &a)])
            .expect("blob values fold under the blob kind");
        let ids: Vec<&str> = folded.rows().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["x", "y"], "first id wins, order kept");
        let err = fold_modality("ann", &[("x", &a)]).expect_err("a blob value is not a vector row");
        assert_eq!(err.found_storage_kind, "blob");
        assert_eq!(err.expected_kind, "ann");
    }
}
