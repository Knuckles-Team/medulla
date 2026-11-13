//! Wire types of the served `Method::SourceCoverage` (MEDULLA-SRC-AUTH-R004.6.1).
//!
//! The selection engine keeps its verdicts and premises internal. This module is
//! the one serializable form a caller receives: each verdict carries its premises
//! rendered to text, and the reply embeds the [`AuthorityAudit`] built from those
//! same premises (R007.2).

use serde::{Deserialize, Serialize};

use crate::authority_audit::{AuditSourceMode, AuthorityAudit, NotUsedReason};

/// Refusal: the request named no class to cover.
pub const SOURCE_COVERAGE_NO_CLASSES: &str = "SOURCE_COVERAGE_NO_CLASSES";
/// Refusal: the request named more classes than `source_coverage.max_classes`.
pub const SOURCE_COVERAGE_TOO_MANY_CLASSES: &str = "SOURCE_COVERAGE_TOO_MANY_CLASSES";
/// Refusal: the hop budget lies outside `[0, 8]`.
pub const SOURCE_COVERAGE_HOP_BUDGET_INVALID: &str = "SOURCE_COVERAGE_HOP_BUDGET_INVALID";

/// The family refusal table of `SourceCoverage`: every code the method answers.
pub const REFUSALS: &[&str] = &[
    SOURCE_COVERAGE_NO_CLASSES,
    SOURCE_COVERAGE_TOO_MANY_CLASSES,
    SOURCE_COVERAGE_HOP_BUDGET_INVALID,
];

/// Largest hop budget a request may carry.
pub const MAX_HOP_BUDGET: u32 = 8;

/// One `(class, property)` facet a question reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceCoverageFacet {
    /// The ontology class the facet belongs to.
    pub class: String,
    /// The property within the class.
    pub property: String,
}

/// One hop on the path to a covered class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CoverageHop {
    /// The class the hop leaves.
    pub from_class: String,
    /// The class the hop reaches.
    pub to_class: String,
    /// The premises that justify the hop, rendered.
    pub premises: Vec<String>,
}

/// The verdict for one class or facet, as a caller receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CoverageVerdict {
    /// The class (or facet) is served by `mapping_name` of `source`.
    Covered {
        /// The covered class.
        class: String,
        /// The facet, when the verdict is facet-scoped.
        facet: Option<String>,
        /// The authoritative source.
        source: String,
        /// The approved mapping that serves it.
        mapping_name: String,
        /// The lower-ranked candidates for the same class, in selection order.
        alternates: Vec<String>,
        /// The hops from a queried root to the class.
        path: Vec<CoverageHop>,
        /// The justifying premises, rendered, each citing its source generation.
        premises: Vec<String>,
        /// `live` or `accelerated`.
        mode: AuditSourceMode,
    },
    /// The class lies within the hop budget but no effective mapping serves it.
    Uncovered {
        /// The unserved class.
        class: String,
        /// The premises, rendered.
        premises: Vec<String>,
    },
    /// The class has no path to any queried root within the hop budget.
    Disconnected {
        /// The unreached class.
        class: String,
    },
    /// The authoritative row cannot answer and no declared fallback applies.
    Abstained {
        /// The class that went unanswered.
        class: String,
        /// The facet, when the verdict is facet-scoped.
        facet: Option<String>,
        /// The closed reason.
        reason: NotUsedReason,
        /// The blocked source, when one exists.
        source: Option<String>,
        /// The premises, rendered.
        premises: Vec<String>,
    },
}

/// The reply of `Method::SourceCoverage`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceCoverageReply {
    /// One verdict per class in the selection universe, then one per facet.
    pub verdicts: Vec<CoverageVerdict>,
    /// The audit, built from the verdicts' premises only.
    pub audit: AuthorityAudit,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-SRC-AUTH-R004.6.1
    #[test]
    fn the_reply_round_trips_and_names_its_refusals() {
        let reply = SourceCoverageReply {
            verdicts: vec![
                CoverageVerdict::Disconnected { class: "c".into() },
                CoverageVerdict::Abstained {
                    class: "k".into(),
                    facet: None,
                    reason: NotUsedReason::Unconnected,
                    source: Some("s".into()),
                    premises: vec!["p".into()],
                },
            ],
            audit: AuthorityAudit::default(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains("\"verdict\":\"abstained\""));
        let back: SourceCoverageReply = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reply);
        assert_eq!(REFUSALS.len(), 3);
    }
}
