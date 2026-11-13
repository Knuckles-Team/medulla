//! `Method::OpenGraphSnapshot` and `Method::ReadGraphChanges`
//! (MEDULLA-DURABLE-KERNEL-R043.100.2, R043.100.3).
//!
//! The feed is downstream of the one change feed the CDC hub keeps: every
//! committed graph write already emits one ordered event there, so the feed
//! adds no second write path and no second store. A cursor binds the tenant,
//! the graph, the feed incarnation, the caller's authorization revision and the
//! next position. A read refuses a cursor whose binding no longer holds with a
//! typed code, and never answers a withheld event's existence.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_server_api::knob_config::knobs;
use medulla_server_cdc::cdc::{CdcHub, CdcStateExt};
use medulla_types::contract::ServerErrorCode;
use medulla_types::graph_change_feed::{
    authorization_revision, snapshot_digest, GraphChange, GraphChangeCursor, GraphChangeEvent,
    GraphChangePage, GraphSnapshot, RowOperation, GRAPH_CHANGE_FEED_SCHEMA_VERSION,
};
use medulla_types::wire::{CdcEvent, CdcKind};

/// Revision of the latency accounting the events carry.
const LATENCY_PROFILE_REVISION: u64 = 1;

/// The verified caller of one feed request.
pub(super) struct FeedCaller {
    pub(super) tenant: String,
    pub(super) authorization_revision: u64,
}

/// Why a feed request is refused; each variant is one registered code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FeedRefusal {
    InvalidArgument(String),
    AuthorizationChanged,
    IncarnationStale { safe_cursor: String },
    CursorExpired { safe_cursor: String },
}

impl FeedRefusal {
    fn text(&self) -> String {
        match self {
            Self::InvalidArgument(reason) => format!("INVALID_ARGUMENT: {reason}"),
            Self::AuthorizationChanged => format!(
                "{}: the caller's authorization changed since the cursor was issued; open a new snapshot",
                ServerErrorCode::AuthorizationChanged.as_str()
            ),
            Self::IncarnationStale { safe_cursor } => format!(
                "{}: the graph was recreated since the cursor was issued; safe_cursor={safe_cursor}",
                ServerErrorCode::GraphIncarnationStale.as_str()
            ),
            Self::CursorExpired { safe_cursor } => format!(
                "{}: the cursor fell behind the retained window; safe_cursor={safe_cursor}",
                ServerErrorCode::CursorExpired.as_str()
            ),
        }
    }
}

fn caller_of(ctx: &GraphOpsContext<'_>) -> Result<FeedCaller, String> {
    let carrier = ctx
        .read_authority
        .carrier()
        .ok_or_else(|| "ACCESS_DENIED: the change feed needs a verified caller".to_string())?;
    Ok(FeedCaller {
        tenant: carrier.tenant_scope().to_string(),
        authorization_revision: authorization_revision(carrier.agent_id(), carrier.scopes()),
    })
}

async fn hub_of(ctx: &GraphOpsContext<'_>) -> Option<Arc<CdcHub>> {
    let state = ctx.state.read().await;
    state.cdc_hub().cloned()
}

/// Route the two feed methods served from the graph.
pub(super) async fn try_handle_graph_change_feed(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    if !matches!(
        method,
        Method::OpenGraphSnapshot | Method::ReadGraphChanges { .. }
    ) {
        return ControlFlow::Continue(method);
    }
    let Some(hub) = hub_of(&ctx).await else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            "ENGINE_UNAVAILABLE: streaming/CDC not configured",
        ));
    };
    let caller = match caller_of(&ctx) {
        Ok(caller) => caller,
        Err(text) => return ControlFlow::Break(Response::err(ctx.req_id, text)),
    };
    let response = match method {
        Method::OpenGraphSnapshot => Response::ok(
            ctx.req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::graph::OpenGraphSnapshot>(
                open_snapshot(&hub, &caller, ctx.graph_name, ctx.core),
            ),
        ),
        Method::ReadGraphChanges {
            cursor,
            limit,
            table,
        } => {
            let persist_dir = ctx.state.read().await.persist_dir.clone();
            let row_access =
                |table: &str| row_access_of(ctx.read_authority, persist_dir.as_deref(), table);
            match read_changes(
                &hub,
                &caller,
                ctx.read_authority,
                ctx.graph_name,
                ReadRequest {
                    cursor: &cursor,
                    limit,
                    table: table.as_deref(),
                    row_access: &row_access,
                },
            ) {
                Ok(page) => Response::ok(
                    ctx.req_id,
                    ResultPayload::of::<
                        medulla_protocol_types::result_contract::graph::ReadGraphChanges,
                    >(page),
                ),
                Err(refusal) => Response::err(ctx.req_id, refusal.text()),
            }
        }
        other => return ControlFlow::Continue(other),
    };
    ControlFlow::Break(response)
}

fn cursor_at(caller: &FeedCaller, graph: &str, incarnation: u64, last_event: u64) -> String {
    GraphChangeCursor {
        graph: graph.to_string(),
        incarnation,
        authorization_revision: caller.authorization_revision,
        last_event,
    }
    .encode(&caller.tenant)
}

/// The digest of the caller's visible graph and the cursor that continues
/// after it. The feed position is read before the graph: a commit that lands
/// between the two reads is in the digest and is delivered again by the feed
/// (at-least-once, stable ids), so the pages leave no gap.
pub(super) fn open_snapshot(
    hub: &CdcHub,
    caller: &FeedCaller,
    graph: &str,
    core: &GraphCore,
) -> GraphSnapshot {
    let position = hub.feed_position(graph);
    let mut nodes = core.get_nodes();
    nodes.sort_by(|left, right| left.0.cmp(&right.0));
    let mut edges = core.get_edges();
    edges.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    let mut body = Vec::new();
    for (id, properties) in &nodes {
        body.extend_from_slice(b"n\0");
        body.extend_from_slice(id.as_bytes());
        body.push(0);
        body.extend_from_slice(&(properties.len() as u64).to_be_bytes());
        body.extend_from_slice(properties);
    }
    for (source, target, properties) in &edges {
        body.extend_from_slice(b"e\0");
        body.extend_from_slice(source.as_bytes());
        body.push(0);
        body.extend_from_slice(target.as_bytes());
        body.push(0);
        body.extend_from_slice(&(properties.len() as u64).to_be_bytes());
        body.extend_from_slice(properties);
    }
    GraphSnapshot {
        schema_version: GRAPH_CHANGE_FEED_SCHEMA_VERSION,
        incarnation: position.incarnation,
        committed_revision: position.head_seq,
        snapshot_digest: snapshot_digest(&body),
        visibility_revision: caller.authorization_revision,
        high_water_cursor: cursor_at(caller, graph, position.incarnation, position.head_seq),
    }
}

/// Whether the caller may see the change: one of its images passes the same
/// row-level predicate every other read applies. Reserved topic markers carry
/// no image and are not graph changes.
fn visible_to(authority: &GraphReadAuthority, event: &CdcEvent) -> bool {
    (event.had_before && authority.can_see_blob(&event.before))
        || (event.had_after && authority.can_see_blob(&event.after))
}

fn edge_id(event: &CdcEvent) -> String {
    format!("{}->{}", event.node_id, event.target_id)
}

/// The feed change of one hub event (a row event is one `Row` change,
/// MEDULLA-ENGINE-PARITY-R015.2).
fn change_of(event: &CdcEvent) -> Option<GraphChange> {
    match event.kind {
        CdcKind::AddNode | CdcKind::UpdateNode => Some(GraphChange::Upsert {
            node_ids: vec![event.node_id.clone()],
            edge_ids: Vec::new(),
        }),
        CdcKind::AddEdge => Some(GraphChange::Upsert {
            node_ids: Vec::new(),
            edge_ids: vec![edge_id(event)],
        }),
        CdcKind::RemoveNode => Some(GraphChange::Tombstone {
            deleted_ids: vec![event.node_id.clone()],
            deletion_revision: event.seq,
        }),
        CdcKind::RemoveEdge => Some(GraphChange::Tombstone {
            deleted_ids: vec![edge_id(event)],
            deletion_revision: event.seq,
        }),
        CdcKind::InsertRow => Some(row_change(event, RowOperation::Insert)),
        CdcKind::UpdateRow => Some(row_change(event, RowOperation::Update)),
        CdcKind::DeleteRow => Some(row_change(event, RowOperation::Delete)),
    }
}

fn row_change(event: &CdcEvent, operation: RowOperation) -> GraphChange {
    GraphChange::Row {
        table: event.table.clone(),
        operation,
        key: event.node_id.clone(),
    }
}

fn is_row_event(event: &CdcEvent) -> bool {
    matches!(
        event.kind,
        CdcKind::InsertRow | CdcKind::UpdateRow | CdcKind::DeleteRow
    )
}

/// Whether the caller may read the committed rows of `table`: the token carries
/// `cdc:read` and the table's ACL grants the caller SELECT
/// (MEDULLA-ENGINE-PARITY-R015.2).
fn row_access_of(authority: &GraphReadAuthority, persist_dir: Option<&str>, table: &str) -> bool {
    let Some(carrier) = authority.carrier() else {
        return false;
    };
    if !carrier
        .scopes()
        .iter()
        .any(|scope| scope == "*" || scope == "cdc:read")
    {
        return false;
    }
    #[cfg(feature = "query")]
    {
        persist_dir.is_some_and(|dir| {
            medulla_server_auth::sql_catalog_acl::may_stream_table(
                std::path::Path::new(dir),
                carrier,
                table,
            )
        })
    }
    #[cfg(not(feature = "query"))]
    {
        let _ = (persist_dir, table);
        false
    }
}

/// One `ReadGraphChanges` request after decoding.
pub(super) struct ReadRequest<'a> {
    pub(super) cursor: &'a str,
    pub(super) limit: u32,
    /// Only the row changes of this table; `None` reads graph changes and every
    /// readable table's row changes.
    pub(super) table: Option<&'a str>,
    /// Whether the caller may read the committed rows of a table.
    pub(super) row_access: &'a dyn Fn(&str) -> bool,
}

/// One page of changes after `cursor`. Refusal order: a cursor that does not
/// decode or names another graph, the caller's authorization, the feed
/// incarnation, then the retained window.
pub(super) fn read_changes(
    hub: &CdcHub,
    caller: &FeedCaller,
    authority: &GraphReadAuthority,
    graph: &str,
    request: ReadRequest<'_>,
) -> Result<GraphChangePage, FeedRefusal> {
    let ReadRequest {
        cursor,
        limit,
        table,
        row_access,
    } = request;
    let max_page = knobs().graph_change_feed_max_page;
    if limit == 0 || u64::from(limit) > max_page {
        return Err(FeedRefusal::InvalidArgument(format!(
            "limit must be between 1 and {max_page}"
        )));
    }
    let bound =
        GraphChangeCursor::decode(&caller.tenant, cursor).map_err(FeedRefusal::InvalidArgument)?;
    if bound.graph != graph {
        return Err(FeedRefusal::InvalidArgument(
            "the cursor names another graph".to_string(),
        ));
    }
    if bound.authorization_revision != caller.authorization_revision {
        return Err(FeedRefusal::AuthorizationChanged);
    }
    let position = hub.feed_position(graph);
    if bound.incarnation != position.incarnation {
        return Err(FeedRefusal::IncarnationStale {
            safe_cursor: cursor_at(caller, graph, position.incarnation, position.head_seq),
        });
    }
    let read = hub.read(graph, bound.last_event, limit);
    if read.gap {
        return Err(FeedRefusal::CursorExpired {
            safe_cursor: cursor_at(caller, graph, position.incarnation, read.watermark),
        });
    }
    let visible_at_ms = medulla_server_api::txn::now_ms();
    let mut next = bound.last_event;
    let mut events = Vec::new();
    for event in &read.events {
        next = event.seq + 1;
        let permitted = if is_row_event(event) {
            table.is_none_or(|wanted| wanted == event.table) && row_access(&event.table)
        } else {
            table.is_none() && visible_to(authority, event)
        };
        if !permitted {
            continue;
        }
        let Some(change) = change_of(event) else {
            continue;
        };
        events.push(GraphChangeEvent {
            schema_version: GRAPH_CHANGE_FEED_SCHEMA_VERSION,
            commit_id: format!("{}:{}", bound.incarnation, event.seq),
            event_ordinal: event.seq,
            committed_at_ms: event.committed_at_ms,
            visible_at_ms,
            latency_profile_revision: LATENCY_PROFILE_REVISION,
            commit_to_visible_ms: visible_at_ms.saturating_sub(event.committed_at_ms),
            change,
        });
    }
    Ok(GraphChangePage {
        schema_version: GRAPH_CHANGE_FEED_SCHEMA_VERSION,
        events,
        next_cursor: cursor_at(caller, graph, position.incarnation, next),
    })
}
