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
//! SQL stored queries run through `handle_sql` and SPARQL stored queries through the served
//! SPARQL handler (R708.10); each `$name` becomes one typed, escaped literal
//! (`StoredQuery::render`), and a put refuses text that is not a parse-clean read.

use std::collections::BTreeMap;

use medulla_identity_types::identity::UnitPrincipal;
use medulla_query_types::wire::{UqlParam, UqlResult, UqlRow};
use medulla_server_api::read_scope::ReadCaller;
use medulla_source_types::stored_query::{
    args_digest, lint_stored_query, needs_run, refuse, serve_materialized, staleness_s, Served,
    StoredCall, StoredParamType, StoredQuery, StoredQueryLanguage, StoredQueryReceipt,
    StoredQueryRefusal, StoredQueryWarning, StoredResult, ACCESS_DENIED, STORED_QUERY_IMMUTABLE, STORED_QUERY_INVALID, STORED_QUERY_NOT_FOUND,
};
use serde_json::{json, Value};

use super::live_artifact::{response_body, row_json};
use super::uql_statement::handle_uql;
use super::*;

const QUERY_LABEL: &str = "StoredQuery";
const HEAD_LABEL: &str = "StoredQueryHead";
const RESULT_LABEL: &str = "StoredQueryResult";
const INTERNAL: &str = "INTERNAL";
/// The node property that holds the assets a stored query version reads
/// (MEDULLA-FEDERATED-QUERY-R708.9).
const REFERENCES_PROPERTY: &str = "references";

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

pub(super) fn reader_of(ctx: &QueryHandlerCtx<'_>) -> UnitPrincipal {
    let mut principal = ReadCaller::from_authority(ctx.read_authority, ctx.graph_name).principal;
    principal.id = ctx.caller.to_string();
    principal
}

/// The nodes that register the catalog stored queries (MEDULLA-TYPED-PACKS-R108.17,
/// MEDULLA-TYPED-PACKS-R802.8): for each registered query whose head node is not in
/// `existing_heads`, its version node and head node, in the shape `StoredQueryPut` publishes,
/// authored by `author` and shared with `tenant`. The one definition of what is seeded.
pub fn catalog_query_seed_nodes(
    author: &str,
    tenant: &str,
    existing_heads: &std::collections::BTreeSet<String>,
) -> Result<Vec<(String, Value)>, String> {
    let mut nodes = Vec::new();
    for mut query in medulla_source_types::catalog_queries::catalog_queries() {
        if existing_heads.contains(&head_node(&query.name)) {
            continue;
        }
        query.author = author.to_string();
        let stored = json!({
            "type": QUERY_LABEL,
            "name": query.name,
            "version": query.version,
            "author": author,
            "author_tenant": tenant,
            "query": serde_json::to_value(&query).map_err(|e| e.to_string())?,
        });
        nodes.push((query_node(&query.name, query.version), stored));
        let head = json!({
            "type": HEAD_LABEL,
            "name": query.name,
            "author": author,
            "latest": query.version,
        });
        nodes.push((head_node(&query.name), head));
    }
    Ok(nodes)
}

/// Head node ids of the catalog stored queries, for an existence probe.
pub fn catalog_query_head_nodes() -> Vec<String> {
    medulla_source_types::catalog_queries::catalog_queries()
        .iter()
        .map(|query| head_node(&query.name))
        .collect()
}

/// Seed the catalog stored queries into the staged graph `core` of a catalog write: the one
/// seeding function. `core` is the staged graph of the write, so the queries commit with the
/// asset or not at all. `Stewardship.MarkCritical` writes the same nodes
/// (`catalog_query_seed_nodes`) through its own batch.
pub fn seed_catalog_queries(
    core: &GraphCore,
    authority: Option<&medulla_server_api::carrier::GraphReadAuthority>,
    graph_name: &str,
) -> Result<(), String> {
    let author = authority
        .map(medulla_server_api::carrier::GraphReadAuthority::verified_principal)
        .unwrap_or_default()
        .to_string();
    let tenant = ReadCaller::from_authority(authority, graph_name).principal.tenant;
    let existing = catalog_query_head_nodes()
        .into_iter()
        .filter(|head| core.has_node(head))
        .collect();
    for (node, props) in catalog_query_seed_nodes(&author, &tenant, &existing)? {
        write_node(core, node, &props).map_err(|refusal| refusal.to_string())?;
    }
    Ok(())
}

// ── Lint (MEDULLA-FEDERATED-QUERY-R708.9) ─────────────────────────────────────────────

/// The assets a stored query reads, found by the extractor of its language: the SQL tables of a
/// SQL query, the scan labels of the plan a UQL query parses to. SPARQL has no extractor, so a
/// SPARQL stored query records no references and is never linted.
fn query_references(query: &StoredQuery) -> Vec<String> {
    match query.language {
        StoredQueryLanguage::Uql => {
            medulla_uql::parse_with(&query.text, &placeholder_params(query))
                .map(|plan| super::asset_usage::plan_labels(&plan))
                .unwrap_or_default()
        }
        #[cfg(feature = "federation-sql")]
        StoredQueryLanguage::Sql => query
            .render(&placeholder_values(query))
            .ok()
            .and_then(|text| {
                medulla_federation::sql_analysis::referenced_tables(
                    &text,
                    medulla_exec::sql_text::SqlDialect::Postgres,
                )
                .ok()
            })
            .unwrap_or_default(),
        #[cfg(not(feature = "federation-sql"))]
        StoredQueryLanguage::Sql => Vec::new(),
        StoredQueryLanguage::Sparql => Vec::new(),
    }
}

/// The assets the caller's tenant has flagged `deprecation`, read from the trust-flag nodes of
/// the graph.
fn deprecated_in(ctx: &QueryHandlerCtx<'_>) -> std::collections::BTreeSet<String> {
    let flags: Vec<serde_json::Map<String, Value>> = ctx
        .core
        .get_nodes_by_label(medulla_catalog_types::stewardship::TRUST_FLAG_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(_, blob)| medulla_types::msgpack::decode_property_object(&blob).ok())
        .collect();
    medulla_catalog_types::stewardship::deprecated_assets(&flags, &reader_of(ctx).tenant)
}

/// The warnings of one stored version: its recorded references against the deprecated assets.
fn lint_version(ctx: &QueryHandlerCtx<'_>, references: &[String]) -> Vec<StoredQueryWarning> {
    if references.is_empty() {
        return Vec::new();
    }
    lint_stored_query(references, &deprecated_in(ctx))
}

/// The references recorded on a stored version node.
fn recorded_references(core: &GraphCore, name: &str, version: u32) -> Vec<String> {
    props_of(core, &query_node(name, version))
        .and_then(|props| serde_json::from_value(props.get(REFERENCES_PROPERTY)?.clone()).ok())
        .unwrap_or_default()
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

/// Declared parameter types as placeholder JSON values, so a put renders the text once.
fn placeholder_values(query: &StoredQuery) -> BTreeMap<String, Value> {
    query
        .params
        .iter()
        .map(|(name, declared)| {
            let value = match declared {
                StoredParamType::String => json!(""),
                StoredParamType::Int => json!(0),
                StoredParamType::Float => json!(0.0),
                StoredParamType::Bool => json!(false),
            };
            (name.clone(), value)
        })
        .collect()
}

/// A SQL or SPARQL stored query must parse and must be a read; a stored query never writes.
fn check_text(language: StoredQueryLanguage, text: &str) -> Result<(), StoredQueryRefusal> {
    match language {
        StoredQueryLanguage::Sql => match medulla_sql_lang::sql::classify::classify(text) {
            Ok(medulla_sql_lang::sql::classify::StatementKind::Read) => Ok(()),
            Ok(_) => Err(refuse(
                STORED_QUERY_INVALID,
                "a stored sql query must be a read statement",
            )),
            Err(error) => Err(refuse(STORED_QUERY_INVALID, error)),
        },
        #[cfg(feature = "sparql")]
        StoredQueryLanguage::Sparql => medulla_rdf::sparql::parse_query(text)
            .map(|_| ())
            .map_err(|error| refuse(STORED_QUERY_INVALID, error)),
        #[cfg(not(feature = "sparql"))]
        StoredQueryLanguage::Sparql => Err(refuse(
            STORED_QUERY_INVALID,
            "sparql is not served in this build",
        )),
        StoredQueryLanguage::Uql => Ok(()),
    }
}

/// `StoredQueryPut`: validate, then publish one immutable version. The author is the
/// verified caller; the first author of a name owns every later version.
pub(crate) fn handle_stored_put(
    ctx: &QueryHandlerCtx<'_>,
    mut query: StoredQuery,
) -> Result<StoredQueryReceipt, StoredQueryRefusal> {
    query.validate(medulla_server_api::knob_config::knobs().stored_query_min_period_s)?;
    match query.language {
        StoredQueryLanguage::Uql => {
            medulla_uql::parse_statement(&query.text, &placeholder_params(&query))
                .map_err(|error| refuse(STORED_QUERY_INVALID, error.render(&query.text)))?;
        }
        StoredQueryLanguage::Sql | StoredQueryLanguage::Sparql => {
            let text = query
                .render(&placeholder_values(&query))
                .map_err(|refusal| refuse(STORED_QUERY_INVALID, refusal.detail))?;
            check_text(query.language, &text)?;
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
    let references = query_references(&query);
    let latest = head.map_or(0, |head| head.latest).max(query.version);
    let props = json!({
        "type": QUERY_LABEL,
        "name": query.name,
        "version": query.version,
        "author": query.author,
        "author_tenant": reader_of(ctx).tenant,
        "query": serde_json::to_value(&query).map_err(internal)?,
        (REFERENCES_PROPERTY): references,
    });
    write_node(ctx.core, query_node(&query.name, query.version), &props)?;
    let head = json!({
        "type": HEAD_LABEL,
        "name": query.name,
        "author": query.author,
        "latest": latest,
    });
    write_node(ctx.core, head_node(&query.name), &head)?;
    let warnings = lint_version(ctx, &references);
    Ok(StoredQueryReceipt { query, warnings })
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
    let response = handle_uql(
        ctx,
        text.to_string(),
        uql_params(bound),
        false,
        Default::default(),
        None,
    )
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

/// Run one stored query in its own language through the served handler of that language.
/// SQL rows and SPARQL bindings become one object per row keyed by column, like UQL rows.
async fn run_stored(
    ctx: &QueryHandlerCtx<'_>,
    query: &StoredQuery,
    bound: &BTreeMap<String, Value>,
) -> Result<(Vec<String>, Value, u64), String> {
    let text = query.render(bound).map_err(|refusal| refusal.to_string())?;
    match query.language {
        StoredQueryLanguage::Uql => run_uql(ctx, &text, bound).await,
        StoredQueryLanguage::Sql => run_sql(ctx, &text).await,
        StoredQueryLanguage::Sparql => run_sparql_stored(ctx, &text).await,
    }
}

fn objects(columns: &[String], rows: Vec<Vec<Value>>) -> Value {
    Value::Array(
        rows.into_iter()
            .map(|cells| {
                Value::Object(
                    columns
                        .iter()
                        .cloned()
                        .zip(cells.into_iter().chain(std::iter::repeat(Value::Null)))
                        .collect(),
                )
            })
            .collect(),
    )
}

#[cfg(feature = "query")]
async fn run_sql(
    ctx: &QueryHandlerCtx<'_>,
    text: &str,
) -> Result<(Vec<String>, Value, u64), String> {
    let response = handle_sql(ctx, text.to_string(), Vec::new())
        .await
        .map_err(|_| "SQL is not served in this build".to_string())?;
    let result = response_body::<medulla_protocol_types::protocol::QueryResult>(response)?;
    let rows = result
        .rows
        .iter()
        .map(|blob| rmp_serde::from_slice::<Vec<Value>>(blob).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let objects = objects(&result.columns, rows);
    Ok((result.columns, objects, ctx.core.version()))
}

#[cfg(not(feature = "query"))]
async fn run_sql(
    _ctx: &QueryHandlerCtx<'_>,
    _text: &str,
) -> Result<(Vec<String>, Value, u64), String> {
    Err("SQL is not served in this build".to_string())
}

#[cfg(feature = "sparql")]
async fn run_sparql_stored(
    ctx: &QueryHandlerCtx<'_>,
    text: &str,
) -> Result<(Vec<String>, Value, u64), String> {
    let response = super::live_artifact::sparql_response(ctx, text).await?;
    let result = response_body::<medulla_protocol_types::protocol::SparqlResult>(response)?;
    let rows = result
        .rows
        .into_iter()
        .map(|cells| {
            cells
                .into_iter()
                .map(|cell| cell.map_or(Value::Null, |term| Value::String(term.display_text())))
                .collect()
        })
        .collect();
    let objects = objects(&result.vars, rows);
    Ok((result.vars, objects, ctx.core.version()))
}

#[cfg(not(feature = "sparql"))]
async fn run_sparql_stored(
    _ctx: &QueryHandlerCtx<'_>,
    _text: &str,
) -> Result<(Vec<String>, Value, u64), String> {
    Err("SPARQL is not served in this build".to_string())
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
    // The lint runs before each run; it warns and never stops the run.
    let warnings = lint_version(ctx, &recorded_references(ctx.core, name, version));
    if !query.materialize {
        let (columns, rows, generation) =
            run_stored(ctx, query, &bound).await.map_err(run_refusal)?;
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
            warnings,
        });
    }
    let digest = args_digest(&bound);
    let prior = load_result(ctx.core, name, version, &digest);
    let outcome = if needs_run(query.refresh, prior.as_ref(), now) {
        Some(run_stored(ctx, query, &bound).await.map_err(run_refusal)?)
    } else {
        None
    };
    let (mut result, served) = serve_materialized(query.refresh, prior, now, || {
        outcome.ok_or_else(|| "the run was skipped".to_string())
    })
    .map_err(internal)?;
    if served == Served::Recomputed {
        result.warnings = warnings.clone();
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
        warnings,
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
                    provenance: Default::default(),
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
        source_coverage: Vec::new(),
        historical_risk: Vec::new(),
        cost: None,
    };
    match ResultPayload::of::<results::Uql>(body) {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    }
}
