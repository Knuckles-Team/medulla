//! Governed query handler. Owns BOTH query methods (one module per domain, per
//! the dispatch conventions — `Sql` + `CypherQuery` are the one `// ── Query ──`
//! protocol section):
//!   * `Method::Sql` (CONCEPT:MEDULLA-KG.query.read-only-sql-query, feature `query`) — `SELECT … FROM nodes …`
//!     over ONE graph via DataFusion (medulla-query::exec_sql).
//!   * `Method::CypherQuery` (CONCEPT:MEDULLA-KG.query.dep-free-behind, feature `cypher`) — `MATCH … RETURN
//!     …` over ONE graph, DEP-FREE (medulla-query::exec_cypher; label index / VF2 / BFS,
//!     no DataFusion). This is the lean-Pi query path.
//!
//! SQL and Cypher reads use detached, policy-filtered snapshots. Query-language
//! writes are staged and committed by the centralized MutationBatch boundary.
//! Cypher's explicit wire mode is verified against the native parser before
//! admission, authorization, or persistence selection.
//!
//! Off-lock execution: take the owned `analysis_snapshot()` (a GraphView that
//! shares property bytes by Arc) under a brief read lock, then run on the blocking
//! pool via `compute_off_lock` — the VF2/algorithm idiom, so no query work runs on
//! a reactor worker or under the graph lock.
//!
//! Each arm is gated on ITS feature and returns `Err(method)` when its feature is
//! off, so a method whose feature is absent falls through to the graph_ops
//! "not available in this build" catch-all (never a panic, never a mis-route).

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_graph_core::graph::GraphCore;
#[cfg(feature = "graphql")]
use medulla_graphql::parser::{Field, GqlValue};
use medulla_protocol_types::protocol::Method;
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
#[cfg(feature = "result-cache")]
use medulla_query_cache::result_cache::ResultCache;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::compute::compute_off_lock;
use medulla_server_api::state::ServerState;
#[cfg(feature = "federation")]
use medulla_server_ops::api::query_txn::foreign_catalog::bound_registry;
#[cfg(feature = "query")]
use medulla_server_ops::api::query_txn::query_support::bind_sql_text_embedder;
use medulla_server_ops::api::query_txn::query_support::overlay_write_set;
#[cfg(all(feature = "query", feature = "tsdb"))]
use medulla_server_ops::api::query_txn::query_support::plan_needs_tsdb;
use medulla_server_ops::api::query_txn::query_support::uql_text_embedder;
#[cfg(feature = "tsdb")]
use medulla_server_ops::api::query_txn::query_support::TsdbLegBind;
use medulla_server_ops::api::query_txn::query_support::{execute_ranked, execute_rows};

#[cfg(feature = "query")]
use medulla_protocol_types::result_contract::EncodeRef;
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
use medulla_protocol_types::{
    result_contract::encoding, result_contract::query as query_results, result_contract::Dynamic,
    result_contract::MethodResult,
};

#[cfg(feature = "query")]
pub type UqlParams = std::collections::BTreeMap<String, medulla_query_types::wire::UqlParam>;

/// MessagePack bytes of a value that is not itself a result: a result-cache key, or one
/// row of a SQL result.
#[cfg(feature = "query")]
pub fn msgpack_bytes<T: serde::Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("INTERNAL: result serialization failed: {error}"))
}

#[cfg(feature = "query")]
pub fn bind_served_statement(
    req_id: u64,
    text: &str,
    params: &UqlParams,
    now_secs: f64,
) -> Result<(medulla_uql::Statement, medulla_query_types::wire::Plan), Response> {
    let statement = medulla_uql::parse_statement_at(text, params, now_secs).map_err(|error| {
        Response::err(req_id, format!("{}: {}", error.code, error.render(text)))
    })?;
    let binding = medulla_plan::uql::serve::binding_plan(&statement);
    Ok((statement, binding))
}

#[cfg(all(feature = "query", feature = "result-cache"))]
pub fn cached_served_response<M>(
    req_id: u64,
    core: &GraphCore,
    tenant: &str,
    key: &Option<(u128, Option<medulla_query_cache::dep_scope::DepSet>)>,
) -> Option<Response>
where
    M: MethodResult<Encoding = encoding::Raw>,
{
    key.as_ref()
        .and_then(|(hash, dep)| cached_payload(core, tenant, *hash, dep))
        .map(|bytes| Response::ok(req_id, ResultPayload::of_cache_hit::<M>(bytes)))
}

/// Encode a [`medulla_sql::sql::TypedQueryResult`] as the wire `Sql` response:
/// msgpack every cell, or fail the whole response if one cell can't encode.
/// The plain, cancellable, and graph-table-backed typed-SQL execution paths
/// all reach exactly this row-to-wire step once DataFusion has produced
/// typed rows — one owner so a change to how a cell fails to encode can't
/// land on one path and not the others.
#[cfg(feature = "query")]
pub fn typed_sql_response(
    req_id: u64,
    typed: medulla_sql_lang::sql::typed_result::TypedQueryResult,
    warnings: Vec<String>,
) -> Response {
    match typed.rows.iter().map(msgpack_bytes).collect() {
        Ok(rows) => dynamic_response::<query_results::Sql, _>(
            req_id,
            &medulla_protocol_types::protocol::QueryResult {
                columns: typed.columns.iter().map(|c| c.name.clone()).collect(),
                rows,
                warnings,
            },
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Answer with `body` encoded as `M`'s declared result.
#[cfg(feature = "query")]
pub fn result_response<M>(req_id: u64, body: &M::Body) -> Response
where
    M: MethodResult,
    M::Encoding: EncodeRef<M::Body>,
{
    Response::ok(req_id, ResultPayload::of_ref::<M>(body))
}

/// Answer with a caller-shaped `body` encoded as `M`'s declared `Raw` result.
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
pub fn dynamic_response<M, T>(req_id: u64, body: &T) -> Response
where
    M: MethodResult<Body = Dynamic, Encoding = encoding::Raw>,
    T: serde::Serialize + ?Sized,
{
    Response::ok(req_id, ResultPayload::of_dynamic::<M, T>(body))
}

pub mod base;
pub use base::*;
#[cfg(feature = "federation")]
pub mod foreign_leg;
#[cfg(feature = "federation")]
pub use foreign_leg::*;

#[cfg(feature = "query")]
pub mod code_map;
#[cfg(feature = "query")]
pub mod spec_intelligence;
pub mod sql_read;
pub use sql_read::*;
#[cfg(feature = "query")]
pub mod access_port;
#[cfg(feature = "query")]
pub mod asset_usage;
#[cfg(feature = "query")]
pub mod catalog_search;
#[cfg(feature = "query")]
pub mod code_context;
#[cfg(feature = "query")]
pub mod column_lineage;
#[cfg(feature = "query")]
pub mod connector_tool;
#[cfg(feature = "query")]
pub mod corpus_retrieve;
#[cfg(feature = "query")]
pub mod node_body;
#[cfg(feature = "query")]
pub mod run_metrics;
#[cfg(feature = "query")]
pub mod run_trace_export;
#[cfg(feature = "query")]
pub mod schema_context;
#[cfg(feature = "query")]
pub mod schema_projections;
#[cfg(all(feature = "query", feature = "federation", feature = "rdf"))]
pub mod source_coverage;
#[cfg(feature = "query")]
pub mod sql_nearest;
#[cfg(feature = "query")]
pub mod standards;
#[cfg(feature = "query")]
pub use code_context::{
    handle_code_context, handle_code_health_sweep, handle_code_navigation, handle_diagnose,
};
#[cfg(all(feature = "query", feature = "federation-sql"))]
pub use column_lineage::derive_lineage;
#[cfg(feature = "query")]
pub use column_lineage::handle_column_lineage;
#[cfg(feature = "query")]
pub mod document_encoding;
#[cfg(all(feature = "query", feature = "nl-query"))]
pub mod expansion;
#[cfg(feature = "query")]
pub mod projection;
#[cfg(feature = "query")]
pub use corpus_retrieve::handle_corpus_retrieve;
#[cfg(feature = "query")]
pub use schema_context::handle_schema_context;
#[cfg(feature = "query")]
pub use schema_projections::handle_schema_projections;
#[cfg(all(feature = "query", feature = "federation", feature = "rdf"))]
pub use source_coverage::handle_source_coverage;
#[cfg(feature = "query")]
pub use standards::handle_standards_drift;
pub mod explain_handlers;
pub use explain_handlers::*;

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "graphql")]
pub use graphql::*;
#[cfg(feature = "cypher")]
pub mod cypher;
#[cfg(feature = "cypher")]
pub use cypher::*;
pub mod planning;
pub use planning::*;
#[cfg(feature = "query")]
pub mod segment_filter;
#[cfg(feature = "query")]
pub use segment_filter::*;
#[cfg(all(feature = "query", feature = "result-cache"))]
pub mod foreign_watermarks;
#[cfg(all(feature = "query", feature = "result-cache"))]
pub use foreign_watermarks::*;
#[cfg(all(feature = "query", feature = "result-cache"))]
pub mod freshness_feed;
#[cfg(all(feature = "query", feature = "result-cache"))]
pub use freshness_feed::*;
pub mod explain_wire_a;
pub use explain_wire_a::*;
#[cfg(any(
    feature = "epistemic-tms",
    feature = "evidence-graph",
    feature = "epistemic-causal"
))]
pub mod explain_wire_b;
#[cfg(any(
    feature = "epistemic-tms",
    feature = "evidence-graph",
    feature = "epistemic-causal"
))]
pub use explain_wire_b::*;
pub mod overlay;
pub use overlay::*;
pub mod sql_write;
pub use sql_write::*;
pub mod sql_dispatch;
#[cfg(feature = "query")]
pub mod sql_matview;
pub use sql_dispatch::*;
pub mod sql_catalog;
pub use sql_catalog::*;
#[cfg(feature = "query")]
pub mod argument_map;
#[cfg(feature = "query")]
pub mod entity_link;
#[cfg(feature = "query")]
pub mod harness_metrics;
