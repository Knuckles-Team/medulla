//! The spec intelligence report (MEDULLA-SDLC-R014.13).
//!
//! `Method::UnifiedQuery` carries an optional [`SpecReportQuery`] beside `code_map`. The served
//! answer is a [`SpecIntelligenceReport`]: the clusters of spec rows that several repositories
//! share terms in, and the rows that break a shape of the spec standard. The report is a read:
//! it writes nothing.

use serde::{Deserialize, Serialize};

/// The Leiden resolution of a query that names none, in millionths (1.0).
pub const DEFAULT_RESOLUTION_MICROS: u32 = 1_000_000;

/// The least resolution a query may name, in millionths (0.1).
pub const MIN_RESOLUTION_MICROS: u32 = 100_000;

/// The greatest resolution a query may name, in millionths (10.0).
pub const MAX_RESOLUTION_MICROS: u32 = 10_000_000;

/// Stable refusal: the query names a resolution outside the allowed range.
pub const SPEC_REPORT_INVALID: &str = "SPEC_REPORT_INVALID";

/// Stable refusal: a spec report arrived with a plan or another alternative body, so a write
/// could ride on it.
pub const REPORT_READ_ONLY: &str = "REPORT_READ_ONLY";

fn default_resolution_micros() -> u32 {
    DEFAULT_RESOLUTION_MICROS
}

/// The `spec_report` field of `Method::UnifiedQuery`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SpecReportQuery {
    /// The clustering resolution in millionths, `MIN_RESOLUTION_MICROS..=MAX_RESOLUTION_MICROS`.
    #[serde(default = "default_resolution_micros")]
    pub resolution_micros: u32,
    /// The clustering seed.
    #[serde(default)]
    pub seed: u64,
}

impl Default for SpecReportQuery {
    fn default() -> Self {
        Self {
            resolution_micros: DEFAULT_RESOLUTION_MICROS,
            seed: 0,
        }
    }
}

impl SpecReportQuery {
    /// Refuse `SPEC_REPORT_INVALID` for a resolution outside the allowed range.
    pub fn validate(&self) -> Result<(), String> {
        if (MIN_RESOLUTION_MICROS..=MAX_RESOLUTION_MICROS).contains(&self.resolution_micros) {
            Ok(())
        } else {
            Err(format!(
                "{SPEC_REPORT_INVALID}: resolution_micros {} is outside {MIN_RESOLUTION_MICROS}..={MAX_RESOLUTION_MICROS}",
                self.resolution_micros
            ))
        }
    }
}

/// One cluster of rows of two or more repositories that share terms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OverlapCandidate {
    /// `l1:<index>`.
    pub cluster_id: String,
    /// The sorted row ids of the cluster.
    pub row_ids: Vec<String>,
    /// The sorted names of the terms that two or more rows of the cluster use.
    pub shared_terms: Vec<String>,
    /// The sorted distinct repositories of the cluster's rows.
    pub repositories: Vec<String>,
}

/// One row that breaks one shape of the spec standard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ShapeViolation {
    pub row_id: String,
    /// The refusal code the shape's message names.
    pub code: String,
}

/// What a served spec report answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SpecIntelligenceReport {
    /// The overlap candidates, ordered by `cluster_id`.
    pub overlaps: Vec<OverlapCandidate>,
    /// The shape violations, ordered by row id and code.
    pub violations: Vec<ShapeViolation>,
    /// The number of requirement rows the report read.
    pub rows: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-SDLC-R014.13
    #[test]
    fn the_query_defaults_and_bounds_its_resolution() {
        let defaulted: SpecReportQuery = serde_json::from_str("{}").expect("an empty query");
        assert_eq!(defaulted, SpecReportQuery::default());
        assert_eq!(defaulted.resolution_micros, 1_000_000);
        assert!(defaulted.validate().is_ok());
        for micros in [99_999, 10_000_001, 0] {
            let query = SpecReportQuery {
                resolution_micros: micros,
                seed: 0,
            };
            let refusal = query.validate().unwrap_err();
            assert!(refusal.starts_with(SPEC_REPORT_INVALID), "{refusal}");
        }
        assert!(SpecReportQuery {
            resolution_micros: 100_000,
            seed: 9
        }
        .validate()
        .is_ok());
        assert!(serde_json::from_str::<SpecReportQuery>(r#"{"seed":1,"extra":2}"#).is_err());
    }
}
