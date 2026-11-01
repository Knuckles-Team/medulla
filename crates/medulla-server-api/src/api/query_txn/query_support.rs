//! Query helpers moved below the handler crates so wire and decide can reach them (MEDULLA-CONTRACT-R056.8).

#[cfg(feature = "query")]
use medulla_types::protocol::Method;

/// The process-wide server-side text→vector embedder for the UQL `RANK BY ~ "text"`
/// (`Op::RankEmbed`) NL→vector seam (CONCEPT:MEDULLA-KG.query.bind-server-side-text / MEDULLA-411) — the FACADE injection point.
/// Returns the bound embedder, or `None` when none is configured (an `Op::RankEmbed` then
/// errors cleanly). The engine stores embeddings but produces them client-side today, so no
/// in-process model ships and the default is `None`; `MEDULLA_UQL_TEXT_EMBEDDER=hash` binds the
/// deterministic `HashEmbedder` fallback (offline/testing — arbitrary ranking). A real
/// embedding model (an ONNX/remote-service impl of `medulla_plan::TextEmbedder`) is wired in HERE.
#[cfg(feature = "query")]
pub fn uql_text_embedder() -> Option<&'static dyn medulla_plan::TextEmbedder> {
    use std::sync::OnceLock;
    static EMBEDDER: OnceLock<Option<Box<dyn medulla_plan::TextEmbedder>>> = OnceLock::new();
    EMBEDDER
        .get_or_init(|| {
            // The configured chunk embedder (MEDULLA-UNIFIED-DATA-PLANE-R651.7.2) when an
            // endpoint is set, so query vectors share the stored chunks' space.
            #[cfg(feature = "modality-serving")]
            {
                let config =
                    crate::api::ingest::embedder::EmbedderConfig::from_env();
                if config.name() == crate::api::ingest::embedder::REMOTE {
                    return Some(Box::new(config) as Box<dyn medulla_plan::TextEmbedder>);
                }
            }
            match std::env::var("MEDULLA_UQL_TEXT_EMBEDDER").ok().as_deref() {
                Some("hash") => Some(Box::new(medulla_plan::HashEmbedder::default())
                    as Box<dyn medulla_plan::TextEmbedder>),
                _ => None,
            }
        })
        .as_deref()
}

/// Bind the SQL `medulla_embed(text)` function's process-wide embedder to the SAME
/// `medulla_plan::TextEmbedder` [`uql_text_embedder`] already gives the UQL `RANK BY ~ "text"`
/// leg (design `plans/semantic-indexing/DESIGN-embedding-bindings.md` §9 phase 2). ONE
/// trait, TWO callers: `medulla-query` cannot name `medulla_plan::TextEmbedder` (`medulla-plan` depends
/// on `medulla-query`, so the reverse edge is a Cargo package cycle — see
/// `medulla_query::sql::embed_udf`'s module doc), so this facade — which depends on BOTH
/// crates — is where the concrete embedder crosses that seam, as a closure.
///
/// Idempotent and cheap; called at the top of the SQL entry point rather than from a
/// server-startup hook because `uql_text_embedder` is itself lazily resolved and
/// `bind_text_embedder` is one-shot, so "first SQL statement" is the earliest moment the
/// binding is both possible and needed. With no embedder configured
/// (`MEDULLA_UQL_TEXT_EMBEDDER` unset — today's default, since the engine stores embeddings
/// but produces them client-side) nothing is bound and `medulla_embed(...)` returns its typed
/// "no server-side text embedder is bound" error rather than a zero vector.
///
/// `pub` so the pgwire/wire SQL entry points can bind through this same function
/// rather than growing a second injection site (they do not call [`handle_sql`]).
#[cfg(feature = "query")]
pub fn bind_sql_text_embedder() {
    static BOUND: std::sync::Once = std::sync::Once::new();
    BOUND.call_once(|| {
        let Some(embedder) = uql_text_embedder() else {
            return;
        };
        let _ = medulla_query::sql::bind_text_embedder(std::sync::Arc::new(move |text: &str| {
            embedder.embed(text)
        }));
    });
}

/// Bundles `run_unified`'s tsdb `Op::TsScan` leg-binding parameters — ONE
/// parameter rather than four, so the function's argument count stays under the
/// clippy ceiling (mirrors the `ServedIndexes` rationale above). Only exists
/// under the `tsdb` feature; a non-`tsdb` build omits the parameter entirely.
#[cfg(all(feature = "query", feature = "tsdb"))]
pub struct TsdbLegBind<'a> {
    /// The committed native tsdb `SeriesStore` backing `Op::TsScan`, threaded in
    /// so a UQL plan fuses its time-series leg with the graph/vector/relational
    /// legs. `None` ⇒ a `TsScan` yields no rows (degrade, never err).
    pub tsdb: Option<&'a medulla_tsdb::store::SeriesStore>,
    /// Verified tenant + actor-owned graph policy context for committed TsScan
    /// reads. Served callers bind both or omit the store entirely.
    pub tsdb_tenant: Option<&'a str>,
    pub tsdb_graph: Option<&'a str>,
    /// In-txn tsdb read-your-own-writes (CONCEPT:MEDULLA-KG.query.txn-tsdb-read-your): the resolved txn's OWN staged,
    /// uncommitted series points, overlaid onto `Op::TsScan` BEFORE the committed store so
    /// an in-txn UQL reads its own measurements. `None` off-txn ⇒ committed series only.
    pub staged_series: Option<&'a medulla_plan::StagedSeries>,
}

/// Execute a unified cross-modal plan (CONCEPT:AU-KG.compute.vector/209) over
/// the bound context and project its rows as `[id, score|nil]`.
#[cfg(feature = "query")]
pub fn execute_rows(
    plan: &medulla_plan::Plan,
    ctx: &medulla_plan::PlanCtx,
) -> Result<Vec<(String, Option<f32>)>, String> {
    let result = medulla_plan::execute(plan, ctx)?;
    Ok(result
        .rows()
        .iter()
        .map(|r| (r.id.clone(), r.score))
        .collect())
}

#[cfg(all(feature = "query", feature = "tsdb"))]
pub fn plan_needs_tsdb(ops: &[medulla_plan::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_plan::Op::TsScan { .. } => true,
        #[cfg(feature = "text")]
        medulla_plan::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|branch| plan_needs_tsdb(branch))
        }
        _ => false,
    })
}

/// Replay a txn's staged durable-mutation `write_set` onto a cloned `GraphView` as an
/// overlay (CONCEPT:MEDULLA-KG.query.txn-cross-modal-ryow — in-txn cross-modal RYOW). Mirrors `handlers::txn::
/// apply_staged`, but against a view's overlay ops (no ledger/durability): so the
/// Filter (node props) and Traverse (BFS over staged edges) legs of an in-txn unified
/// query observe the txn's own uncommitted graph writes. Only the durable-mutation set
/// is ever staged (the protocol restricts `Txn*` to it); any other variant is a no-op.
#[cfg(feature = "query")]
pub fn overlay_write_set(view: &mut medulla_core::graph::GraphView, write_set: &[Method]) {
    for m in write_set {
        match m {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => view.overlay_add_node(node_id.clone(), properties_msgpack.clone()),
            Method::RemoveNode { node_id } => view.overlay_remove_node(node_id),
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => {
                view.overlay_add_edge(
                    source_id.clone(),
                    target_id.clone(),
                    properties_msgpack.clone(),
                );
            }
            Method::RemoveEdge {
                source_id,
                target_id,
            } => view.overlay_remove_edge(source_id, target_id),
            Method::CompareAndSetNodeFields {
                node_id,
                conditions_msgpack,
                updates_msgpack,
            } => {
                // A decode failure is a no-op overlay (mirrors `apply_staged`).
                if let (Ok(conditions), Ok(updates)) = (
                    medulla_types::msgpack::decode_property_object(conditions_msgpack),
                    medulla_types::msgpack::decode_property_object(updates_msgpack),
                ) {
                    view.overlay_compare_and_set_fields(node_id, &conditions, &updates);
                }
            }
            _ => {}
        }
    }
}

/// Does `ops` NAME a registered foreign source — an `Op::Foreign` (the UQL
/// `FOREIGN "<name>"` marker) or a `Named` `Op::ForeignScan`, at the top level or nested
/// inside an `Op::FuseRrf` branch (CONCEPT:MEDULLA-KG.query.closure-backed-source, mirroring
/// `plan_needs_text`)? Drives whether a served path builds the caller's owner-scoped
/// foreign-source registry at all
/// (`crate::server::api::query_txn::foreign_catalog::ForeignSourceCatalog::resolve_for_plan`), so a
/// non-federated plan pays nothing. A self-describing (inline-spec)
/// `Op::ForeignScan` resolves without a registry, so it does not need the binding.
#[cfg(all(feature = "query", feature = "federation"))]
pub fn plan_needs_foreign(ops: &[medulla_plan::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_plan::Op::Foreign { .. } => true,
        medulla_plan::Op::ForeignScan { source, .. } => {
            matches!(
                **source,
                medulla_types::wire::ForeignSourceSpec::Named { .. }
            )
        }
        medulla_plan::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|b| plan_needs_foreign(b))
        }
        _ => false,
    })
}

/// The plan pin shared by explain and foreign-read binding, also available
/// in query-only builds without the federation feature.
#[cfg(feature = "query")]
pub fn canonical_plan_digest(
    plan: &medulla_plan::Plan,
) -> Result<medulla_types::contract::Digest256, String> {
    let plan_bytes = rmp_serde::to_vec_named(plan)
        .map_err(|error| format!("INTERNAL: plan digest serialization failed: {error}"))?;
    Ok(medulla_types::contract::Digest256::sha256(&plan_bytes))
}
