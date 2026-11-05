//! The shared batch report and the provenance stamp
//! (MEDULLA-INGEST-WORKER-R606.1).
//!
//! Content hashes reuse `Digest256`; provenance reuses
//! `SourceRecordProvenance` of `medulla-source-types`. The extractor registry
//! is not an engine type: vendor extractors live in the SDK.

use serde::{Deserialize, Serialize};

/// Most per-class reports a receipt may carry.
pub const MAX_CLASS_REPORTS: usize = 1_024;

/// Counters of one materialization batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestBatchReport {
    pub written: u64,
    pub unchanged: u64,
    pub refused: u64,
}

impl IngestBatchReport {
    /// Sum two reports, refusing on overflow.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            written: self.written.checked_add(other.written)?,
            unchanged: self.unchanged.checked_add(other.unchanged)?,
            refused: self.refused.checked_add(other.refused)?,
        })
    }

    /// Whether every counter is zero; an empty report stays off the wire.
    pub fn is_empty(&self) -> bool {
        self.written == 0 && self.unchanged == 0 && self.refused == 0
    }

    /// Records the batch saw in total.
    pub fn total(&self) -> Option<u64> {
        self.written
            .checked_add(self.unchanged)?
            .checked_add(self.refused)
    }
}

/// The report of one ontology class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClassReport {
    pub ontology_class: String,
    pub report: IngestBatchReport,
}

/// Where and when a batch came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MaterializeStamp {
    pub source_system: String,
    pub source_id: String,
    pub ingest_time_ms: u64,
}

/// Total of per-class reports, `None` on overflow or when more than 1 024
/// classes are given.
pub fn sum_classes(classes: &[ClassReport]) -> Option<IngestBatchReport> {
    if classes.len() > MAX_CLASS_REPORTS {
        return None;
    }
    classes
        .iter()
        .try_fold(IngestBatchReport::default(), |acc, c| acc.checked_add(c.report))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-INGEST-WORKER-R606.1
    #[test]
    fn report_round_trips_and_sums() {
        let a = ClassReport {
            ontology_class: "Doc".into(),
            report: IngestBatchReport { written: 2, unchanged: 3, refused: 1 },
        };
        let b = ClassReport {
            ontology_class: "Person".into(),
            report: IngestBatchReport { written: 5, unchanged: 0, refused: 0 },
        };
        let sum = sum_classes(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(sum, IngestBatchReport { written: 7, unchanged: 3, refused: 1 });
        assert_eq!(sum.total(), Some(11));
        assert!(IngestBatchReport::default().is_empty() && !sum.is_empty());
        let max = ClassReport {
            ontology_class: "X".into(),
            report: IngestBatchReport { written: u64::MAX, unchanged: 0, refused: 0 },
        };
        assert_eq!(sum_classes(&[max, a.clone()]), None);
        assert_eq!(sum_classes(&vec![a.clone(); MAX_CLASS_REPORTS + 1]), None);

        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(serde_json::from_value::<ClassReport>(json).unwrap(), a);
        let stamp = MaterializeStamp {
            source_system: "s".into(),
            source_id: "i".into(),
            ingest_time_ms: 9,
        };
        let packed: MaterializeStamp =
            rmp_serde::from_slice(&rmp_serde::to_vec_named(&stamp).unwrap()).unwrap();
        assert_eq!(packed, stamp);
        assert!(serde_json::from_value::<IngestBatchReport>(
            serde_json::json!({"written": 1, "unchanged": 0, "refused": 0, "x": 1})
        )
        .is_err());
    }
}
