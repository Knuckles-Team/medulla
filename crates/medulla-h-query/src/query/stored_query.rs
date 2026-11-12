//! Stored, versioned queries (MEDULLA-FEDERATED-QUERY-R708.2 to R708.8): `StoredQueryPut`,
//! `StoredQueryCall` and the UQL form `CALL STORED name(args)`.
//!
//! One immutable version is one node of label `StoredQuery` (`stored_query:<name>:<version>`).
//! A head node (`stored_query_head:<name>`) names the author and the highest published
//! version. A materialized result is one `StoredQueryResult` node per (version, binding)
//! (`stored_query_result:<name>:<version>:<args digest>`). Put and Call run over the staged
//! graph of the query gateway, so a record and its refreshed result commit together or not at
//! all. UQL stored queries run through `handle_uql`, the one served UQL path, so typed
//! parameter binding and the dependency-keyed result cache are the ones every UQL read uses.

use std::collections::BTreeMap;

use medulla_identity_types::identity::UnitPrincipal;
use medulla_server_api::read_scope::ReadCaller;
use medulla_source_types::stored_query::{
    args_digest, needs_run, refuse, serve_materialized, staleness_s, Served, StoredCall,
    StoredParamType, StoredQuery, StoredQueryLanguage, StoredQueryRefusal, StoredResult,
    ACCESS_DENIED, STORED_QUERY_IMMUTABLE, STORED_QUERY_INVALID, STORED_QUERY_NOT_FOUND,
};
use medulla_types::wire::{UqlParam, UqlResult, UqlRow};
use serde_json::{json, Value};

use super::live_artifact::{response_body, row_json};
use super::uql_statement::handle_uql;
use super::*;

const QUERY_LABEL: &str = "StoredQuery";
const HEAD_LABEL: &str = "StoredQueryHead";
const RESULT_LABEL: &str = "StoredQueryResult";
const INTERNAL: &str = "INTERNAL";

fn query_node(name: &str, version: u32) -> String {
    format!("stored_query:{name}:{version}")
}

fn head_node(name: &str) -> String {
    format!("stored_query_head:{name}")
}

fn result_node(name: &str, version: u32, digest: &str) -> String {
    format!("stored_query_result:{name}:{version}:{digest}")
}

fn internal(error: impl std::fmt::Display) -> StoredQueryRefusal {
    refuse(INTERNAL, error.to_string())
}

pub(crate) fn is_stored_query_method(method: &Method) -> bool {
    matches!(
        method,
        Method::StoredQueryPut { .. } | Method::StoredQueryCall { .. }
    )
}

/// Route one stored-query method to its handler.
pub(crate) async fn dispatch_stored_query(
    ctx: &QueryHandlerCtx<'_>,
    method: Method,
) -> Result<Response, Method> {
    use medulla_protocol_types::result_contract::query as results;
    match method {
        Method::StoredQueryPut { query } => Ok(reply::<results::StoredQueryPut>(
            ctx.req_id,
            handle_stored_put(ctx, query),
        )),
        Method::StoredQueryCall {
            name,
            version,
            args,
        } => Ok(reply::<results::StoredQueryCall>(
            ctx.req_id,
            handle_stored_call(ctx, &name, version, &args).await,
        )),
        other => Err(other),
    }
}

fn reply<M: MethodResult>(req_id: u64, out: Result<M::Body, StoredQueryRefusal>) -> Response {
    match out
        .map_err(|refusal| refusal.to_string())
        .and_then(ResultPayload::of::<M>)
    {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

// ── Storage ───────────────────────────────────────────────────────────────────────────

fn props_of(core: &GraphCore, node: &str) -> Option<serde_json::Map<String, Value>> {
    let blob = core.get_node_properties(node)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

fn write_node(core: &GraphCore, node: String, props: &Value) -> Result<(), StoredQueryRefusal> {
    core.add_node(node, msgpack_bytes(props).map_err(internal)?);
    Ok(())
}

/// The head of one name: its author and its highest published version.
struct Head {
    author: String,
    latest: u32,
}

fn load_head(core: &GraphCore, name: &str) -> Option<Head> {
    let props = props_of(core, &head_node(name))?;
    Some(Head {
        author: props.get("author")?.as_str()?.to_string(),
        latest: u32::try_from(props.get("latest")?.as_u64()?).ok()?,
    })
}

/// One stored version and the tenant its author published in.
struct Stored {
    query: StoredQuery,
    author_tenant: String,
}

fn load_query(core: &GraphCore, name: &str, version: u32) -> Option<Stored> {
    let props = props_of(core, &query_node(name, version))?;
    Some(Stored {
        query: serde_json::from_value(props.get("query")?.clone()).ok()?,
        author_tenant: props
            .get("author_tenant")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn load_result(core: &GraphCore, name: &str, version: u32, digest: &str) -> Option<StoredResult> {
    let props = props_of(core, &result_node(name, version, digest))?;
    serde_json::from_value(props.get("result")?.clone()).ok()
}

fn now_ms() -> u64 {
    medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
}

fn reader_of(ctx: &QueryHandlerCtx<'_>) -> UnitPrincipal {
    let mut principal = ReadCaller::from_authority(ctx.read_authority, ctx.graph_name).principal;
    principal.id = ctx.caller.to_string();
    principal
}

// ── Put ───────────────────────────────────────────────────────────────────────────────

/// Declared parameter types as placeholder bindings, so a put parses the text once.
fn placeholder_params(query: &StoredQuery) -> BTreeMap<String, UqlParam> {
    query
        .params
        .iter()
        .map(|(name, declared)| {
            let value = match declared {
                StoredParamType::String => UqlParam::Str(String::new()),
                StoredParamType::Int | StoredParamType::Float => UqlParam::Num(0.0),
                StoredParamType::Bool => UqlParam::Bool(false),
            };
            (name.clone(), value)
        })
        .collect()
}

/// `StoredQueryPut`: validate, then publish one immutable version. The author is the
/// verified caller; the first author of a name owns every later version.
pub(crate) fn handle_stored_put(
    ctx: &QueryHandlerCtx<'_>,
    mut query: StoredQuery,
) -> Result<StoredQuery, StoredQueryRefusal> {
    query.validate()?;
    match query.language {
        StoredQueryLanguage::Uql => {
            medulla_uql::parse_statement(&query.text, &placeholder_params(&query))
                .map_err(|error| refuse(STORED_QUERY_INVALID, error.render(&query.text)))?;
        }
        StoredQueryLanguage::Sql | StoredQueryLanguage::Sparql => {
            return Err(refuse(
                STORED_QUERY_INVALID,
                "only uql stored queries are callable until sql and sparql are served",
            ))
        }
    }
    let head = load_head(ctx.core, &query.name);
    if let Some(head) = &head {
        if head.author != ctx.caller {
            return Err(refuse(
                ACCESS_DENIED,
                "the stored query belongs to another principal",
            ));
        }
    }
    if ctx.core.has_node(&query_node(&query.name, query.version)) {
        return Err(refuse(
            STORED_QUERY_IMMUTABLE,
            format!(
                "version {} of `{}` is published and cannot change",
                query.version, query.name
            ),
        ));
    }
    query.author = ctx.caller.to_string();
    let latest = head.map_or(0, |head| head.latest).max(query.version);
    let props = json!({
        "type": QUERY_LABEL,
        "name": query.name,
        "version": query.version,
        "author": query.author,
        "author_tenant": reader_of(ctx).tenant,
        "query": serde_json::to_value(&query).map_err(internal)?,
    });
    write_node(ctx.core, query_node(&query.name, query.version), &props)?;
    let head = json!({
        "type": HEAD_LABEL,
        "name": query.name,
        "author": query.author,
        "latest": latest,
    });
    write_node(ctx.core, head_node(&query.name), &head)?;
    Ok(query)
}

// ── Call ──────────────────────────────────────────────────────────────────────────────

fn uql_params(bound: &BTreeMap<String, Value>) -> BTreeMap<String, UqlParam> {
    bound
        .iter()
        .filter_map(|(name, value)| {
            let param = match value {
                Value::String(text) => UqlParam::Str(text.clone()),
                Value::Bool(flag) => UqlParam::Bool(*flag),
                Value::Number(number) => UqlParam::Num(number.as_f64()?),
                _ => return None,
            };
            Some((name.clone(), param))
        })
        .collect()
}

/// Run one UQL text through the served UQL path: the columns, the rows as objects and the
/// snapshot revision they were read at.
async fn run_uql(
    ctx: &QueryHandlerCtx<'_>,
    text: &str,
    bound: &BTreeMap<String, Value>,
) -> Result<(Vec<String>, Value, u64), String> {
    let response = handle_uql(ctx, text.to_string(), uql_params(bound), false, None)
        .await
        .map_err(|_| "UQL is not served in this build".to_string())?;
    match response_body::<UqlResult>(response)? {
        UqlResult::Rows {
            columns,
            rows,
            snapshot_revision,
            ..
        } => {
            let objects = rows
                .into_iter()
                .map(|row| row_json(&columns, row))
                .collect();
            Ok((columns, Value::Array(objects), snapshot_revision))
        }
        _ => Err("the stored UQL statement does not return rows".to_string()),
    }
}

/// A run failure as a refusal: the engine's own code when the text carries one.
fn run_refusal(text: String) -> StoredQueryRefusal {
    let code = text
        .split(':')
        .next()
        .filter(|head| {
            !head.is_empty()
                && head
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        })
        .unwrap_or(INTERNAL);
    refuse(code, text.clone())
}

/// `StoredQueryCall`: resolve the version, check the share scope, bind the arguments, then
/// serve the rows (materialized or fresh) with their freshness.
pub(crate) async fn handle_stored_call(
    ctx: &QueryHandlerCtx<'_>,
    name: &str,
    version: Option<u32>,
    args: &BTreeMap<String, Value>,
) -> Result<StoredCall, StoredQueryRefusal> {
    let not_found = || refuse(STORED_QUERY_NOT_FOUND, format!("no stored query `{name}`"));
    let version = match version {
        Some(version) => version,
        None => load_head(ctx.core, name).ok_or_else(not_found)?.latest,
    };
    let stored = load_query(ctx.core, name, version).ok_or_else(not_found)?;
    let query = &stored.query;
    let author = UnitPrincipal {
        id: query.author.clone(),
        placement: ctx.graph_name.to_string(),
        tenant: stored.author_tenant,
    };
    if !query.may_call(&reader_of(ctx), &author) {
        return Err(refuse(
            ACCESS_DENIED,
            "the caller is outside the share scope of the stored query",
        ));
    }
    let bound = query.bind(args)?;
    let now = now_ms();
    if !query.materialize {
        let (columns, rows, generation) = run_uql(ctx, &query.text, &bound)
            .await
            .map_err(run_refusal)?;
        let row_count = rows.as_array().map_or(0, |rows| rows.len() as u64);
        return Ok(StoredCall {
            name: name.to_string(),
            version,
            materialized: false,
            columns,
            rows,
            row_count,
            input_generation: generation,
            refreshed_at_ms: now,
            staleness_s: 0,
        });
    }
    let digest = args_digest(&bound);
    let prior = load_result(ctx.core, name, version, &digest);
    let outcome = if needs_run(query.refresh, prior.as_ref(), now) {
        Some(
            run_uql(ctx, &query.text, &bound)
                .await
                .map_err(run_refusal)?,
        )
    } else {
        None
    };
    let (result, served) = serve_materialized(query.refresh, prior, now, || {
        outcome.ok_or_else(|| "the run was skipped".to_string())
    })
    .map_err(internal)?;
    if served == Served::Recomputed {
        let props = json!({
            "type": RESULT_LABEL,
            "name": name,
            "version": version,
            "args_digest": digest,
            "result": serde_json::to_value(&result).map_err(internal)?,
        });
        write_node(ctx.core, result_node(name, version, &digest), &props)?;
    }
    Ok(StoredCall {
        name: name.to_string(),
        version,
        materialized: true,
        staleness_s: staleness_s(result.refreshed_at_ms, now),
        columns: result.columns,
        rows: result.rows,
        row_count: result.row_count,
        input_generation: result.input_generation,
        refreshed_at_ms: result.refreshed_at_ms,
    })
}

// ── UQL: CALL STORED name[@version](arg = literal, …) ─────────────────────────────────

/// Serve a `CALL STORED` statement through the same handler as `StoredQueryCall`, answering
/// in the shape of every UQL read.
pub(crate) async fn handle_call_stored_statement(
    ctx: &QueryHandlerCtx<'_>,
    call: medulla_uql::CallStored,
) -> Response {
    use medulla_protocol_types::result_contract::query as results;
    let args = call
        .args
        .into_iter()
        .map(|(name, arg)| {
            let value = match arg {
                medulla_uql::CallArg::Str(text) => Value::String(text),
                medulla_uql::CallArg::Num(number) => {
                    if number.fract() == 0.0 && number.abs() < 9.0e15 {
                        json!(number as i64)
                    } else {
                        json!(number)
                    }
                }
                medulla_uql::CallArg::Bool(flag) => Value::Bool(flag),
            };
            (name, value)
        })
        .collect();
    let called = match handle_stored_call(ctx, &call.name, call.version, &args).await {
        Ok(called) => called,
        Err(refusal) => return Response::err(ctx.req_id, refusal.to_string()),
    };
    let rows = called
        .rows
        .as_array()
        .map(|objects| {
            objects
                .iter()
                .map(|object| UqlRow {
                    id: object
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    score: object
                        .get("score")
                        .and_then(Value::as_f64)
                        .map(|s| s as f32),
                    channels: called
                        .columns
                        .iter()
                        .map(|column| object.get(column).and_then(Value::as_f64))
                        .collect(),
                    knowledge: None,
                    proof: None,
                })
                .collect()
        })
        .unwrap_or_default();
    let body = UqlResult::Rows {
        columns: called.columns,
        rows,
        warnings: vec![format!(
            "stored query `{}` version {}: staleness_s={}",
            called.name, called.version, called.staleness_s
        )],
        snapshot_revision: called.input_generation,
        knowledge_revision: 0,
    };
    match ResultPayload::of::<results::Uql>(body) {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    }
}
