//! Drain, page-checkpoint, graph-route and stage-profile model
//! (MEDULLA-INGEST-WORKER-R605.1).
//!
//! Every identifier is validated at construction, and deserialization goes
//! through the same constructors, so an invalid value is never representable.

use medulla_types::ingest_throughput::IngestStage;
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
    #[error("{0}")]
    PageTooLarge(String),
    #[error("an ack carries at most {MAX_PROFILE_ENTRIES} stage profiles")]
    TooManyProfiles,
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
    pub stage: IngestStage,
    pub wall_ms: u64,
    pub tokens: u64,
    pub embeddings: u64,
}

/// Most stage-profile entries one ack may carry: one per ingest stage.
pub const MAX_PROFILE_ENTRIES: usize = 4;
/// Node id prefix of a drain record in the graph.
pub const DRAIN_NODE_PREFIX: &str = "source_drain:";
/// Node id prefix of one routing record.
pub const ROUTE_NODE_PREFIX: &str = "ingest_route:";

/// The node id of the drain `drain_id` (MEDULLA-INGEST-WORKER-R605.2).
pub fn drain_node_id(drain_id: &str) -> String {
    format!("{DRAIN_NODE_PREFIX}{drain_id}")
}

/// The id of the routing record that names the graph of `key` at `scope`. `Default` has no record.
pub fn route_node_id(scope: RouteScope, key: &str) -> Option<String> {
    let name = match scope {
        RouteScope::Source => "source",
        RouteScope::Repository => "repository",
        RouteScope::Tenant => "tenant",
        RouteScope::Default => return None,
    };
    Some(format!("{ROUTE_NODE_PREFIX}{name}:{key}"))
}

/// What a worker reports when it acks one page task (MEDULLA-INGEST-WORKER-R605.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DrainAck {
    /// Records this page held.
    #[serde(default)]
    pub items: u64,
    /// The cursor of the next page; absent when the source has no more pages.
    #[serde(default)]
    pub next_cursor: Option<String>,
    /// Stage costs of this page.
    #[serde(default)]
    pub profile: Vec<StageProfile>,
}

impl DrainAck {
    /// Refuse a cursor over [`MAX_CURSOR_BYTES`] or more profile entries than there are stages.
    pub fn validate(&self) -> Result<(), IngestDrainError> {
        if self
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() > MAX_CURSOR_BYTES)
        {
            return Err(IngestDrainError::CursorTooLong);
        }
        if self.profile.len() > MAX_PROFILE_ENTRIES {
            return Err(IngestDrainError::TooManyProfiles);
        }
        Ok(())
    }
}

/// Where a drain stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DrainState {
    Running,
    Complete,
    Failed,
}

/// The served answer of `SourceDrainStatus` (MEDULLA-INGEST-WORKER-R605.5): totals are the sums
/// of the acks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DrainStatus {
    pub drain_id: String,
    pub pages: u64,
    pub items: u64,
    /// The cursor the next page starts at.
    pub cursor: String,
    pub state: DrainState,
    pub profile: Vec<StageProfile>,
}

/// The drain aggregate the graph node `source_drain:<id>` holds: the stored checkpoint (the next
/// page to fetch), the sums of the acks and the state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DrainRecord {
    pub drain_id: String,
    pub tenant: String,
    pub connector: String,
    pub stream: String,
    /// `full`, `delta` or `reconcile`.
    pub mode: String,
    /// The graph the drained records are written to.
    pub graph: String,
    /// Index of the next page to fetch: pages below it are acked.
    pub page_index: u64,
    /// Cursor of the next page; empty for page 0.
    pub cursor: String,
    pub pages: u64,
    pub items: u64,
    pub state: DrainState,
    pub profile: Vec<StageProfile>,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Why an ack does not advance a drain.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DrainAckError {
    /// The page is below the stored checkpoint, or the drain is closed (R605.4).
    #[error("page {page_index} is below the stored checkpoint {stored}")]
    Stale { page_index: u64, stored: u64 },
    /// The ack itself is malformed.
    #[error(transparent)]
    Invalid(#[from] IngestDrainError),
    /// The page held more records than the page bound.
    #[error("page holds {items} records, above the bound {max}")]
    PageTooLarge { items: u64, max: u64 },
}

fn add_profile(into: &mut Vec<StageProfile>, add: &[StageProfile]) {
    for entry in add {
        match into.iter_mut().find(|held| held.stage == entry.stage) {
            Some(held) => {
                held.wall_ms = held.wall_ms.saturating_add(entry.wall_ms);
                held.tokens = held.tokens.saturating_add(entry.tokens);
                held.embeddings = held.embeddings.saturating_add(entry.embeddings);
            }
            None => into.push(entry.clone()),
        }
    }
    into.sort_by_key(|entry| entry.stage);
}

impl DrainRecord {
    /// The status the served read answers.
    pub fn status(&self) -> DrainStatus {
        DrainStatus {
            drain_id: self.drain_id.clone(),
            pages: self.pages,
            items: self.items,
            cursor: self.cursor.clone(),
            state: self.state,
            profile: self.profile.clone(),
        }
    }

    /// The record after page `page_index` acked with `ack`: a cursor moves the checkpoint to the
    /// next page, no cursor closes the drain. A page below the stored checkpoint, or any page of
    /// a closed drain, is stale and changes nothing (MEDULLA-INGEST-WORKER-R605.4). `max_items`
    /// is the page bound `ingest.drain_page_records`.
    pub fn after_ack(
        &self,
        page_index: u64,
        ack: &DrainAck,
        max_items: u64,
        now_ms: i64,
    ) -> Result<DrainRecord, DrainAckError> {
        ack.validate()?;
        if page_index < self.page_index || self.state != DrainState::Running {
            return Err(DrainAckError::Stale {
                page_index,
                stored: self.page_index,
            });
        }
        if ack.items > max_items {
            return Err(DrainAckError::PageTooLarge {
                items: ack.items,
                max: max_items,
            });
        }
        let mut next = self.clone();
        next.pages = next.pages.saturating_add(1);
        next.items = next.items.saturating_add(ack.items);
        add_profile(&mut next.profile, &ack.profile);
        next.updated_at_ms = now_ms;
        match &ack.next_cursor {
            Some(cursor) => {
                next.page_index = page_index.saturating_add(1);
                next.cursor = cursor.clone();
            }
            None => next.state = DrainState::Complete,
        }
        Ok(next)
    }

    /// The record of a drain whose page task failed for good.
    pub fn failed(&self, now_ms: i64) -> DrainRecord {
        let mut next = self.clone();
        next.state = DrainState::Failed;
        next.updated_at_ms = now_ms;
        next
    }

    /// The checkpoint of the next page to fetch.
    pub fn checkpoint(&self) -> Result<PageCheckpoint, IngestDrainError> {
        PageCheckpoint::new(self.drain_id.clone(), self.page_index, self.cursor.clone())
    }
}

/// The graph names the routing records and the server config offer for one ingest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteInputs<'a> {
    /// The graph the connector's own routing record names.
    pub source: Option<&'a str>,
    /// The graph the repository's routing record names.
    pub repository: Option<&'a str>,
    /// The graph the tenant's routing record names.
    pub tenant: Option<&'a str>,
    /// `default_ingest_graph`, the server config key.
    pub default: &'a str,
}

/// The graph an ingest is written to (MEDULLA-INGEST-WORKER-R605.2): the first match of the
/// connector graph (`Source`), the repository graph (`Repository`) and the tenant graph
/// (`Tenant`), else `inputs.default` (`Default`). A blank name does not match; a name that is
/// not a valid graph name refuses. Pure.
pub fn route_ingest_graph(inputs: &RouteInputs<'_>) -> Result<GraphRoute, IngestDrainError> {
    let chosen = [
        (inputs.source, RouteScope::Source),
        (inputs.repository, RouteScope::Repository),
        (inputs.tenant, RouteScope::Tenant),
    ]
    .into_iter()
    .find_map(|(name, scope)| {
        name.filter(|name| !name.trim().is_empty())
            .map(|name| (name, scope))
    });
    match chosen {
        Some((graph, scope)) => GraphRoute::new(graph, scope),
        None => GraphRoute::new(inputs.default, RouteScope::Default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-INGEST-WORKER-R605.1
    /// Verifies MEDULLA-INGEST-WORKER-R605.1: Model: drains, page checkpoints, graph routes and stage profiles.
    #[test]
    fn types_round_trip_and_reject_bad_ids() {
        let id = "ab".repeat(32);
        let handle = DrainHandle::new(id.clone()).unwrap();
        let page = PageCheckpoint::new(id.clone(), 3, "cursor-3").unwrap();
        let route = GraphRoute::new("tenant:docs.main-1", RouteScope::Repository).unwrap();
        let profile = StageProfile {
            stage: IngestStage::Embed,
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

    fn record() -> DrainRecord {
        DrainRecord {
            drain_id: "ab".repeat(32),
            tenant: "acme".into(),
            connector: "crm".into(),
            stream: "accounts".into(),
            mode: "full".into(),
            graph: "main".into(),
            page_index: 0,
            cursor: String::new(),
            pages: 0,
            items: 0,
            state: DrainState::Running,
            profile: Vec::new(),
            started_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn profile(stage: IngestStage, wall_ms: u64) -> StageProfile {
        StageProfile {
            stage,
            wall_ms,
            tokens: 1,
            embeddings: 2,
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R605.2
    /// Verifies MEDULLA-INGEST-WORKER-R605.2: the first match wins in the order connector,
    /// repository, tenant, default.
    #[test]
    fn route_precedence() {
        let all = RouteInputs {
            source: Some("src"),
            repository: Some("repo"),
            tenant: Some("ten"),
            default: "main",
        };
        let route = route_ingest_graph(&all).unwrap();
        assert_eq!((route.graph(), route.scope()), ("src", RouteScope::Source));
        let route = route_ingest_graph(&RouteInputs { source: None, ..all }).unwrap();
        assert_eq!((route.graph(), route.scope()), ("repo", RouteScope::Repository));
        let route = route_ingest_graph(&RouteInputs {
            source: Some(" "),
            repository: None,
            ..all
        })
        .unwrap();
        assert_eq!((route.graph(), route.scope()), ("ten", RouteScope::Tenant));
        let route = route_ingest_graph(&RouteInputs {
            source: None,
            repository: None,
            tenant: None,
            default: "main",
        })
        .unwrap();
        assert_eq!((route.graph(), route.scope()), ("main", RouteScope::Default));
        assert_eq!(
            route_ingest_graph(&RouteInputs {
                source: Some("bad name"),
                ..all
            }),
            Err(IngestDrainError::InvalidGraph)
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R605.2
    #[test]
    fn an_ack_with_a_cursor_advances_and_one_without_closes() {
        let ack = DrainAck {
            items: 40,
            next_cursor: Some("c1".into()),
            profile: vec![profile(IngestStage::Fetch, 5)],
        };
        let one = record().after_ack(0, &ack, 500, 10).unwrap();
        assert_eq!((one.page_index, one.cursor.as_str(), one.pages, one.items), (1, "c1", 1, 40));
        assert_eq!(one.state, DrainState::Running);
        let last = DrainAck {
            items: 2,
            next_cursor: None,
            profile: vec![profile(IngestStage::Fetch, 7), profile(IngestStage::Embed, 3)],
        };
        let two = one.after_ack(1, &last, 500, 20).unwrap();
        assert_eq!((two.pages, two.items, two.state), (2, 42, DrainState::Complete));
        assert_eq!(two.profile.len(), 2);
        assert_eq!(two.profile[0].stage, IngestStage::Fetch);
        assert_eq!(two.profile[0].wall_ms, 12);
        assert_eq!(two.status().cursor, "c1");
    }

    // spec: MEDULLA-INGEST-WORKER-R605.4
    #[test]
    fn a_page_below_the_checkpoint_or_of_a_closed_drain_is_stale() {
        let ack = DrainAck {
            items: 1,
            next_cursor: Some("c".into()),
            profile: Vec::new(),
        };
        let advanced = record().after_ack(0, &ack, 500, 10).unwrap();
        assert_eq!(
            advanced.after_ack(0, &ack, 500, 11),
            Err(DrainAckError::Stale { page_index: 0, stored: 1 })
        );
        let closed = advanced
            .after_ack(1, &DrainAck::default(), 500, 12)
            .unwrap();
        assert!(matches!(
            closed.after_ack(5, &ack, 500, 13),
            Err(DrainAckError::Stale { .. })
        ));
        assert!(matches!(
            record().after_ack(0, &DrainAck { items: 501, ..ack.clone() }, 500, 1),
            Err(DrainAckError::PageTooLarge { items: 501, max: 500 })
        ));
        let long = DrainAck {
            next_cursor: Some("c".repeat(4097)),
            ..ack
        };
        assert_eq!(
            record().after_ack(0, &long, 500, 1),
            Err(DrainAckError::Invalid(IngestDrainError::CursorTooLong))
        );
    }
}
