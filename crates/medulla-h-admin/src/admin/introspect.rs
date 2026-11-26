//! The served runtime introspection read (MEDULLA-OBSERVE-R006.1).
//!
//! `Method::Introspect` answers one page of one section of the process's runtime registries,
//! ascending by the section's key. The `tasks` section reads the task registry (key: the task
//! id) and the `readiness` section reads the readiness registry (key: the graph name). The
//! `requests` section reads the in-flight request registry (key: the registry key), the
//! `locks` section the lock sites (key: the lock name) and the `runtime` section the runtime
//! sampler (keys `global`, then `worker:<index>`). The `events` section reads the transition
//! event ring (key: the event's sequence number). Pure over
//! `medulla_observe`: no durable substrate is read, so the method serves in every build.

use medulla_observe::registry::tasks::{file_line, ProgressCursor, TaskRow};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::introspect::{IntrospectPage, IntrospectSection};
use serde_json::{json, Value};

/// The default page size.
const DEFAULT_PAGE_LIMIT: usize = 100;
/// The largest page.
const MAX_PAGE_LIMIT: u16 = 500;
/// A cursor is a section key: at most 256 characters.
const MAX_CURSOR_CHARS: usize = 256;

/// Answer one `Method::Introspect` request.
///
/// Refusals: `INTROSPECT_LIMIT_INVALID` for a limit of 0 or over 500 and
/// `INTROSPECT_CURSOR_INVALID` for a cursor that is not a key of the section.
pub async fn handle(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    req_id: u64,
    method: Method,
) -> Response {
    let Method::Introspect {
        section,
        limit,
        cursor,
    } = method
    else {
        return Response::err(
            req_id,
            "INTERNAL: introspect handler received a foreign method",
        );
    };
    let limit = match limit {
        Some(value) if (1..=MAX_PAGE_LIMIT).contains(&value) => value as usize,
        Some(value) => {
            return Response::err(
                req_id,
                format!("INTROSPECT_LIMIT_INVALID: limit {value} is outside 1..={MAX_PAGE_LIMIT}"),
            );
        }
        None => DEFAULT_PAGE_LIMIT,
    };
    if let Some(cursor) = cursor.as_deref() {
        if cursor.is_empty() || cursor.chars().count() > MAX_CURSOR_CHARS {
            return Response::err(
                req_id,
                format!(
                    "INTROSPECT_CURSOR_INVALID: the cursor must be 1..={MAX_CURSOR_CHARS} characters"
                ),
            );
        }
    }
    let text_space = bound_text_space(state).await;
    match read_section(section, limit, cursor.as_deref(), text_space) {
        Ok(page) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::security::Introspect>(
                page,
            ),
        ),
        Err(refusal) => Response::err(req_id, refusal),
    }
}

fn read_section(
    section: IntrospectSection,
    limit: usize,
    cursor: Option<&str>,
    text_space: Option<(String, usize)>,
) -> Result<IntrospectPage, String> {
    match section {
        IntrospectSection::Tasks => tasks_page(limit, cursor),
        IntrospectSection::Readiness => Ok(readiness_page(limit, cursor)),
        IntrospectSection::Build => Ok(build_page(cursor)),
        IntrospectSection::Indexes => Ok(indexes_page(cursor, text_space)),
        IntrospectSection::Startup => Ok(startup_page(limit, cursor)),
        IntrospectSection::Requests => requests_page(limit, cursor),
        IntrospectSection::Locks => Ok(locks_page(limit, cursor)),
        IntrospectSection::Runtime => Ok(runtime_page(limit, cursor)),
        IntrospectSection::Events => events_page(limit, cursor),
    }
}

/// Take at most `limit` rows after `cursor` from rows already ascending by key, and the
/// cursor of the next page when rows remain.
fn page_of<T>(
    rows: Vec<T>,
    limit: usize,
    after: impl Fn(&T) -> bool,
    key: impl Fn(&T) -> String,
    render: impl Fn(&T) -> Value,
) -> (Vec<Value>, Option<String>) {
    let remaining: Vec<T> = rows.into_iter().filter(|row| after(row)).collect();
    let more = remaining.len() > limit;
    let page: Vec<&T> = remaining.iter().take(limit).collect();
    let next = if more {
        page.last().map(|row| key(row))
    } else {
        None
    };
    (page.into_iter().map(render).collect(), next)
}

fn tasks_page(limit: usize, cursor: Option<&str>) -> Result<IntrospectPage, String> {
    let after_id: Option<u64> = match cursor {
        Some(value) => Some(value.parse().map_err(|_| {
            "INTROSPECT_CURSOR_INVALID: a tasks cursor is a task id".to_string()
        })?),
        None => None,
    };
    let registry = medulla_observe::global();
    let seq = registry.sequence();
    let (rows, next_cursor) = page_of(
        registry.all(),
        limit,
        |row: &TaskRow| after_id.map_or(true, |id| row.id > id),
        |row| row.id.to_string(),
        task_json,
    );
    Ok(IntrospectPage {
        section: IntrospectSection::Tasks,
        rows,
        next_cursor,
        seq,
    })
}

fn task_json(row: &TaskRow) -> Value {
    let cursor = row.cursor.as_ref().map(|cursor| match cursor {
        ProgressCursor::Keyset { node, edge } => json!({
            "kind": "keyset",
            "node": node,
            "edge": edge,
        }),
        ProgressCursor::Counter(value) => json!({ "kind": "counter", "value": value }),
        ProgressCursor::Opaque(value) => json!({ "kind": "opaque", "value": value }),
    });
    json!({
        "id": row.id,
        "name": row.name,
        "owner": row.owner,
        "kind": format!("{:?}", row.kind),
        "state": medulla_types::transition_refused::MachineState::stored(row.state),
        "failure": row.failure.as_ref().map(|failure| failure.code()),
        "cursor": cursor,
        "last_progress_unix_ms": row.last_progress_unix_ms,
        "last_progress_location": row.last_progress_location.map(file_line),
        "registered_location": file_line(row.registered_location),
        "units_done": row.units_done,
        "origin_request": row.origin_request,
    })
}

fn readiness_page(limit: usize, cursor: Option<&str>) -> IntrospectPage {
    let registry = medulla_observe::readiness();
    let seq = registry.sequence();
    let (rows, next_cursor) = page_of(
        registry.snapshot(),
        limit,
        |row| cursor.map_or(true, |last| row.graph.as_str() > last),
        |row| row.graph.clone(),
        |row| {
            json!({
                "graph": row.graph,
                "state": medulla_types::transition_refused::MachineState::stored(row.state),
                "reason_code": row.reason.as_ref().map(|reason| reason.code.as_str()),
                "reason_task": row.reason.as_ref().and_then(|reason| reason.task.as_deref()),
                "reason_detail": row.reason.as_ref().map(|reason| reason.detail.as_str()),
                "since_unix_ms": row.since_unix_ms,
            })
        },
    );
    IntrospectPage {
        section: IntrospectSection::Readiness,
        rows,
        next_cursor,
        seq,
    }
}

/// The `build` section: one row, key `build`, the identity of the running binary
/// (MEDULLA-OBSERVE-R009.1).
fn build_page(cursor: Option<&str>) -> IntrospectPage {
    let info = &*medulla_version::BUILD_INFO;
    // The only key is `build`: a cursor at or past it leaves nothing to read.
    let rows = if cursor.is_some_and(|last| last >= "build") {
        Vec::new()
    } else {
        vec![json!({
            "key": "build",
            "version": info.version,
            "git_sha": info.git_sha,
            "build_id": info.build_id,
            "profile": info.profile,
            "features": info.features,
        })]
    };
    IntrospectPage {
        section: IntrospectSection::Build,
        rows,
        next_cursor: None,
        seq: 0,
    }
}

/// The `indexes` section: one row per engine-maintained semantic index, key ascending. The
/// bookmark index needs a bound text embedder; with none, its row says so
/// (MEDULLA-MEDIA-VOCAB-R001.102.2), so authoring nothing is never silent.
fn indexes_page(cursor: Option<&str>, text_space: Option<(String, usize)>) -> IntrospectPage {
    const KEY: &str = "bookmark_index";
    let rows = if cursor.is_some_and(|last| last >= KEY) {
        Vec::new()
    } else {
        let row = match text_space {
            Some((space_id, dimensions)) => json!({
                "key": KEY,
                "state": "bound",
                "report": format!("{KEY}: bound ({space_id})"),
                "space_id": space_id,
                "dimensions": dimensions,
            }),
            None => json!({
                "key": KEY,
                "state": "unbound",
                "report": format!("{KEY}: unbound (no text embedder)"),
                "space_id": null,
                "dimensions": null,
            }),
        };
        vec![row]
    };
    IntrospectPage {
        section: IntrospectSection::Indexes,
        rows,
        next_cursor: None,
        seq: 0,
    }
}

/// The space id and width of the text embedder this state serves with, or `None` when none is
/// bound. It resolves through the same identity as binding authoring: the state's injected
/// embedder first, then the process-wide one (MEDULLA-MEDIA-VOCAB-R001.102.2).
#[cfg(feature = "query")]
async fn bound_text_space(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
) -> Option<(String, usize)> {
    let state_embedder = state.read().await.text_embedder_override.clone();
    medulla_server_ops::api::query_txn::query_support::state_text_encoder_identity(
        state_embedder.as_ref(),
    )
    .map(|identity| (identity.space_id, identity.dimensions))
}

/// Without the `query` feature no text embedder can be bound.
#[cfg(not(feature = "query"))]
async fn bound_text_space(
    _state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
) -> Option<(String, usize)> {
    None
}

/// The `startup` section: the recorded boot steps. The key is `<started_unix_ms>:<name>:<graph>`
/// with the start time zero-padded, so the keys ascend with the rows.
fn startup_page(limit: usize, cursor: Option<&str>) -> IntrospectPage {
    let key = |step: &medulla_observe::startup::StartupStep| {
        format!("{:020}:{}:{}", step.started_unix_ms, step.name, step.graph)
    };
    let (rows, next_cursor) = page_of(
        medulla_observe::startup().steps(),
        limit,
        |step| cursor.map_or(true, |last| key(step).as_str() > last),
        key,
        |step| {
            json!({
                "name": step.name,
                "graph": step.graph,
                "started_unix_ms": step.started_unix_ms,
                "duration_ms": step.duration_ms,
                "outcome": step.outcome.word(),
                "detail": step.outcome.detail(),
            })
        },
    );
    IntrospectPage {
        section: IntrospectSection::Startup,
        rows,
        next_cursor,
        seq: 0,
    }
}

/// The `requests` section: the in-flight requests, key the registry key (MEDULLA-OBSERVE-R006.2).
fn requests_page(limit: usize, cursor: Option<&str>) -> Result<IntrospectPage, String> {
    let after: Option<u64> = match cursor {
        Some(value) => Some(value.parse().map_err(|_| {
            "INTROSPECT_CURSOR_INVALID: a requests cursor is a request key".to_string()
        })?),
        None => None,
    };
    let registry = medulla_observe::requests();
    let seq = registry.sequence();
    let (rows, next_cursor) = page_of(
        registry.snapshot(),
        limit,
        |row: &medulla_observe::registry::requests::RequestRow| {
            after.map_or(true, |key| row.key > key)
        },
        |row| row.key.to_string(),
        |row| {
            json!({
                "key": row.key,
                "request_id": row.request_id,
                "trace_id": row.trace_id,
                "method": row.method,
                "graph": row.graph,
                "principal_ref": row.principal_ref,
                "phase": medulla_types::transition_refused::MachineState::stored(row.phase),
                "age_ms": row.age_ms,
                "waiting_for": row.waiting_for,
            })
        },
    );
    Ok(IntrospectPage {
        section: IntrospectSection::Requests,
        rows,
        next_cursor,
        seq,
    })
}

/// The `locks` section: the named lock sites, key the lock name (MEDULLA-OBSERVE-R006.4).
fn locks_page(limit: usize, cursor: Option<&str>) -> IntrospectPage {
    let (rows, next_cursor) = page_of(
        medulla_observe::locks().snapshot(),
        limit,
        |row| cursor.map_or(true, |last| row.name.as_str() > last),
        |row| row.name.clone(),
        |row| {
            json!({
                "name": row.name,
                "holder": row.holder,
                "hold_ms": row.hold_ms,
                "waiters": row.waiters,
                "longest_wait_ms": row.longest_wait_ms,
            })
        },
    );
    IntrospectPage {
        section: IntrospectSection::Locks,
        rows,
        next_cursor,
        seq: 0,
    }
}

/// The `runtime` section: the `global` row, then one `worker:<index>` row per tokio worker
/// (MEDULLA-OBSERVE-R006.3). Outside a runtime the page is empty.
fn runtime_page(limit: usize, cursor: Option<&str>) -> IntrospectPage {
    let snapshot = tokio::runtime::Handle::try_current()
        .ok()
        .map(|handle| medulla_observe::runtime_sampler().snapshot(&handle));
    let mut keyed: Vec<(String, Value)> = Vec::new();
    if let Some(snapshot) = snapshot {
        keyed.push((
            "global".to_string(),
            json!({
                "key": "global",
                "global_queue_depth": snapshot.global_queue_depth,
                "workers": snapshot.workers.len(),
            }),
        ));
        for worker in snapshot.workers {
            let key = format!("worker:{:04}", worker.index);
            keyed.push((
                key.clone(),
                json!({
                    "key": key,
                    "index": worker.index,
                    "busy_ms": worker.busy_ms,
                    "busy_ratio": worker.busy_ratio,
                    "parks": worker.parks,
                    "spinning": worker.spinning,
                }),
            ));
        }
    }
    let (rows, next_cursor) = page_of(
        keyed,
        limit,
        |(key, _)| cursor.map_or(true, |last| key.as_str() > last),
        |(key, _)| key.clone(),
        |(_, row)| row.clone(),
    );
    IntrospectPage {
        section: IntrospectSection::Runtime,
        rows,
        next_cursor,
        seq: 0,
    }
}

/// The `events` section: the accepted transitions the ring still holds, key the event's sequence
/// number (MEDULLA-OBSERVE-R004.4).
fn events_page(limit: usize, cursor: Option<&str>) -> Result<IntrospectPage, String> {
    let after: Option<u64> = match cursor {
        Some(value) => Some(value.parse().map_err(|_| {
            "INTROSPECT_CURSOR_INVALID: an events cursor is an event sequence number".to_string()
        })?),
        None => None,
    };
    let ring = medulla_observe::events();
    let seq = ring.sequence();
    let (rows, next_cursor) = page_of(
        ring.snapshot(),
        limit,
        |(key, _)| after.map_or(true, |last| *key > last),
        |(key, _)| key.to_string(),
        |(key, event)| {
            json!({
                "seq": key,
                "machine": event.machine,
                "subject": event.subject,
                "from": event.from,
                "event": event.event,
                "to": event.to,
                "at_unix_ms": event.at_unix_ms,
                "trace_id": event.trace_id,
            })
        },
    );
    Ok(IntrospectPage {
        section: IntrospectSection::Events,
        rows,
        next_cursor,
        seq,
    })
}
