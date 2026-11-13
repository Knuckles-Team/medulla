//! Live artifacts (MEDULLA-FEDERATED-QUERY-R701.3, R701.5): `LiveArtifactPut`,
//! `LiveArtifactGet` and `LiveArtifactRefresh`.
//!
//! An artifact is one node of label `LiveArtifact` (`live_artifact:<id>`) that holds the
//! artifact document, its owner and a refresh counter. Each refresh attempt writes a
//! `LiveArtifactRefresh` node linked to the artifact by a `REFRESHED` edge. The engine keeps
//! the latest `artifact.refresh_history` of them. Put and Refresh run over the staged graph
//! of the query gateway, so the artifact node and the refresh record commit together or not
//! at all. A failed run writes the same record with `error` set and leaves `data` and
//! `version` unchanged.

use medulla_server_api::knob_config::knobs;
use medulla_source_types::live_artifact::{
    LiveArtifact, LiveArtifactRefusal, LiveArtifactView, QueryLanguage, QuerySource, RefreshEntry,
    LIVE_ARTIFACT_INVALID, LIVE_ARTIFACT_NOT_FOUND,
};
use medulla_types::wire::{UqlResult, UqlRow};
use serde_json::{json, Map, Value};

use super::uql_statement::handle_uql;
use super::*;

const ARTIFACT_LABEL: &str = "LiveArtifact";
const REFRESH_LABEL: &str = "LiveArtifactRefresh";
const REFRESHED: &str = "REFRESHED";
const ACCESS_DENIED: &str = "ACCESS_DENIED";
const INTERNAL: &str = "INTERNAL";

/// The artifact node as stored: the document plus what the engine keeps beside it.
struct Stored {
    artifact: LiveArtifact,
    owner: String,
    refresh_seq: u64,
}

fn refuse(code: &str, detail: impl Into<String>) -> LiveArtifactRefusal {
    LiveArtifactRefusal {
        code: code.to_string(),
        detail: detail.into(),
    }
}

fn internal(error: impl std::fmt::Display) -> LiveArtifactRefusal {
    refuse(INTERNAL, error.to_string())
}

fn artifact_node(id: &str) -> String {
    format!("live_artifact:{id}")
}

fn refresh_node(id: &str, seq: u64) -> String {
    format!("live_artifact_refresh:{id}:{seq}")
}

pub(crate) fn is_live_artifact_method(method: &Method) -> bool {
    matches!(
        method,
        Method::LiveArtifactPut { .. }
            | Method::LiveArtifactGet { .. }
            | Method::LiveArtifactRefresh { .. }
    )
}

/// Route one live-artifact method to its handler.
pub(crate) async fn dispatch_live_artifact(
    ctx: &QueryHandlerCtx<'_>,
    method: Method,
) -> Result<Response, Method> {
    use medulla_protocol_types::result_contract::query as results;
    let req_id = ctx.req_id;
    match method {
        Method::LiveArtifactPut { artifact } => Ok(reply::<results::LiveArtifactPut>(
            req_id,
            handle_put(ctx, artifact),
        )),
        Method::LiveArtifactGet { id } => Ok(reply::<results::LiveArtifactGet>(
            req_id,
            handle_get(ctx, &id),
        )),
        Method::LiveArtifactRefresh {
            id,
            expected_version,
        } => Ok(reply::<results::LiveArtifactRefresh>(
            req_id,
            handle_refresh(ctx, &id, expected_version).await,
        )),
        other => Err(other),
    }
}

fn reply<M: MethodResult>(req_id: u64, out: Result<M::Body, LiveArtifactRefusal>) -> Response {
    match out
        .map_err(|refusal| refusal.to_string())
        .and_then(ResultPayload::of::<M>)
    {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

// ── Storage ───────────────────────────────────────────────────────────────────────────

fn load(core: &GraphCore, id: &str) -> Result<Stored, LiveArtifactRefusal> {
    let blob = core
        .get_node_properties(&artifact_node(id))
        .ok_or_else(|| refuse(LIVE_ARTIFACT_NOT_FOUND, format!("no live artifact `{id}`")))?;
    let props = medulla_types::msgpack::decode_property_object(&blob)
        .map_err(|_| internal("stored artifact node is not a property object"))?;
    let artifact = props
        .get("artifact")
        .cloned()
        .ok_or_else(|| internal("stored artifact node has no document"))?;
    Ok(Stored {
        artifact: serde_json::from_value(artifact).map_err(internal)?,
        owner: props
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        refresh_seq: props
            .get("refresh_seq")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn write_node(core: &GraphCore, node: String, props: &Value) -> Result<(), LiveArtifactRefusal> {
    core.add_node(node, msgpack_bytes(props).map_err(internal)?);
    Ok(())
}

fn store_artifact(core: &GraphCore, stored: &Stored) -> Result<(), LiveArtifactRefusal> {
    let props = json!({
        "type": ARTIFACT_LABEL,
        "owner": stored.owner,
        "refresh_seq": stored.refresh_seq,
        "artifact": serde_json::to_value(&stored.artifact).map_err(internal)?,
    });
    write_node(core, artifact_node(&stored.artifact.id), &props)
}

fn entry_of(props: &Map<String, Value>) -> Option<RefreshEntry> {
    let number = |key: &str| props.get(key).and_then(Value::as_u64);
    Some(RefreshEntry {
        record: medulla_source_types::live_artifact::RefreshRecord {
            started_at_ms: number("started_at")?,
            ended_at_ms: number("ended_at")?,
            row_count: number("row_count")?,
            error: props
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        new_version: number("new_version")?,
    })
}

/// Write one refresh record node, link it to its artifact and prune the history.
fn record_refresh(
    core: &GraphCore,
    stored: &mut Stored,
    entry: &RefreshEntry,
) -> Result<(), LiveArtifactRefusal> {
    stored.refresh_seq += 1;
    store_artifact(core, stored)?;
    let id = stored.artifact.id.as_str();
    let record = &entry.record;
    let props = json!({
        "type": REFRESH_LABEL,
        "artifact_id": id,
        "seq": stored.refresh_seq,
        "started_at": record.started_at_ms,
        "ended_at": record.ended_at_ms,
        "row_count": record.row_count,
        "error": record.error,
        "new_version": entry.new_version,
    });
    let node = refresh_node(id, stored.refresh_seq);
    write_node(core, node.clone(), &props)?;
    let edge = msgpack_bytes(&json!({ "relationship": REFRESHED })).map_err(internal)?;
    core.add_edge(node, artifact_node(id), edge)
        .map_err(internal)?;
    prune_history(core, id, stored.refresh_seq);
    Ok(())
}

fn history_limit() -> u64 {
    knobs().artifact_refresh_history
}

/// Drop the records older than the newest `artifact.refresh_history`. The pruned records
/// are always a contiguous prefix, so the walk stops at the first one already gone.
fn prune_history(core: &GraphCore, id: &str, newest_seq: u64) {
    let oldest_kept = newest_seq.saturating_sub(history_limit()) + 1;
    for seq in (1..oldest_kept).rev() {
        let node = refresh_node(id, seq);
        if !core.has_node(&node) {
            break;
        }
        core.remove_node(node);
    }
}

/// The retained records, newest first.
fn read_history(core: &GraphCore, stored: &Stored) -> Vec<RefreshEntry> {
    let newest = stored.refresh_seq;
    let oldest = newest.saturating_sub(history_limit()) + 1;
    (oldest..=newest)
        .rev()
        .map_while(|seq| core.get_node_properties(&refresh_node(&stored.artifact.id, seq)))
        .filter_map(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .filter_map(|props| entry_of(&props))
        .collect()
}

// ── Handlers ──────────────────────────────────────────────────────────────────────────

fn check_owner(ctx: &QueryHandlerCtx<'_>, owner: &str) -> Result<(), LiveArtifactRefusal> {
    if owner == ctx.caller {
        return Ok(());
    }
    Err(refuse(
        ACCESS_DENIED,
        "the artifact belongs to another principal",
    ))
}

fn check_language(source: &QuerySource) -> Result<(), LiveArtifactRefusal> {
    match source.language {
        QueryLanguage::Uql | QueryLanguage::Sparql => Ok(()),
        QueryLanguage::Sql => Err(refuse(
            LIVE_ARTIFACT_INVALID,
            "source language must be uql or sparql",
        )),
    }
}

/// `LiveArtifactPut`: validate, then create or replace the artifact node. A replacement
/// keeps the owner and the refresh counter of the artifact it replaces.
fn handle_put(
    ctx: &QueryHandlerCtx<'_>,
    artifact: LiveArtifact,
) -> Result<LiveArtifact, LiveArtifactRefusal> {
    artifact.validate()?;
    check_language(&artifact.source)?;
    let stored = match load(ctx.core, &artifact.id) {
        Ok(prior) => {
            check_owner(ctx, &prior.owner)?;
            Stored { artifact, ..prior }
        }
        Err(_) => Stored {
            artifact,
            owner: ctx.caller.to_string(),
            refresh_seq: 0,
        },
    };
    store_artifact(ctx.core, &stored)?;
    Ok(stored.artifact)
}

/// `LiveArtifactGet`: the artifact and its retained refresh records, over the caller's
/// row-security view.
fn handle_get(
    ctx: &QueryHandlerCtx<'_>,
    id: &str,
) -> Result<LiveArtifactView, LiveArtifactRefusal> {
    let visible = ctx.read_authority.map_or_else(
        || Arc::clone(ctx.core),
        |authority| authority.project_core(ctx.core),
    );
    let stored = load(&visible, id)?;
    let refreshes = read_history(&visible, &stored);
    Ok(LiveArtifactView {
        artifact: stored.artifact,
        refreshes,
    })
}

/// `LiveArtifactRefresh`: re-run the stored query, apply the outcome by compare-and-set on
/// `version` and record the attempt. This is the function the scheduled refresh calls.
pub(crate) async fn handle_refresh(
    ctx: &QueryHandlerCtx<'_>,
    id: &str,
    expected_version: u64,
) -> Result<RefreshEntry, LiveArtifactRefusal> {
    let mut stored = load(ctx.core, id)?;
    check_owner(ctx, &stored.owner)?;
    let started = now_ms();
    let outcome = run_source(ctx, &stored.artifact.source).await;
    let record = stored
        .artifact
        .refresh(expected_version, outcome, started, now_ms())?;
    let entry = RefreshEntry {
        record,
        new_version: stored.artifact.version,
    };
    record_refresh(ctx.core, &mut stored, &entry)?;
    Ok(entry)
}

fn now_ms() -> u64 {
    medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
}

// ── Running the stored query ──────────────────────────────────────────────────────────

async fn run_source(ctx: &QueryHandlerCtx<'_>, source: &QuerySource) -> Result<Value, String> {
    match source.language {
        QueryLanguage::Uql => run_uql(ctx, &source.text).await,
        QueryLanguage::Sparql => run_sparql(ctx, &source.text).await,
        QueryLanguage::Sql => Err(format!("{LIVE_ARTIFACT_INVALID}: sql is not refreshable")),
    }
}

async fn run_uql(ctx: &QueryHandlerCtx<'_>, text: &str) -> Result<Value, String> {
    let response = handle_uql(
        ctx,
        text.to_string(),
        UqlParams::new(),
        false,
        Default::default(),
    )
    .await
    .map_err(|_| "UQL is not served in this build".to_string())?;
    match response_body::<UqlResult>(response)? {
        UqlResult::Rows { columns, rows, .. } => Ok(Value::Array(
            rows.into_iter()
                .map(|row| row_json(&columns, row))
                .collect(),
        )),
        _ => Err("the stored UQL statement does not return rows".to_string()),
    }
}

pub(super) fn row_json(columns: &[String], row: UqlRow) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), Value::String(row.id));
    object.insert("score".into(), json!(row.score));
    for (column, value) in columns.iter().zip(row.channels) {
        object.insert(column.clone(), json!(value));
    }
    Value::Object(object)
}

#[cfg(feature = "sparql")]
async fn run_sparql(ctx: &QueryHandlerCtx<'_>, text: &str) -> Result<Value, String> {
    response_body::<Value>(sparql_response(ctx, text).await?)
}

/// Run one SPARQL text through the served SPARQL handler and hand back its response.
#[cfg(feature = "sparql")]
pub(super) async fn sparql_response(
    ctx: &QueryHandlerCtx<'_>,
    text: &str,
) -> Result<Response, String> {
    use medulla_server_api::api::reentry::TryHandleContext;
    let method = Method::Sparql {
        query: text.to_string(),
        base_iri: String::new(),
        type_convention: String::new(),
        explain: false,
        requires_packs: Vec::new(),
    };
    let handler_ctx = TryHandleContext {
        req_id: ctx.req_id,
        graph_name: ctx.graph_name,
        read_authority: ctx.read_authority,
        caller: ctx.caller,
    };
    medulla_h_sparql::rdf::try_handle(
        ctx.state,
        handler_ctx,
        Arc::clone(ctx.core),
        method,
        #[cfg(feature = "security")]
        ctx.rls,
    )
    .await
    .map_err(|_| "SPARQL is not served in this build".to_string())
}

#[cfg(not(feature = "sparql"))]
async fn run_sparql(_ctx: &QueryHandlerCtx<'_>, _text: &str) -> Result<Value, String> {
    Err("SPARQL is not served in this build".to_string())
}

/// Decode the body of a handler response, or its refusal as text.
pub(super) fn response_body<T: serde::de::DeserializeOwned>(
    response: Response,
) -> Result<T, String> {
    if let Some(error) = response.error {
        return Err(match response.error_detail {
            Some(detail) => format!("{error}: {detail}"),
            None => error,
        });
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).map_err(|e| e.to_string()),
        Some(ResultPayload::Json(value)) => {
            serde_json::from_value(value).map_err(|e| e.to_string())
        }
        _ => Err("the query returned no decodable result".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::contract::ServerErrorCode;

    // spec: MEDULLA-FEDERATED-QUERY-R701.3
    #[test]
    fn refusal_codes_are_registered_server_error_codes() {
        use medulla_source_types::live_artifact as codes;
        const CASES: [(&str, ServerErrorCode); 6] = [
            (
                codes::LIVE_ARTIFACT_INVALID,
                ServerErrorCode::LiveArtifactInvalid,
            ),
            (
                codes::LIVE_ARTIFACT_TEMPLATE_UNSAFE,
                ServerErrorCode::LiveArtifactTemplateUnsafe,
            ),
            (
                codes::LIVE_ARTIFACT_DEPTH_EXCEEDED,
                ServerErrorCode::LiveArtifactDepthExceeded,
            ),
            (
                codes::LIVE_ARTIFACT_SIZE_EXCEEDED,
                ServerErrorCode::LiveArtifactSizeExceeded,
            ),
            (
                codes::LIVE_ARTIFACT_VERSION_CONFLICT,
                ServerErrorCode::LiveArtifactVersionConflict,
            ),
            (
                codes::LIVE_ARTIFACT_NOT_FOUND,
                ServerErrorCode::LiveArtifactNotFound,
            ),
        ];
        for (wire, registered) in CASES {
            assert_eq!(wire, registered.as_str());
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R701.5
    #[test]
    fn history_is_pruned_to_the_newest_records_and_read_newest_first() {
        let core = GraphCore::new();
        let mut stored = Stored {
            artifact: LiveArtifact {
                id: "a".into(),
                version: 1,
                template: String::new(),
                source: QuerySource {
                    language: QueryLanguage::Uql,
                    text: "MATCH (n) RETURN n".into(),
                },
                data: Value::Null,
                provenance: Default::default(),
            },
            owner: "owner".into(),
            refresh_seq: 0,
        };
        store_artifact(&core, &stored).unwrap();
        let keep = history_limit();
        for n in 0..keep + 3 {
            let record = medulla_source_types::live_artifact::RefreshRecord {
                started_at_ms: n,
                ended_at_ms: n + 1,
                row_count: n,
                error: (n % 2 == 1).then(|| "boom".to_string()),
            };
            let entry = RefreshEntry {
                record,
                new_version: 1,
            };
            record_refresh(&core, &mut stored, &entry).unwrap();
        }
        let history = read_history(&core, &stored);
        assert_eq!(history.len() as u64, keep);
        assert_eq!(history[0].record.started_at_ms, keep + 2);
        assert!(!core.has_node(&refresh_node("a", 3)));
        assert!(core.has_node(&refresh_node("a", 4)));
        assert_eq!(history[0].record.error.is_some(), (keep + 2) % 2 == 1);
    }
}
