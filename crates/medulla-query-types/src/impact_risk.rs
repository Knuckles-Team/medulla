//! The layer- and history-weighted impact of one `PROPAGATE ... RISK HISTORY` stage
//! (MEDULLA-FEDERATED-QUERY-R032.100).
//!
//! Lives beside the wire types because `UqlResult` references it and `medulla-types` cannot
//! see query types. Pure serde; the arithmetic lives in `medulla-exec`.

use serde::{Deserialize, Serialize};

use crate::wire::LayerWeight;

/// Whether a node has any counted observation in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// At least one counted observation.
    Known,
    /// No counted observation: the node contributes nothing to its paths.
    Unknown,
}

/// One reached node of a [`HistoricalRisk`], with the path that gave it its risk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NodeRisk {
    pub node_id: String,
    /// The node ids from the seed to `node_id`, both included.
    pub path: Vec<String>,
    /// One id per walked edge, `<from>-><to>` in the walked direction.
    pub edge_ids: Vec<String>,
    /// The node's `layer` property, when it has one.
    pub layer: Option<String>,
    pub observations: u64,
    pub failures: u64,
    /// `layer_weight * failure_rate`; absent when the node has no observation.
    pub layer_contribution: Option<f64>,
    /// `1 - product(1 - layer_contribution)` over the path's nodes; absent when unknown.
    pub path_risk: Option<f64>,
    pub coverage: Coverage,
}

/// The historical risk of one seed row, both dependency directions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HistoricalRisk {
    pub target: String,
    pub graph_revision: u64,
    pub hops: usize,
    pub layers: Vec<LayerWeight>,
    pub dataset: String,
    pub from_ms: i64,
    pub to_ms: i64,
    /// Lowercase hex digest over the dataset, window and every counted observation id.
    pub dataset_digest: String,
    pub upstream: Vec<NodeRisk>,
    pub downstream: Vec<NodeRisk>,
    /// The maximum `path_risk` over both directions; null when any node is unknown.
    pub aggregate_risk: Option<f64>,
    pub partial: bool,
    pub request_digest: String,
}
