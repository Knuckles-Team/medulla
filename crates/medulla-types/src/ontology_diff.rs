//! Typed summaries of ontology revision changes and their impact.

use serde::{Deserialize, Serialize};

/// The canonical axiom changes between two ontology revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OntologyDiff {
    pub from_rev: String,
    pub to_rev: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub deprecated: Vec<String>,
}

/// Counts of held data and consumers affected by an ontology revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangeImpact {
    pub instances_invalid: u64,
    pub shapes_broken: u64,
    pub mappings_broken: u64,
    pub queries_broken: u64,
}

/// The ids behind each [`ChangeImpact`] count, each list sorted
/// (MEDULLA-TYPED-PACKS-R804.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ImpactItems {
    pub instances: Vec<String>,
    pub shapes: Vec<String>,
    pub mappings: Vec<String>,
    pub queries: Vec<String>,
}

/// What `GraphSchema.DiffOntology` returns: the axiom diff, the impact counts
/// and the ids behind each count (MEDULLA-TYPED-PACKS-R804.5). The counts are
/// exact; each id list is sorted and holds at most `ontology_diff.max_listed`
/// ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OntologyDiffReport {
    pub diff: OntologyDiff,
    pub impact: ChangeImpact,
    pub items: ImpactItems,
}
