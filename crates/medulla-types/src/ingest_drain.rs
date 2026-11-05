//! Drain, page-checkpoint, graph-route and stage-profile model
//! (MEDULLA-INGEST-WORKER-R605.1).
//!
//! Every identifier is validated at construction, and deserialization goes
//! through the same constructors, so an invalid value is never representable.

use serde::{Deserialize, Serialize};

/// Most bytes a page cursor may hold.
pub const MAX_CURSOR_BYTES: usize = 4096;
/// Most bytes a graph name may hold.
pub const MAX_GRAPH_BYTES: usize = 128;

/// Why a drain-model value is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IngestDrainError {
    #[error("drain_id must be a 64-character lowercase hex digest")]
    InvalidDrainId,
    #[error("page cursor exceeds {MAX_CURSOR_BYTES} bytes")]
    CursorTooLong,
    #[error("graph must match [A-Za-z0-9._:-]{{1,128}}")]
    InvalidGraph,
}

fn valid_drain_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_graph(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_GRAPH_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// Handle of one source drain.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "DrainHandleWire")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DrainHandle {
    drain_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
struct DrainHandleWire {
    drain_id: String,
}

impl TryFrom<DrainHandleWire> for DrainHandle {
    type Error = IngestDrainError;
    fn try_from(wire: DrainHandleWire) -> Result<Self, Self::Error> {
        Self::new(wire.drain_id)
    }
}

impl DrainHandle {
    pub fn new(drain_id: impl Into<String>) -> Result<Self, IngestDrainError> {
        let drain_id = drain_id.into();
        if !valid_drain_id(&drain_id) {
            return Err(IngestDrainError::InvalidDrainId);
        }
        Ok(Self { drain_id })
    }
    pub fn drain_id(&self) -> &str {
        &self.drain_id
    }
}

/// The checkpoint of one fetched page of a drain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "PageCheckpointWire")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PageCheckpoint {
    drain_id: String,
    page_index: u64,
    cursor: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
struct PageCheckpointWire {
    drain_id: String,
    page_index: u64,
    cursor: String,
}

impl TryFrom<PageCheckpointWire> for PageCheckpoint {
    type Error = IngestDrainError;
    fn try_from(wire: PageCheckpointWire) -> Result<Self, Self::Error> {
        Self::new(wire.drain_id, wire.page_index, wire.cursor)
    }
}

impl PageCheckpoint {
    pub fn new(
        drain_id: impl Into<String>,
        page_index: u64,
        cursor: impl Into<String>,
    ) -> Result<Self, IngestDrainError> {
        let drain_id = drain_id.into();
        let cursor = cursor.into();
        if !valid_drain_id(&drain_id) {
            return Err(IngestDrainError::InvalidDrainId);
        }
        if cursor.len() > MAX_CURSOR_BYTES {
            return Err(IngestDrainError::CursorTooLong);
        }
        Ok(Self {
            drain_id,
            page_index,
            cursor,
        })
    }
    pub fn drain_id(&self) -> &str {
        &self.drain_id
    }
    pub fn page_index(&self) -> u64 {
        self.page_index
    }
    pub fn cursor(&self) -> &str {
        &self.cursor
    }
}

/// The scope a graph route was chosen at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RouteScope {
    Source,
    Repository,
    Tenant,
    Default,
}

/// The graph a drained record is written to, and the scope that chose it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "GraphRouteWire")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GraphRoute {
    graph: String,
    scope: RouteScope,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
struct GraphRouteWire {
    graph: String,
    scope: RouteScope,
}

impl TryFrom<GraphRouteWire> for GraphRoute {
    type Error = IngestDrainError;
    fn try_from(wire: GraphRouteWire) -> Result<Self, Self::Error> {
        Self::new(wire.graph, wire.scope)
    }
}

impl GraphRoute {
    pub fn new(graph: impl Into<String>, scope: RouteScope) -> Result<Self, IngestDrainError> {
        let graph = graph.into();
        if !valid_graph(&graph) {
            return Err(IngestDrainError::InvalidGraph);
        }
        Ok(Self { graph, scope })
    }
    pub fn graph(&self) -> &str {
        &self.graph
    }
    pub fn scope(&self) -> RouteScope {
        self.scope
    }
}

/// Cost profile of one ingest stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StageProfile {
    pub stage: String,
    pub wall_ms: u64,
    pub tokens: u64,
    pub embeddings: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-INGEST-WORKER-R605.1
    #[test]
    fn types_round_trip_and_reject_bad_ids() {
        let id = "ab".repeat(32);
        let handle = DrainHandle::new(id.clone()).unwrap();
        let page = PageCheckpoint::new(id.clone(), 3, "cursor-3").unwrap();
        let route = GraphRoute::new("tenant:docs.main-1", RouteScope::Repository).unwrap();
        let profile = StageProfile {
            stage: "embed".to_string(),
            wall_ms: 12,
            tokens: 400,
            embeddings: 8,
        };
        let h: DrainHandle =
            serde_json::from_value(serde_json::to_value(&handle).unwrap()).unwrap();
        assert_eq!(h, handle);
        let p: PageCheckpoint =
            serde_json::from_value(serde_json::to_value(&page).unwrap()).unwrap();
        assert_eq!(p, page);
        let r: GraphRoute = serde_json::from_value(serde_json::to_value(&route).unwrap()).unwrap();
        assert_eq!(r, route);
        let s: StageProfile =
            serde_json::from_value(serde_json::to_value(&profile).unwrap()).unwrap();
        assert_eq!(s, profile);

        for bad in [
            "",
            "AB".repeat(32).as_str(),
            "ab".repeat(31).as_str(),
            "zz".repeat(32).as_str(),
        ] {
            assert_eq!(DrainHandle::new(bad), Err(IngestDrainError::InvalidDrainId));
            assert!(
                serde_json::from_value::<DrainHandle>(serde_json::json!({ "drain_id": bad }))
                    .is_err()
            );
        }
        assert_eq!(
            PageCheckpoint::new(id.clone(), 0, "c".repeat(4097)),
            Err(IngestDrainError::CursorTooLong)
        );
        assert!(PageCheckpoint::new(id, 0, "c".repeat(4096)).is_ok());
        for bad in ["", "has space", "slash/no", &"g".repeat(129)] {
            assert_eq!(
                GraphRoute::new(bad, RouteScope::Default),
                Err(IngestDrainError::InvalidGraph)
            );
        }
        assert!(serde_json::from_value::<GraphRoute>(
            serde_json::json!({ "graph": "g", "scope": "tenant", "extra": 1 })
        )
        .is_err());
    }
}
