//! Provenance of the graph rows a graph-sourced decision read
//! (MEDULLA-DECISION-ENGINE-R121.2).
//!
//! A graph candidate is a row of the caller's visible snapshot. The record keeps,
//! per row, which graph and version it was read at, the class the row's typed
//! label gives it, and (for evidence) the source it names, so the answer's
//! evidence class follows the row rather than a property name.

use serde::{Deserialize, Serialize};

/// Most graph rows one decision records provenance for.
pub const MAX_GRAPH_ROWS: usize = 64;

/// The class a graph row's typed label gives it. An `Evidence` node that names
/// a `source` is an observation; every other node is a claim. Property names
/// and subject IRIs never change the class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GraphNodeClass {
    Observation,
    Claim,
}

/// One graph row a decision read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GraphRowProvenance {
    /// The graph the row was read from.
    pub graph: String,
    pub node_id: String,
    pub node_class: GraphNodeClass,
    /// The IRI an evidence row names as its source; absent for a claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_iri: Option<String>,
    /// The graph's write version the row was read at.
    pub graph_version: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_round_trips_and_omits_an_absent_source() {
        let row = GraphRowProvenance {
            graph: "kg".to_string(),
            node_id: "n-1".to_string(),
            node_class: GraphNodeClass::Claim,
            source_iri: None,
            graph_version: 7,
        };
        let text = serde_json::to_string(&row).unwrap();
        assert!(!text.contains("source_iri"), "{text}");
        assert_eq!(
            serde_json::from_str::<GraphRowProvenance>(&text).unwrap(),
            row
        );
    }
}
