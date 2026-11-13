//! The shared batch report and the provenance stamp of `SourceIngest`
//! (MEDULLA-INGEST-WORKER-R606.1).
//!
//! Pure data. `stamp_record` and `build_terminal_receipt` (R606.2) build these
//! types; the extractor registry is not an engine type, because vendor
//! extractors live in the SDK.

use serde::{Deserialize, Serialize};

/// Most per-class reports a receipt carries.
pub const MAX_CLASS_REPORTS: usize = 1_024;

/// What one batch did to its records: written, found unchanged (same content
/// hash) and refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestBatchReport {
    /// Records written (new or changed).
    pub written: u64,
    /// Records whose content hash matched the held version.
    pub unchanged: u64,
    /// Records refused; a refused record writes nothing.
    pub refused: u64,
}

impl IngestBatchReport {
    /// Every record the batch accounted for: `written + unchanged + refused`.
    pub fn total(&self) -> u64 {
        self.written
            .saturating_add(self.unchanged)
            .saturating_add(self.refused)
    }

    /// The sum of two reports (saturating).
    pub fn plus(&self, other: &IngestBatchReport) -> IngestBatchReport {
        IngestBatchReport {
            written: self.written.saturating_add(other.written),
            unchanged: self.unchanged.saturating_add(other.unchanged),
            refused: self.refused.saturating_add(other.refused),
        }
    }

    /// The sum of many reports.
    pub fn sum<'a>(reports: impl IntoIterator<Item = &'a IngestBatchReport>) -> IngestBatchReport {
        reports
            .into_iter()
            .fold(IngestBatchReport::default(), |acc, r| acc.plus(r))
    }
}

/// The report of one ontology class within a batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClassReport {
    /// The ontology class the records were materialized as.
    pub ontology_class: String,
    /// What the batch did to records of that class.
    pub report: IngestBatchReport,
}

/// Where a materialized record came from and when it was ingested.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MaterializeStamp {
    /// The source system (the connector's resource id).
    pub source_system: String,
    /// The record id in that system.
    pub source_id: String,
    /// The engine clock when the batch was ingested, in ms.
    pub ingest_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-INGEST-WORKER-R606.1
    #[test]
    fn report_round_trips_and_sums() {
        let a = IngestBatchReport {
            written: 3,
            unchanged: 2,
            refused: 1,
        };
        let b = IngestBatchReport {
            written: 10,
            unchanged: 0,
            refused: 4,
        };
        assert_eq!(a.total(), 6);
        let sum = IngestBatchReport::sum([&a, &b]);
        assert_eq!(
            sum,
            IngestBatchReport {
                written: 13,
                unchanged: 2,
                refused: 5
            }
        );
        assert_eq!(sum.total(), a.total() + b.total());
        let max = IngestBatchReport {
            written: u64::MAX,
            unchanged: 0,
            refused: 0,
        };
        assert_eq!(max.plus(&a).written, u64::MAX);

        let class = ClassReport {
            ontology_class: "https://knuckles-team.github.io/ontology/core#Document".into(),
            report: a,
        };
        let json = serde_json::to_string(&class).unwrap();
        assert_eq!(serde_json::from_str::<ClassReport>(&json).unwrap(), class);
        let stamp = MaterializeStamp {
            source_system: "wiki".into(),
            source_id: "page-1".into(),
            ingest_time_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&stamp).unwrap();
        assert_eq!(
            serde_json::from_str::<MaterializeStamp>(&json).unwrap(),
            stamp
        );
        assert!(serde_json::from_str::<IngestBatchReport>(
            r#"{"written":1,"unchanged":0,"refused":0,"extra":1}"#
        )
        .is_err());
    }
}
