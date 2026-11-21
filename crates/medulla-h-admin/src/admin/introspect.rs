//! The served runtime introspection read (MEDULLA-OBSERVE-R006.1).
//!
//! `Method::Introspect` answers one page of one section of the process's runtime registries,
//! ascending by the section's key. The `tasks` section reads the task registry (key: the task
//! id) and the `readiness` section reads the readiness registry (key: the graph name); the
//! sections whose registries land with their own rows (`requests`, `runtime`, `locks`,
//! `startup`, `events`) answer an empty page until then. Pure over
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
pub fn handle(req_id: u64, method: Method) -> Response {
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
    match read_section(section, limit, cursor.as_deref()) {
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
) -> Result<IntrospectPage, String> {
    match section {
        IntrospectSection::Tasks => tasks_page(limit, cursor),
        IntrospectSection::Readiness => Ok(readiness_page(limit, cursor)),
        IntrospectSection::Build => Ok(build_page(cursor)),
        other => Ok(IntrospectPage {
            section: other,
            rows: Vec::new(),
            next_cursor: None,
            seq: 0,
        }),
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
