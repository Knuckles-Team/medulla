//! Query helpers moved below the handler crates so wire and decide can reach them (MEDULLA-CONTRACT-R056.8).

#[cfg(feature = "query")]
use medulla_protocol_types::protocol::Method;

/// The process-wide server-side text→vector embedder for the UQL `RANK BY ~ "text"`
/// (`Op::RankEmbed`) NL→vector seam (CONCEPT:MEDULLA-KG.query.bind-server-side-text / MEDULLA-411) — the FACADE injection point.
/// Returns the bound embedder, or `None` when none is configured (an `Op::RankEmbed` then
/// errors cleanly). The engine stores embeddings but produces them client-side today, so no
/// in-process model ships and the default is `None`; `MEDULLA_UQL_TEXT_EMBEDDER=hash` binds the
/// deterministic `HashEmbedder` fallback (offline/testing — arbitrary ranking). A real
/// embedding model (an ONNX/remote-service impl of `medulla_plan::TextEmbedder`) is wired in HERE.
#[cfg(feature = "query")]
pub fn uql_text_embedder() -> Option<&'static dyn medulla_exec::exec::TextEmbedder> {
    bound_text_embedder().map(|(embedder, _)| embedder)
}

/// The identity a bound text embedder registers under in the modality encoder registry
/// (MEDULLA-FEDERATED-QUERY-R039.1.3): the registry id a space's text binding names, and
/// the `(space id, dimensions, revision)` tuple of the space its vectors live in.
#[cfg(feature = "query")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEncoderIdentity {
    pub encoder_id: String,
    pub space_id: String,
    pub dimensions: usize,
    pub revision: String,
}

/// The revision both built-in text encoders report: the encoding recipe they implement.
#[cfg(feature = "query")]
pub const TEXT_ENCODER_REVISION: &str = "1";

/// The registry id of the deterministic hash encoder.
#[cfg(feature = "query")]
pub const HASH_ENCODER_ID: &str = "builtin:hash";

/// The registry id of the configured remote chunk embedder.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub const REMOTE_ENCODER_ID: &str = "remote:openai-compatible";

/// The identity of the bound text embedder, or `None` when nothing is bound.
#[cfg(feature = "query")]
pub fn uql_text_encoder_identity() -> Option<TextEncoderIdentity> {
    bound_text_embedder().map(|(_, identity)| identity)
}

#[cfg(feature = "query")]
fn bound_text_embedder() -> Option<(
    &'static dyn medulla_exec::exec::TextEmbedder,
    TextEncoderIdentity,
)> {
    use std::sync::OnceLock;
    type Bound = Option<(
        Box<dyn medulla_exec::exec::TextEmbedder>,
        TextEncoderIdentity,
    )>;
    static EMBEDDER: OnceLock<Bound> = OnceLock::new();
    EMBEDDER
        .get_or_init(|| {
            // The configured chunk embedder (MEDULLA-UNIFIED-DATA-PLANE-R651.7.2) when an
            // endpoint is set, so query vectors share the stored chunks' space.
            #[cfg(feature = "modality-serving")]
            {
                let config = crate::api::ingest::embedder::EmbedderConfig::from_env();
                if config.name() == crate::api::ingest::embedder::REMOTE {
                    let identity = TextEncoderIdentity {
                        encoder_id: REMOTE_ENCODER_ID.to_string(),
                        space_id: config
                            .model
                            .clone()
                            .unwrap_or_else(|| crate::api::ingest::embedder::REMOTE.to_string()),
                        dimensions: config.dims,
                        revision: TEXT_ENCODER_REVISION.to_string(),
                    };
                    return Some((
                        Box::new(config) as Box<dyn medulla_exec::exec::TextEmbedder>,
                        identity,
                    ));
                }
            }
            match std::env::var("MEDULLA_UQL_TEXT_EMBEDDER").ok().as_deref() {
                Some("hash") => {
                    let embedder = medulla_exec::exec::HashEmbedder::default();
                    let dimensions = medulla_exec::exec::TextEmbedder::embed(&embedder, "")
                        .map(|vector| vector.len())
                        .unwrap_or_default();
                    Some((
                        Box::new(embedder) as Box<dyn medulla_exec::exec::TextEmbedder>,
                        TextEncoderIdentity {
                            encoder_id: HASH_ENCODER_ID.to_string(),
                            space_id: HASH_ENCODER_ID.to_string(),
                            dimensions,
                            revision: TEXT_ENCODER_REVISION.to_string(),
                        },
                    ))
                }
                _ => None,
            }
        })
        .as_ref()
        .map(|(embedder, identity)| (&**embedder, identity.clone()))
}

/// Bind the SQL `medulla_embed(text)` function's process-wide embedder to the SAME
/// `medulla_plan::TextEmbedder` [`uql_text_embedder`] already gives the UQL `RANK BY ~ "text"`
/// leg (design `plans/semantic-indexing/DESIGN-embedding-bindings.md` §9 phase 2). ONE
/// trait, TWO callers: `medulla-query` cannot name `medulla_plan::TextEmbedder` (`medulla-plan` depends
/// on `medulla-query`, so the reverse edge is a Cargo package cycle — see
/// `medulla_sql::sql::embed_udf`'s module doc), so this facade — which depends on BOTH
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
        let _ = medulla_sql::sql::bind_text_embedder(std::sync::Arc::new(move |text: &str| {
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
    pub staged_series: Option<&'a medulla_exec::exec::StagedSeries>,
}

/// Execute a unified cross-modal plan (CONCEPT:AU-KG.compute.vector/209) over
/// the bound context and project its rows as `[id, score|nil]`, with the named text and
/// numeric values a source attached to a row as a third element (MEDULLA-CQ-XSRC-R002.7).
#[cfg(feature = "query")]
pub fn execute_rows(
    plan: &medulla_exec::algebra::Plan,
    ctx: &medulla_exec::exec::PlanCtx,
) -> Result<Vec<medulla_types::unified_row::UnifiedRow>, String> {
    Ok(unified_rows(&medulla_exec::exec::execute(plan, ctx)?))
}

/// MEDULLA-FEDERATED-QUERY-R039: [`execute_rows`] for a plan that sets `report_shortfall`:
/// the same rows plus the typed `Shortfall` of a top-`k` rank that could not return `k`.
#[cfg(feature = "query")]
pub fn execute_ranked(
    plan: &medulla_exec::algebra::Plan,
    ctx: &medulla_exec::exec::PlanCtx,
) -> Result<medulla_types::retrieval::RankedRows, String> {
    let result = medulla_exec::exec::execute(plan, ctx)?;
    let shortfall = result.shortfall().copied();
    let constraint = ctx
        .constraints
        .as_ref()
        .map(|constraints| constraints.trace().clone());
    // MEDULLA-FEDERATED-QUERY-R039.1.5: present exactly when the request named a modality pair.
    let modality = ctx.modality_pair.as_ref().map(|pair| {
        let calls = ctx
            .encoding_recorder
            .as_ref()
            .map(|recorder| recorder.calls())
            .unwrap_or_default();
        let mut model_revisions = Vec::new();
        if let (Some(recorder), false) = (&ctx.encoding_recorder, calls.is_empty()) {
            model_revisions.push(recorder.revision().to_string());
        }
        medulla_types::retrieval::TraceModality {
            modality_direction: format!(
                "{}->{}",
                modality_name(&pair.query),
                modality_name(&pair.target)
            ),
            space_id: ctx
                .semantic
                .space()
                .map(|space| space.id.clone())
                .unwrap_or_default(),
            model_revisions,
            index_generation: ctx.semantic.generation(),
            authorized_shortfall_counts: shortfall,
            encoding_calls: calls,
        }
    });
    let receipt = (constraint.is_some() || modality.is_some()).then_some(
        medulla_types::retrieval::RetrievalReceipt {
            constraint,
            modality,
        },
    );
    Ok(medulla_types::retrieval::RankedRows {
        rows: unified_rows(&result),
        receipt,
        shortfall,
    })
}

/// The lowercase name of a modality, as the receipt and the wire spell it (the variants of
/// `Modality` are single words, so the lowercase of its name is its wire name).
#[cfg(feature = "query")]
fn modality_name(modality: &impl std::fmt::Debug) -> String {
    format!("{modality:?}").to_lowercase()
}

#[cfg(feature = "query")]
fn unified_rows(
    result: &medulla_exec::rowset::RowSet,
) -> Vec<medulla_types::unified_row::UnifiedRow> {
    let (texts, values) = (result.texts(), result.values());
    result
        .rows()
        .iter()
        .map(|r| medulla_types::unified_row::UnifiedRow {
            id: r.id.clone(),
            score: r.score,
            texts: texts.get(&r.id).cloned().unwrap_or_default(),
            values: values.get(&r.id).cloned().unwrap_or_default(),
        })
        .collect()
}

#[cfg(all(feature = "query", feature = "tsdb"))]
pub fn plan_needs_tsdb(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_exec::algebra::Op::TsScan { .. } => true,
        // MEDULLA-FEDERATED-QUERY-R078.7: the time-series rows of the company-context view
        // read the committed store, so every scan that can emit them binds it.
        medulla_exec::algebra::Op::CompanyContextScan { category } => matches!(
            category,
            None | Some(medulla_query_types::wire::CompanyCategory::TimeSeries)
        ),
        #[cfg(feature = "text")]
        medulla_exec::algebra::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|branch| plan_needs_tsdb(branch))
        }
        _ => false,
    })
}

/// Replay a txn's staged durable-mutation `write_set` onto a cloned `GraphView` as an
/// overlay (CONCEPT:MEDULLA-KG.query.txn-cross-modal-ryow — in-txn cross-modal RYOW). Mirrors `medulla_h_txn::txn::
/// apply_staged`, but against a view's overlay ops (no ledger/durability): so the
/// Filter (node props) and Traverse (BFS over staged edges) legs of an in-txn unified
/// query observe the txn's own uncommitted graph writes. Only the durable-mutation set
/// is ever staged (the protocol restricts `Txn*` to it); any other variant is a no-op.
#[cfg(feature = "query")]
pub fn overlay_write_set(view: &mut medulla_graph_core::graph::GraphView, write_set: &[Method]) {
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
/// (`medulla_server::api::query_txn::foreign_catalog::ForeignSourceCatalog::resolve_for_plan`), so a
/// non-federated plan pays nothing. A self-describing (inline-spec)
/// `Op::ForeignScan` resolves without a registry, so it does not need the binding.
#[cfg(all(feature = "query", feature = "federation"))]
pub fn plan_needs_foreign(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_exec::algebra::Op::Foreign { .. } => true,
        medulla_exec::algebra::Op::ForeignScan { source, .. } => {
            matches!(
                **source,
                medulla_query_types::wire::ForeignSourceSpec::Named { .. }
            )
        }
        medulla_exec::algebra::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|b| plan_needs_foreign(b))
        }
        _ => false,
    })
}

/// The distinct foreign source names a plan reads, in plan order (MEDULLA-CQ-CONN-R090).
#[cfg(all(feature = "query", feature = "federation"))]
pub fn plan_foreign_names(ops: &[medulla_exec::algebra::Op]) -> Vec<String> {
    fn collect(ops: &[medulla_exec::algebra::Op], names: &mut Vec<String>) {
        for op in ops {
            let name = match op {
                medulla_exec::algebra::Op::Foreign { name } => Some(name.as_str()),
                medulla_exec::algebra::Op::ForeignScan { source, .. } => match source.as_ref() {
                    medulla_query_types::wire::ForeignSourceSpec::Named { name } => {
                        Some(name.as_str())
                    }
                    _ => None,
                },
                medulla_exec::algebra::Op::FuseRrf { branches, .. } => {
                    for branch in branches {
                        collect(branch, names);
                    }
                    None
                }
                _ => None,
            };
            if let Some(name) = name {
                if !names.iter().any(|known| known == name) {
                    names.push(name.to_string());
                }
            }
        }
    }
    let mut names = Vec::new();
    collect(ops, &mut names);
    names
}

/// The plan pin shared by explain and foreign-read binding, also available
/// in query-only builds without the federation feature.
#[cfg(feature = "query")]
pub fn canonical_plan_digest(
    plan: &medulla_exec::algebra::Plan,
) -> Result<medulla_types::contract::Digest256, String> {
    let plan_bytes = rmp_serde::to_vec_named(plan)
        .map_err(|error| format!("INTERNAL: plan digest serialization failed: {error}"))?;
    Ok(medulla_types::contract::Digest256::sha256(&plan_bytes))
}
