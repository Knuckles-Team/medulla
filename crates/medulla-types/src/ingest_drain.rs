//! Drains, page checkpoints, graph routes and stage profiles
//! (MEDULLA-INGEST-WORKER-R605.1).
//!
//! Pure data; every identifier is validated at construction and on decode.

use serde::{Deserialize, Deserializer, Serialize};

/// Longest page cursor, in bytes.
pub const MAX_CURSOR_BYTES: usize = 4096;

/// A drain record that failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainError {
    /// `drain_id` is not 64 lowercase hex characters.
    DrainId,
    /// The cursor exceeds 4096 bytes.
    Cursor,
    /// The graph name does not match `[A-Za-z0-9._:-]{1,128}`.
    Graph,
}

impl std::fmt::Display for DrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DrainId => "drain_id must be 64 lowercase hex characters",
            Self::Cursor => "cursor must be at most 4096 bytes",
            Self::Graph => "graph must match [A-Za-z0-9._:-]{1,128}",
        })
    }
}

impl std::error::Error for DrainError {}

fn check_drain_id(id: &str) -> Result<(), DrainError> {
    if id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(DrainError::DrainId)
    }
}

/// Identity of one source drain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DrainHandle {
    drain_id: String,
}

impl DrainHandle {
    pub fn new(drain_id: impl Into<String>) -> Result<Self, DrainError> {
        let drain_id = drain_id.into();
        check_drain_id(&drain_id)?;
        Ok(Self { drain_id })
    }

    pub fn drain_id(&self) -> &str {
        &self.drain_id
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHandle {
    drain_id: String,
}

impl<'de> Deserialize<'de> for DrainHandle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawHandle::deserialize(d)?;
        Self::new(raw.drain_id).map_err(serde::de::Error::custom)
    }
}

/// Resume point of one page of a drain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PageCheckpoint {
    drain_id: String,
    page_index: u64,
    cursor: String,
}

impl PageCheckpoint {
    pub fn new(
        drain_id: impl Into<String>,
        page_index: u64,
        cursor: impl Into<String>,
    ) -> Result<Self, DrainError> {
        let drain_id = drain_id.into();
        let cursor = cursor.into();
        check_drain_id(&drain_id)?;
        if cursor.len() > MAX_CURSOR_BYTES {
            return Err(DrainError::Cursor);
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheckpoint {
    drain_id: String,
    page_index: u64,
    cursor: String,
}

impl<'de> Deserialize<'de> for PageCheckpoint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawCheckpoint::deserialize(d)?;
        Self::new(raw.drain_id, raw.page_index, raw.cursor).map_err(serde::de::Error::custom)
    }
}

/// What a graph route is keyed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RouteScope {
    Source,
    Repository,
    Tenant,
    Default,
}

/// The graph a batch is written to and the scope that chose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GraphRoute {
    graph: String,
    scope: RouteScope,
}

impl GraphRoute {
    pub fn new(graph: impl Into<String>, scope: RouteScope) -> Result<Self, DrainError> {
        let graph = graph.into();
        let ok = !graph.is_empty()
            && graph.len() <= 128
            && graph
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
        if !ok {
            return Err(DrainError::Graph);
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoute {
    graph: String,
    scope: RouteScope,
}

impl<'de> Deserialize<'de> for GraphRoute {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawRoute::deserialize(d)?;
        Self::new(raw.graph, raw.scope).map_err(serde::de::Error::custom)
    }
}

/// Cost of one pipeline stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    fn id() -> String {
        "ab".repeat(32)
    }

    // spec: MEDULLA-INGEST-WORKER-R605.1
    #[test]
    fn types_round_trip_and_reject_bad_ids() {
        let handle = DrainHandle::new(id()).unwrap();
        let page = PageCheckpoint::new(id(), 3, "cursor-1").unwrap();
        let route = GraphRoute::new("tenant:graph-1.v2", RouteScope::Repository).unwrap();
        let stage = StageProfile { stage: "embed".into(), wall_ms: 5, tokens: 6, embeddings: 7 };
        macro_rules! rt {
            ($v:expr, $t:ty) => {{
                let json = serde_json::to_value(&$v).unwrap();
                assert_eq!(serde_json::from_value::<$t>(json).unwrap(), $v);
                let packed: $t = rmp_serde::from_slice(&rmp_serde::to_vec_named(&$v).unwrap()).unwrap();
                assert_eq!(packed, $v);
            }};
        }
        rt!(handle, DrainHandle);
        rt!(page, PageCheckpoint);
        rt!(route, GraphRoute);
        rt!(stage, StageProfile);
        for scope in [RouteScope::Source, RouteScope::Repository, RouteScope::Tenant, RouteScope::Default] {
            let v = serde_json::to_value(scope).unwrap();
            assert_eq!(serde_json::from_value::<RouteScope>(v).unwrap(), scope);
        }

        for bad in ["", "AB".repeat(32).as_str(), &"a".repeat(63), &"g".repeat(64)] {
            assert_eq!(DrainHandle::new(bad).unwrap_err(), DrainError::DrainId);
            assert!(serde_json::from_value::<DrainHandle>(serde_json::json!({"drain_id": bad})).is_err());
        }
        assert_eq!(
            PageCheckpoint::new(id(), 0, "x".repeat(4097)).unwrap_err(),
            DrainError::Cursor
        );
        assert!(PageCheckpoint::new(id(), 0, "x".repeat(4096)).is_ok());
        for bad in ["", "has space", &"g".repeat(129)] {
            assert_eq!(GraphRoute::new(bad, RouteScope::Default).unwrap_err(), DrainError::Graph);
        }
        assert!(serde_json::from_value::<GraphRoute>(
            serde_json::json!({"graph": "g", "scope": "tenant", "extra": 1})
        )
        .is_err());
    }
}
