use super::*;

/// Compute a SOUND dependency set for a UQL/`UnifiedQuery` plan
/// (CONCEPT:MEDULLA-KG.coordination.dependency-scoped-cache-invalidation, W1.6/P7 + EH-393), or `None`
/// when the plan's shape cannot be reduced to one — the caller then uses the coarse
/// version-keyed result-cache path (unchanged). What each supported op reads:
///   * `Scan { label }` — a SOURCE: that label (or, unlabeled, the whole node set). Its rows are
///     label-scoped, so a following `Filter` reads only properties the label dimension covers.
///   * `ScanAll` (UQL `MATCH ()`) — a SOURCE over every node: `AllNodes`.
///   * `Project` (UQL `RETURN`) — names score channels; rows pass through unchanged.
///   * `Filter` / `Limit` — narrow the current rows. A `Filter` over rows that are NOT
///     label-scoped (reached by a traversal) reads arbitrary nodes' properties: `AllNodes`.
///   * `Traverse { rel }` — edges of type `rel` ([`Dim::EdgeType`]) through nodes of any label,
///     which the reader's row-security view may hide ([`Dim::RowVisibility`]); a reached node
///     cannot appear or vanish without a `rel` edge being written or cascaded.
///   * `RankNodeDistance` / `RankMentions` — the untyped topology (`AllEdges`) plus visibility.
///   * `Rank` / `RankEmbed` / `RankMmr` — the embedding store: [`Dim::EmbeddingGeneration`] at
///     the live stamp `embedding_generation` (no stamp known ⇒ no dependency set).
///   * `FuseRrf` — every branch's reads, each branch starting from the fused input.
///
/// ANY other op — lexical `RankText` (BM25 over the whole corpus), a temporal `AsOf`, a
/// reasoner/SPARQL/federation/tensor/spatial/tsdb/epistemic leg — reads state the clock does not
/// model, so the WHOLE plan falls back. A dependency set is only ever returned when it PROVABLY
/// captures everything the query reads: a stale hit is a correctness bug.
#[cfg(feature = "result-cache")]
pub(crate) fn plan_dependency_set(
    plan: &medulla_exec::algebra::Plan,
    embedding_generation: Option<u64>,
) -> Option<medulla_query_cache::dep_scope::DepSet> {
    let mut walk = DepWalk {
        dims: Vec::new(),
        has_source: false,
        rows_unscoped: false,
        embedding_generation,
    };
    walk.ops(&plan.ops)?;
    // A plan with no graph SOURCE op (e.g. a pure federation/tsdb seed) is not a bounded graph
    // read — fall back rather than claim a dependency set.
    walk.has_source
        .then(|| medulla_query_cache::dep_scope::DepSet::new(walk.dims))
}

#[cfg(all(test, feature = "result-cache", feature = "query"))]
mod plan_deps_tests;

/// The accumulator [`plan_dependency_set`] threads through a plan (and each `FuseRrf` branch).
#[cfg(feature = "result-cache")]
struct DepWalk {
    dims: Vec<medulla_query_cache::dep_scope::Dim>,
    has_source: bool,
    /// The current rows may be nodes of ANY label (a traversal reached them).
    rows_unscoped: bool,
    embedding_generation: Option<u64>,
}

#[cfg(feature = "result-cache")]
impl DepWalk {
    fn ops(&mut self, ops: &[medulla_exec::algebra::Op]) -> Option<()> {
        ops.iter().try_for_each(|op| self.op(op))
    }

    fn op(&mut self, op: &medulla_exec::algebra::Op) -> Option<()> {
        use medulla_query_cache::dep_scope::Dim;
        match op {
            medulla_exec::algebra::Op::Scan { label } => self.source(label),
            medulla_exec::algebra::Op::ScanAll {} => self.source(""),
            medulla_exec::algebra::Op::Filter { .. } if self.rows_unscoped => self.dims.push(Dim::AllNodes),
            medulla_exec::algebra::Op::Filter { .. }
            | medulla_exec::algebra::Op::Limit { .. }
            | medulla_exec::algebra::Op::Project { .. } => {}
            medulla_exec::algebra::Op::Traverse { rel, .. } => {
                self.dims.push(Dim::EdgeType(rel.clone()));
                self.dims.push(Dim::RowVisibility);
                self.rows_unscoped = true;
            }
            medulla_exec::algebra::Op::RankNodeDistance { .. } | medulla_exec::algebra::Op::RankMentions {} => {
                self.dims.push(Dim::AllEdges);
                self.dims.push(Dim::RowVisibility);
            }
            medulla_exec::algebra::Op::Rank { .. }
            | medulla_exec::algebra::Op::RankEmbed { .. }
            | medulla_exec::algebra::Op::RankMmr { .. } => {
                self.dims
                    .push(Dim::EmbeddingGeneration(self.embedding_generation?));
            }
            #[cfg(feature = "text")]
            medulla_exec::algebra::Op::FuseRrf { branches, .. } => self.fuse(branches)?,
            // Any op reading state outside the dependency clock's model ⇒ coarse fallback.
            _ => return None,
        }
        Some(())
    }

    fn source(&mut self, label: &str) {
        use medulla_query_cache::dep_scope::Dim;
        self.has_source = true;
        self.rows_unscoped = false;
        self.dims.push(if label.is_empty() {
            Dim::AllNodes
        } else {
            Dim::Label(label.to_string())
        });
    }

    /// Each branch runs over the SAME input rows; the fused rows are unscoped when any branch's
    /// are. A branch's own `Scan` does not make the plan sourced (the fused input is).
    #[cfg(feature = "text")]
    fn fuse(&mut self, branches: &[Vec<medulla_exec::algebra::Op>]) -> Option<()> {
        let input_unscoped = self.rows_unscoped;
        let has_source = self.has_source;
        let mut fused_unscoped = input_unscoped;
        for branch in branches {
            self.rows_unscoped = input_unscoped;
            self.ops(branch)?;
            fused_unscoped |= self.rows_unscoped;
        }
        self.rows_unscoped = fused_unscoped;
        self.has_source = has_source;
        Some(())
    }
}

/// Does `ops` reference a lexical text op — an `Op::RankText` at the top level or nested
/// inside an `Op::FuseRrf` branch (CONCEPT:MEDULLA-KG.query.served-text-index-binding)? Drives whether
/// `run_unified` builds+binds a served text index at all, so a non-text plan pays nothing.
#[cfg(all(feature = "query", feature = "text"))]
pub(crate) fn plan_needs_text(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_exec::algebra::Op::RankText { .. } => true,
        medulla_exec::algebra::Op::FuseRrf { branches, .. } => branches.iter().any(|b| plan_needs_text(b)),
        _ => false,
    })
}

/// Does `ops` reference `Op::SpatialScan` — at the top level or nested inside an
/// `Op::FuseRrf` branch (CONCEPT:MEDULLA-KG.storage.incremental-spatial, L37, mirroring `plan_needs_text`)?
/// Drives whether `run_unified` binds the served spatial index at all, so a non-spatial
/// plan pays nothing.
#[cfg(all(feature = "query", feature = "geo"))]
pub(crate) fn plan_needs_spatial(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_exec::algebra::Op::SpatialScan { .. } => true,
        medulla_exec::algebra::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|b| plan_needs_spatial(b))
        }
        _ => false,
    })
}

/// Does `ops` read the decision log — a `DECISIONS` source (EH-066), at the top level
/// or inside an `Op::FuseRrf` branch? Only then is the caller's log bound to the plan.
#[cfg(all(feature = "query", feature = "decide"))]
pub(crate) fn plan_needs_decisions(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_exec::algebra::Op::DecisionScan { .. } => true,
        #[cfg(feature = "text")]
        medulla_exec::algebra::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|b| plan_needs_decisions(b))
        }
        _ => false,
    })
}

/// Does `ops` hold a `SOURCE RELIABILITY` stage (EH-525), which learns from the caller's
/// decision log when one can be bound — and keeps its belief-graph prior when not?
#[cfg(all(feature = "query", feature = "decide"))]
pub(crate) fn plan_reads_reputation(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(is_reliability_stage)
}

#[cfg(all(feature = "query", feature = "decide", feature = "epistemic"))]
fn is_reliability_stage(op: &medulla_exec::algebra::Op) -> bool {
    matches!(op, medulla_exec::algebra::Op::SourceReliability { .. })
}

#[cfg(all(feature = "query", feature = "decide", not(feature = "epistemic")))]
fn is_reliability_stage(_op: &medulla_exec::algebra::Op) -> bool {
    false
}

/// Build a BM25 [`medulla_text::TextIndex`] from a graph snapshot's node blobs
/// (CONCEPT:MEDULLA-KG.query.served-text-index-binding) — the served lexical index for `Op::RankText` /
/// `Op::FuseRrf`. Each node's indexable text is the concatenation of every STRING leaf in its
/// JSON property blob (so `name` / `description` / `text` / `content` / `title` / … are all
/// searchable, matching the human-readable fields `Discover` hydrates), keyed by node id. An
/// in-memory index (no persist dir needed — it is rebuilt per served text query off the exact
/// queried snapshot, so it is always current with the read). Returns `None` on a build/commit
/// error (the plan then degrades to no lexical hits — never errs), mirroring an absent index.
#[cfg(all(feature = "query", feature = "text"))]
pub(crate) fn build_text_index_from_view(
    view: &medulla_graph_core::graph::GraphView,
) -> Option<medulla_text::TextIndex> {
    /// Append every string leaf in `v` (recursing objects/arrays) to `out`, space-separated.
    fn collect_strings(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(s);
            }
            serde_json::Value::Array(a) => a.iter().for_each(|e| collect_strings(e, out)),
            serde_json::Value::Object(o) => o.values().for_each(|e| collect_strings(e, out)),
            _ => {}
        }
    }
    let mut index = medulla_text::TextIndex::in_memory().ok()?;
    for (id, blob) in &view.node_properties {
        let Ok(v) = medulla_types::msgpack::decode_property_value(blob.as_slice()) else {
            continue;
        };
        let mut text = String::new();
        collect_strings(&v, &mut text);
        if !text.is_empty() {
            index.upsert(id, &text);
        }
    }
    index.commit().ok()?;
    Some(index)
}

/// Bundles the served-adapter modality indexes `run_unified` pushes a plan's legs down
/// into (CONCEPT:MEDULLA-KG.query.served-text-index-binding / CONCEPT:MEDULLA-KG.storage.incremental-spatial) — ONE parameter
/// rather than one per modality, so `run_unified`'s argument count stays sane as more
/// served-index modalities are added over time (L37 added spatial beside text; a future
/// modality adds a field here, not a new top-level parameter). Each field is `Some` and
/// `.available()` ⇒ the matching op searches the MAINTAINED persistent index directly, NO
/// per-query rebuild; otherwise `run_unified` falls back to the prior behavior (a
/// snapshot-derived index for text, an unbound `PlanCtx::spatial` — ephemeral R-tree — for
/// spatial), exactly as if this bundle were never passed.
#[cfg(feature = "query")]
#[derive(Default)]
pub struct ServedIndexes<'a> {
    #[cfg(feature = "text")]
    pub text: Option<&'a crate::secondary_indexes::ServedTextIndex>,
    #[cfg(feature = "geo")]
    pub spatial: Option<&'a crate::secondary_indexes::ServedSpatialIndex>,
    /// CONCEPT:MEDULLA-KG.query.closure-backed-source — the CALLER'S owner-scoped foreign
    /// registry (EH-373), built by
    /// [`medulla_server_api::api::query_txn::foreign_catalog::ForeignSourceCatalog::registry_for`] from only the
    /// caller's own (tenant+principal) `RegisterForeignSource` entries. An `Op::Foreign`
    /// (the UQL `FOREIGN "<name>"` marker) / a `Named` `Op::ForeignScan` resolves through
    /// it, so another principal's source name resolves as not-registered. `None` ⇒ no registry is
    /// bound and a name-resolving op stays a clean typed error — never a silent empty
    /// set, never silently-local rows.
    #[cfg(feature = "federation")]
    pub foreign: Option<&'a medulla_federation::federation::ForeignSourceRegistry>,
    /// EH-196 — the graph's composed GraphSchema SHACL shapes
    /// (`handlers::rdf::ServedShapes`), which a `VALIDATE SHAPE` stage without its own
    /// `USING` document validates against. `None` ⇒ such a stage is a typed error.
    #[cfg(all(feature = "shacl", feature = "owl-plan"))]
    pub shapes: Option<&'a dyn medulla_exec::exec::ShapeSource>,
    // Keeps `'a` used even when neither `text` nor `geo` is built, so `ServedIndexes<'_>`
    // stays a valid (zero-field-active) type in every feature combination.
    #[cfg(not(any(feature = "text", feature = "geo")))]
    pub _marker: std::marker::PhantomData<&'a ()>,
}

/// A graph's maintained secondary indexes, opened where a plan runs (inside
/// the off-lock closure) and lent to `run_unified` as [`ServedIndexes`]: the
/// one construction every served plan path shares (UnifiedQuery, NL, in-txn
/// overlay, wire UQL, mining-sourced plans).
pub struct CoreIndexes<'c> {
    #[cfg(feature = "text")]
    text: crate::secondary_indexes::ServedTextIndex,
    #[cfg(feature = "geo")]
    spatial: crate::secondary_indexes::ServedSpatialIndex,
    #[cfg(all(feature = "shacl", feature = "owl-plan"))]
    shapes: crate::rdf::ServedShapes<'c>,
    core: std::marker::PhantomData<&'c ()>,
}

impl<'c> CoreIndexes<'c> {
    pub fn open(core: &'c std::sync::Arc<medulla_graph_core::graph::GraphCore>) -> Self {
        #[cfg(not(any(
            feature = "text",
            feature = "geo",
            all(feature = "shacl", feature = "owl-plan")
        )))]
        let _ = core;
        Self {
            #[cfg(feature = "text")]
            text: crate::secondary_indexes::ServedTextIndex::new(core.clone()),
            #[cfg(feature = "geo")]
            spatial: crate::secondary_indexes::ServedSpatialIndex::new(core.clone()),
            #[cfg(all(feature = "shacl", feature = "owl-plan"))]
            shapes: crate::rdf::ServedShapes::new(core),
            core: std::marker::PhantomData,
        }
    }

    /// These indexes as `run_unified` takes them, with the caller's owner-scoped
    /// foreign registry (EH-373) when the plan names a foreign source.
    pub fn served<'a>(
        &'a self,
        #[cfg(feature = "federation")] foreign: Option<
            &'a medulla_federation::federation::ForeignSourceRegistry,
        >,
    ) -> ServedIndexes<'a> {
        ServedIndexes {
            #[cfg(feature = "text")]
            text: Some(&self.text),
            #[cfg(feature = "geo")]
            spatial: Some(&self.spatial),
            #[cfg(feature = "federation")]
            foreign,
            #[cfg(all(feature = "shacl", feature = "owl-plan"))]
            shapes: Some(&self.shapes),
            #[cfg(not(any(feature = "text", feature = "geo")))]
            _marker: std::marker::PhantomData,
        }
    }
}

/// Leg binding with a caller-chosen `finish` over the fully bound
/// `PlanCtx` — `UnifiedQuery` executes and projects rows; `Method::Uql` runs a whole
/// statement (EXPLAIN/PROFILE/channels/DAG) over the SAME bindings (UQL-07/08/09).
/// `plan` decides which legs are bound (every op of the statement).
#[cfg(feature = "query")]
pub fn run_unified_with<T>(
    plan: medulla_exec::algebra::Plan,
    view: &medulla_graph_core::graph::GraphView,
    semantic: &medulla_semantic_index::semantic::SemanticStore,
    served: ServedIndexes<'_>,
    #[cfg(feature = "tsdb")] tsdb_ctx: TsdbLegBind<'_>,
    finish: impl FnOnce(&medulla_exec::algebra::Plan, &medulla_exec::exec::PlanCtx) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(feature = "tsdb")]
    let TsdbLegBind {
        tsdb,
        tsdb_tenant,
        tsdb_graph,
        staged_series,
    } = tsdb_ctx;
    #[cfg(feature = "text")]
    let served_text = served.text;
    #[cfg(feature = "geo")]
    let served_spatial = served.spatial;
    #[cfg(all(feature = "shacl", feature = "owl-plan"))]
    let served_shapes = served.shapes;
    #[cfg(not(any(feature = "text", feature = "geo", feature = "federation")))]
    let ServedIndexes { .. } = served;
    use medulla_exec::exec::PlanCtx;

    // CONCEPT:MEDULLA-KG.query.served-plan-optimize-routing — the served path hands the plan
    // directly to `medulla_plan::execute`, which applies the complete cost optimizer using
    // snapshot-derived cardinality and cost statistics. Optimizer rules are
    // answer-preserving within the MEDULLA-405 non-empty guard.
    let ops = plan.ops;
    // EH-563 — one federation-optimizer session per served query that touches a foreign
    // source: the server budget, the plan's LIMIT hints and the per-fragment trace.
    #[cfg(feature = "federation")]
    let federation =
        plan_touches_foreign(&ops).then(medulla_federation::federation_opt::FederationSession::from_env);

    // CONCEPT:MEDULLA-KG.query.served-text-index-binding — bind a live BM25 lexical search surface into the
    // served `PlanCtx` so a served `UnifiedQuery`/`Uql` whose plan carries
    // `Op::RankText` or an `Op::FuseRrf` text branch gets REAL lexical scores (it
    // previously always rebuilt a throwaway index from the queried snapshot on EVERY
    // request — the MEDULLA-P1-4 gap). Preference order:
    //   1. the MAINTAINED persistent per-graph `GraphTextIndex`, via `served_text`, when
    //      one is registered — no per-query rebuild, and it reflects every committed
    //      write incrementally (CONCEPT:MEDULLA-KG.storage.incremental-text);
    //   2. a snapshot-derived `medulla_text::TextIndex` built from `view` (the PRIOR
    //      behavior), for a graph with no `ServerIndexFactory` installed (a bare test
    //      harness, or a graph that predates the factory).
    // Built/bound ONLY when the plan actually references a text op, so a non-text
    // served query pays nothing either way.
    let ctx = PlanCtx::new(view, semantic);
    #[cfg(feature = "text")]
    let need_text = plan_needs_text(&ops);
    #[cfg(feature = "text")]
    let persistent_text = served_text.filter(|st| st.available());
    #[cfg(feature = "text")]
    let snapshot_text_index: Option<medulla_text::TextIndex> =
        if need_text && persistent_text.is_none() {
            build_text_index_from_view(view)
        } else {
            None
        };
    #[cfg(feature = "text")]
    let ctx = if !need_text {
        ctx
    } else if let Some(served) = persistent_text {
        ctx.with_text(served)
    } else if let Some(index) = snapshot_text_index.as_ref() {
        ctx.with_text(index)
    } else {
        ctx
    };
    // CONCEPT:MEDULLA-KG.storage.incremental-spatial, L37 — bind a persistent spatial index into the served `PlanCtx` so a
    // served `Op::SpatialScan` gets pushed into the MAINTAINED per-graph
    // `GraphSpatialIndex` instead of rebuilding a throwaway packed Hilbert R-tree on
    // EVERY request. `None` (no factory installed, or the plan has no spatial op) keeps
    // `spatial_scan`'s prior ephemeral-build fallback — byte-for-byte the old behavior.
    #[cfg(feature = "geo")]
    let ctx = run_unified_bind_spatial(ctx, &ops, served_spatial);
    // CONCEPT:MEDULLA-KG.query.closure-backed-source — bind the caller's owner-scoped
    // foreign registry so a served `Op::Foreign` (`FOREIGN "<name>"`) / a `Named`
    // `Op::ForeignScan` resolves the caller's OWN registered sources (EH-373).
    #[cfg(feature = "federation")]
    let federated =
        medulla_federation::federation_opt::FederatedExec::new(served.foreign, federation.as_ref());
    let ctx = run_unified_bind_foreign(ctx, &federated);
    // CONCEPT:MEDULLA-KG.query.bind-server-side-text — bind the server-side text→vector embedder so a UQL `RANK BY ~ "text"`
    // (`Op::RankEmbed`) resolves its query vector at exec time (the NL→vector seam,
    // MEDULLA-411). This is the facade INJECTION POINT: the engine stores embeddings but
    // produces them CLIENT-side today (no in-process model), so a real embedding model — an
    // ONNX/remote embedding-service impl of `medulla_plan::TextEmbedder` producing vectors in the
    // graph's embedding space — is bound HERE. Absent a bound model an `Op::RankEmbed` is a
    // clean typed error (never a panic), exactly the documented unbound behavior. The
    // deterministic `HashEmbedder` fallback is opt-in via `MEDULLA_UQL_TEXT_EMBEDDER=hash` so the
    // seam is exercisable end-to-end offline (its ranking is deterministic but semantically
    // arbitrary — never the production default).
    let ctx = run_unified_bind_embedder(ctx);
    // RECONCILE (CONCEPT:MEDULLA-KG.query.native-time-series): attach the committed
    // store and its ownership scope atomically, plus the txn's staged-series
    // overlay (CONCEPT:MEDULLA-KG.query.txn-tsdb-read-your). A partial/missing scope never
    // leaves a raw store reachable through `TsScan`.
    #[cfg(feature = "tsdb")]
    let ctx = run_unified_bind_tsdb(ctx, tsdb, tsdb_tenant, tsdb_graph, staged_series);
    // CONCEPT:MEDULLA-KG.storage.derived-tensor-writeback-sink — bind the tensor CAS
    // write-back sink so a served `Op::TensorOp` actually runs instead of its
    // documented-but-unreachable "TensorOp requires a bound tensor store" error.
    #[cfg(feature = "tensor")]
    let ctx = run_unified_bind_tensor(ctx);
    #[cfg(all(feature = "shacl", feature = "owl-plan"))]
    let ctx = match served_shapes {
        Some(shapes) => ctx.with_shape_source(shapes),
        None => ctx,
    };
    let result = finish(&medulla_exec::algebra::Plan::new(ops), &ctx);
    #[cfg(feature = "federation")]
    log_federation_trace(federation.as_ref());
    result
}

/// The `Op::SpatialScan` leg-binding of [`run_unified_with`] (CONCEPT:MEDULLA-KG.storage.incremental-spatial, L37): bind a
/// persistent spatial index into the served `PlanCtx` only when the plan needs one
/// and a live, available index was supplied — otherwise keep `spatial_scan`'s
/// prior ephemeral-build fallback, byte-for-byte the old behavior.
#[cfg(feature = "geo")]
pub(crate) fn run_unified_bind_spatial<'a>(
    ctx: medulla_exec::exec::PlanCtx<'a>,
    ops: &[medulla_exec::algebra::Op],
    served_spatial: Option<&'a crate::secondary_indexes::ServedSpatialIndex>,
) -> medulla_exec::exec::PlanCtx<'a> {
    if !plan_needs_spatial(ops) {
        return ctx;
    }
    match served_spatial.filter(|s| s.available()) {
        Some(served) => ctx.with_spatial(served),
        None => ctx,
    }
}

/// Does `ops` read any foreign source (named or inline, top level or inside an
/// `Op::FuseRrf` branch)? Such a plan runs under a federation-optimizer session (EH-563).
#[cfg(all(feature = "query", feature = "federation"))]
fn plan_touches_foreign(ops: &[medulla_exec::algebra::Op]) -> bool {
    ops.iter().any(|op| match op {
        medulla_exec::algebra::Op::Foreign { .. } | medulla_exec::algebra::Op::ForeignScan { .. } => true,
        medulla_exec::algebra::Op::FuseRrf { branches, .. } => {
            branches.iter().any(|b| plan_touches_foreign(b))
        }
        _ => false,
    })
}

/// Log what one served federated query moved, fragment by fragment (EH-563 §7).
#[cfg(all(feature = "query", feature = "federation"))]
fn log_federation_trace(session: Option<&medulla_federation::federation_opt::FederationSession>) {
    let Some(trace) = session.map(|s| s.trace()).filter(|t| !t.is_empty()) else {
        return;
    };
    tracing::info!(
        fragments = trace.len(),
        trace = %medulla_federation::federation_opt::render_trace(&trace),
        "federated query (EH-563)"
    );
}

/// The `Op::Foreign`/`Op::ForeignScan` leg-binding of [`run_unified_with`]
/// (CONCEPT:MEDULLA-KG.query.closure-backed-source): attach the caller's owner-scoped registry and
/// the query's federation-optimizer session (EH-563), each if any.
#[cfg(feature = "federation")]
pub(crate) fn run_unified_bind_foreign<'a>(
    ctx: medulla_exec::exec::PlanCtx<'a>,
    federated: &'a medulla_federation::federation_opt::FederatedExec<'a>,
) -> medulla_exec::exec::PlanCtx<'a> {
    if let Some(session) = federated.session {
        session.set_cache_scope(
            federated
                .registry
                .and_then(|registry| registry.cache_scope().cloned()),
        );
    }
    ctx.with_foreign_executor(federated)
}

/// The `Op::RankEmbed` leg-binding of [`run_unified_with`] (CONCEPT:MEDULLA-KG.query.bind-server-side-text): attach the
/// server-side text→vector embedder, if one is bound (`MEDULLA_UQL_TEXT_EMBEDDER=hash`
/// for the deterministic offline fallback; otherwise absent, and `Op::RankEmbed` is
/// a clean typed error).
#[cfg(feature = "query")]
pub(crate) fn run_unified_bind_embedder(
    ctx: medulla_exec::exec::PlanCtx<'_>,
) -> medulla_exec::exec::PlanCtx<'_> {
    match uql_text_embedder() {
        Some(embedder) => ctx.with_embedder(embedder),
        None => ctx,
    }
}

/// The `Op::TsScan` leg-binding of [`run_unified_with`] (CONCEPT:MEDULLA-KG.query.native-time-series): attach the committed
/// store and its ownership scope atomically (a partial/missing scope never leaves
/// a raw store reachable through `TsScan`), then the txn's staged-series overlay
/// (CONCEPT:MEDULLA-KG.query.txn-tsdb-read-your) so an in-txn `TsScan` reads its own uncommitted points.
#[cfg(all(feature = "query", feature = "tsdb"))]
pub(crate) fn run_unified_bind_tsdb<'a>(
    ctx: medulla_exec::exec::PlanCtx<'a>,
    tsdb: Option<&'a medulla_tsdb::store::SeriesStore>,
    tsdb_tenant: Option<&'a str>,
    tsdb_graph: Option<&'a str>,
    staged_series: Option<&'a medulla_exec::exec::StagedSeries>,
) -> medulla_exec::exec::PlanCtx<'a> {
    let ctx = match (tsdb, tsdb_tenant, tsdb_graph) {
        (Some(store), Some(tenant), Some(graph)) => {
            ctx.with_tsdb(store).with_tsdb_scope(tenant, graph)
        }
        _ => ctx,
    };
    match staged_series {
        Some(staged) => ctx.with_staged_series(staged),
        None => ctx,
    }
}

/// The `Op::TensorOp` leg-binding of [`run_unified_with`] (CONCEPT:MEDULLA-KG.storage.derived-tensor-writeback-sink): bind the
/// tensor CAS write-back sink so a served `Op::TensorOp` actually runs instead of
/// its documented-but-unreachable "TensorOp requires a bound tensor store" error.
/// `Op::TensorScan`/`Op::TensorOp` read their INPUT tensor directly off the
/// queried `GraphView`'s node properties (`medulla_plan::exec::row_tensor`) — this
/// store is purely the write-back destination for a TensorOp's DERIVED output, so
/// a process-wide singleton (not threaded through `ServerState`/callers) is
/// sufficient and still gives real content-address dedup across requests, unlike a
/// fresh store per call. In-memory only for now — `TensorStore::persist`/`load`
/// (disk durability across restarts) is a follow-up, tracked the same way
/// `tsdb_store` earned its own dedicated `ServerState` wiring.
#[cfg(feature = "tensor")]
pub(crate) fn run_unified_bind_tensor(ctx: medulla_exec::exec::PlanCtx<'_>) -> medulla_exec::exec::PlanCtx<'_> {
    static TENSOR_STORE: std::sync::OnceLock<std::sync::Mutex<medulla_tensor::TensorStore>> =
        std::sync::OnceLock::new();
    let store =
        TENSOR_STORE.get_or_init(|| std::sync::Mutex::new(medulla_tensor::TensorStore::new()));
    ctx.with_tensor_store(store)
}

/// What a unified plan run hands back: the outer `Err` is a ready response
/// (off-lock execution failed), the inner result is the plan's scored rows or
/// its error message.
#[cfg(feature = "query")]
pub(crate) type UnifiedRunOutcome = Result<Result<Vec<(String, Option<f32>)>, String>, Response>;

/// The verified-carrier-scoped legs of one served unified plan, resolved once per
/// request before the result-cache probe: the tsdb `(tenant, namespace)` scope and the
/// caller's owner-scoped (tenant+principal) foreign-source registry (EH-373). Both are
/// derived from the verified read authority only, and both make a result
/// caller-specific, so
/// [`Self::salt_cache_key`] folds them into the result-cache key.
#[cfg(feature = "query")]
#[derive(Default)]
pub(crate) struct ServedPlanLegs {
    #[cfg(feature = "tsdb")]
    pub(crate) tsdb_scope: Option<(String, String)>,
    #[cfg(feature = "federation")]
    pub(crate) foreign:
        Option<medulla_server_ops::api::query_txn::foreign_catalog::OwnedForeignRegistry>,
    /// EH-066 — the caller's visible decision log, bound when the plan reads it.
    #[cfg(feature = "decide")]
    pub(crate) decisions: Option<Arc<dyn medulla_exec::exec::DecisionSource>>,
    /// EH-400 — whether the named foreign sources the plan reads are fresh, and their
    /// watermarks (see [`ForeignWatermarks`]).
    #[cfg(feature = "result-cache")]
    pub(crate) watermarks: ForeignWatermarks,
    /// EH-396 — the tenant's active query adapter for this graph, bound when the plan
    /// ranks by vector.
    #[cfg(feature = "decide")]
    pub(crate) adapter: Option<Arc<medulla_server_ops::api::admin::served_adapter::ServedAdapter>>,
}

#[cfg(feature = "query")]
impl ServedPlanLegs {
    /// Resolve both legs for `plan`; `Err` is the caller-facing refusal text.
    pub(crate) async fn resolve(
        state: &Arc<RwLock<ServerState>>,
        graph_name: &str,
        read_authority: Option<&GraphReadAuthority>,
        plan: &medulla_exec::algebra::Plan,
        foreign_execution: Option<&medulla_catalog_types::foreign_read::ForeignReadExecutionRef>,
        foreign_executions: &[medulla_catalog_types::foreign_read::ForeignReadExecutionRef],
    ) -> Result<Self, String> {
        #[cfg(not(feature = "tsdb"))]
        let _ = graph_name;
        #[cfg(not(any(feature = "federation", feature = "decide")))]
        let _ = state;
        #[cfg(not(any(feature = "tsdb", feature = "federation", feature = "decide")))]
        let _ = (read_authority, plan, foreign_execution, foreign_executions);
        Ok(Self {
            #[cfg(feature = "tsdb")]
            tsdb_scope: served_tsdb_scope(plan, graph_name, read_authority)?,
            #[cfg(feature = "federation")]
            foreign: served_foreign_leg(
                state,
                plan,
                read_authority,
                foreign_execution,
                foreign_executions,
            )
            .await?,
            #[cfg(feature = "decide")]
            decisions: served_decision_leg(state, plan, read_authority).await?,
            #[cfg(feature = "result-cache")]
            watermarks: ForeignWatermarks::NotRead,
            #[cfg(feature = "decide")]
            adapter: medulla_server_ops::api::admin::served_adapter::served_plan_adapter(
                state,
                graph_name,
                read_authority,
                plan,
            )
            .await,
        })
    }

    /// Whether the plan's answer may be cached: not when a leg reads state outside the
    /// graph version (the decision log changes without a graph write).
    #[cfg(feature = "result-cache")]
    pub(crate) fn cacheable(&self) -> bool {
        #[cfg(feature = "decide")]
        return self.decisions.is_none();
        #[cfg(not(feature = "decide"))]
        true
    }

    /// Whether the plan's answer may be cached at all: [`Self::cacheable`] AND every named
    /// foreign source it reads has a fresh watermark (EH-400).
    #[cfg(feature = "result-cache")]
    pub(crate) fn cache_admissible(&self) -> bool {
        self.cacheable() && self.watermarks.cacheable()
    }

    /// Append the foreign sources' watermarks to a result-cache key payload (EH-400), so a
    /// connector checkpoint advance retires the cached answer.
    #[cfg(feature = "result-cache")]
    pub(crate) fn salt_watermarks(&self, payload: &mut Vec<u8>) {
        self.watermarks.salt(payload);
    }

    /// Decide the watermark leg for `plan` against the queried graph's watermark nodes.
    #[cfg(feature = "result-cache")]
    fn with_watermarks(mut self, core: &GraphCore, plan: &medulla_exec::algebra::Plan) -> Self {
        self.watermarks = ForeignWatermarks::for_ops(core, &plan.ops);
        #[cfg(feature = "federation")]
        if let (Some(foreign), Some(sources)) = (
            self.foreign.as_mut(),
            ForeignWatermarks::fragment_sources(core, &plan.ops),
        ) {
            foreign.install_fragment_cache(sources);
        }
        self
    }

    #[cfg(not(feature = "result-cache"))]
    fn with_watermarks(self, _core: &GraphCore, _plan: &medulla_exec::algebra::Plan) -> Self {
        self
    }

    /// Append the tenant-specific parts of these legs to a result-cache key payload
    /// (the tsdb tenant + namespace first, byte-identical to the prior tsdb salt, then the
    /// foreign registry's digest of its owner and every resolved source, so a grant,
    /// revocation or re-registration never serves stale foreign rows).
    #[cfg(feature = "result-cache")]
    pub(crate) fn salt_cache_key(&self, payload: &mut Vec<u8>) {
        #[cfg(feature = "tsdb")]
        if let Some((tenant, graph)) = self.tsdb_scope.as_ref() {
            payload.extend_from_slice(tenant.as_bytes());
            payload.extend_from_slice(graph.as_bytes());
        }
        #[cfg(feature = "federation")]
        if let Some(foreign) = self.foreign.as_ref() {
            payload.extend_from_slice(foreign.cache_salt().as_bytes());
        }
        // EH-396: the active adapter re-aims vector ranks, so it keys the answer.
        #[cfg(feature = "decide")]
        medulla_server_ops::api::admin::served_adapter::salt(self.adapter.as_deref(), payload);
        #[cfg(not(any(feature = "tsdb", feature = "federation", feature = "decide")))]
        let _ = payload;
    }
}

/// The caller's visible decision log for a plan with a `DECISIONS` source (EH-066) or
/// a `SOURCE RELIABILITY` stage (EH-525), derived from the verified carrier only; `None`
/// for a plan that does not read it, and for a reputation-only plan without a carrier
/// or a log (the stage then keeps its belief-graph prior).
#[cfg(all(feature = "query", feature = "decide"))]
async fn served_decision_leg(
    state: &Arc<RwLock<ServerState>>,
    plan: &medulla_exec::algebra::Plan,
    read_authority: Option<&GraphReadAuthority>,
) -> Result<Option<Arc<dyn medulla_exec::exec::DecisionSource>>, String> {
    let required = plan_needs_decisions(&plan.ops);
    if !required && !plan_reads_reputation(&plan.ops) {
        return Ok(None);
    }
    let carrier = read_authority.and_then(GraphReadAuthority::carrier);
    let Some(carrier) = carrier else {
        if !required {
            return Ok(None);
        }
        medulla_kernel::metrics::access_denied();
        return Err("ACCESS_DENIED: DECISIONS requires a verified tenant carrier".to_string());
    };
    let source = match medulla_server_ops::api::admin::decide_seam(state).await {
        Some(decide) => decide.decision_source(state, carrier).await,
        None => None,
    };
    if source.is_none() && required {
        return Err("DECISIONS: the decision log is unavailable on this server".to_string());
    }
    Ok(source)
}

/// Parse UQL `text` into the `wire::Plan` every UQL front-end runs; a parse error is
/// the caret-annotated refusal response.
#[cfg(feature = "query")]
pub(crate) fn parse_uql(req_id: u64, text: &str) -> Result<medulla_exec::algebra::Plan, Response> {
    medulla_uql::parse(text)
        .map_err(|e| Response::err(req_id, format!("{}: {}", e.code, e.render(text))))
}

/// Digest convention for a host-driven UQL foreign read. Formatting and bound
/// parameter values are part of the query identity; the parsed plan digest is
/// checked separately by the served leg resolver.
#[cfg(feature = "query")]
pub fn foreign_uql_query_digest(
    text: &str,
    params: &std::collections::BTreeMap<String, medulla_types::wire::UqlParam>,
) -> Result<medulla_types::contract::Digest256, String> {
    let params_bytes = rmp_serde::to_vec_named(params)
        .map_err(|error| format!("INTERNAL: UQL parameter digest serialization failed: {error}"))?;
    medulla_types::contract::Digest256::framed(
        b"eg/foreign-read/uql-query/v1",
        &[text.as_bytes(), &params_bytes],
    )
}

#[cfg(all(test, feature = "query"))]
mod foreign_query_digest_tests {
    use super::foreign_uql_query_digest;
    use medulla_server_ops::api::query_txn::query_support::canonical_plan_digest;
    use std::collections::BTreeMap;

    #[test]
    fn plan_pin_is_available_without_federation() {
        let plan = medulla_uql::parse("MATCH (:Issue)").expect("valid plan");
        let bytes = rmp_serde::to_vec_named(&plan).expect("serializable plan");
        assert_eq!(
            canonical_plan_digest(&plan).expect("canonical digest"),
            medulla_types::contract::Digest256::sha256(&bytes)
        );
    }

    #[test]
    fn uql_query_digest_binds_text_and_typed_parameters() {
        let empty = BTreeMap::new();
        let baseline = foreign_uql_query_digest("MATCH (:Issue)", &empty).expect("digest");
        assert_eq!(
            baseline,
            foreign_uql_query_digest("MATCH (:Issue)", &empty).expect("same digest")
        );

        let mut params = BTreeMap::new();
        params.insert(
            "state".to_string(),
            medulla_types::wire::UqlParam::Str("open".to_string()),
        );
        let bound = foreign_uql_query_digest("MATCH (:Issue)", &params).expect("bound digest");
        assert_ne!(
            baseline, bound,
            "typed parameters are part of the query pin"
        );
        assert_ne!(
            bound,
            foreign_uql_query_digest("MATCH (:Issue) |> LIMIT 1", &params)
                .expect("different text digest"),
            "query text is part of the query pin"
        );
    }
}

#[cfg(feature = "query")]
impl QueryHandlerCtx<'_> {
    /// [`ServedPlanLegs::resolve`] for this request's verified read authority; `Err`
    /// is the caller-facing refusal response.
    pub(crate) async fn served_legs(
        &self,
        plan: &medulla_exec::algebra::Plan,
    ) -> Result<ServedPlanLegs, Response> {
        self.served_legs_with_foreign_execution(plan, None).await
    }

    pub(crate) async fn served_legs_with_foreign_execution(
        &self,
        plan: &medulla_exec::algebra::Plan,
        foreign_execution: Option<&medulla_catalog_types::foreign_read::ForeignReadExecutionRef>,
    ) -> Result<ServedPlanLegs, Response> {
        self.served_legs_with_foreign_executions(plan, foreign_execution, &[])
            .await
    }

    pub(crate) async fn served_legs_with_foreign_executions(
        &self,
        plan: &medulla_exec::algebra::Plan,
        foreign_execution: Option<&medulla_catalog_types::foreign_read::ForeignReadExecutionRef>,
        foreign_executions: &[medulla_catalog_types::foreign_read::ForeignReadExecutionRef],
    ) -> Result<ServedPlanLegs, Response> {
        ServedPlanLegs::resolve(
            self.state,
            self.graph_name,
            self.read_authority,
            plan,
            foreign_execution,
            foreign_executions,
        )
        .await
        .map(|legs| {
            if foreign_execution.is_some() || !foreign_executions.is_empty() {
                // Page-backed executions are request-scoped; catalog watermarks
                // cannot prove their transient row buffers are cache-equivalent.
                #[allow(unused_mut)]
                let mut legs = legs;
                #[cfg(feature = "result-cache")]
                {
                    legs.watermarks = ForeignWatermarks::Unfresh;
                }
                legs
            } else {
                legs.with_watermarks(self.core, plan)
            }
        })
        .map_err(|denied| Response::err(self.req_id, denied))
    }
}

/// Resolve the tsdb/text/geo/federation legs and run `plan` off-lock via
/// `run_unified`, exactly as `UnifiedQuery`/`Uql`/`NlQuery` already
/// did inline — pure extract-method out of those three arms' bodies (identical
/// duplicated code in each), no behaviour change. Returns `compute_off_lock`'s
/// raw nested result unchanged; callers keep doing their own
/// `Ok(Ok(rows))`/`Ok(Err(msg))`/`Err(resp)` handling (result-cache storage and
/// the error message text differ per caller, so that stays out of here).
#[cfg(feature = "query")]
pub(crate) async fn run_unified_off_lock(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    core: &Arc<GraphCore>,
    snap: Arc<medulla_graph_core::graph::GraphView>,
    plan: medulla_exec::algebra::Plan,
    legs: ServedPlanLegs,
) -> UnifiedRunOutcome {
    run_unified_off_lock_with(state, req_id, core, snap, plan, legs, execute_rows).await
}

/// [`run_unified_off_lock`] with a caller-chosen finisher (see [`run_unified_with`]).
#[cfg(feature = "query")]
pub(crate) async fn run_unified_off_lock_with<T, F>(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    core: &Arc<GraphCore>,
    snap: Arc<medulla_graph_core::graph::GraphView>,
    plan: medulla_exec::algebra::Plan,
    legs: ServedPlanLegs,
    finish: F,
) -> Result<Result<T, String>, Response>
where
    T: Send + 'static,
    F: FnOnce(&medulla_exec::algebra::Plan, &medulla_exec::exec::PlanCtx) -> Result<T, String> + Send + 'static,
{
    let core_for_ctx = core.clone();
    let ServedPlanLegs {
        #[cfg(feature = "tsdb")]
        tsdb_scope,
        #[cfg(feature = "federation")]
        foreign,
        #[cfg(feature = "decide")]
        decisions,
        #[cfg(feature = "decide")]
        adapter,
        ..
    } = legs;
    #[cfg(feature = "tsdb")]
    let (tsdb, tsdb_tenant, tsdb_graph) = bound_tsdb_leg(state, tsdb_scope).await;
    #[cfg(not(feature = "tsdb"))]
    let _ = state;
    compute_off_lock(req_id, move || {
        #[cfg(feature = "decide")]
        let finish = with_decision_log(decisions, finish);
        // EH-396: the tenant's adapter re-aims every vector rank of the plan.
        #[cfg(feature = "decide")]
        let plan = medulla_server_ops::api::admin::served_adapter::adapt_plan(
            plan,
            adapter,
            uql_text_embedder(),
        );
        run_unified_with_staged_finish(
            plan,
            &snap,
            &core_for_ctx,
            &[],
            #[cfg(feature = "federation")]
            bound_registry(&foreign),
            #[cfg(feature = "tsdb")]
            TsdbLegBind {
                tsdb: tsdb.as_deref(),
                tsdb_tenant: tsdb_tenant.as_deref(),
                tsdb_graph: tsdb_graph.as_deref(),
                // Off-txn: no staged-series overlay (CONCEPT:MEDULLA-KG.query.txn-tsdb-read-your).
                staged_series: None,
            },
            finish,
        )
    })
    .await
}

#[cfg(feature = "tsdb")]
pub(crate) async fn bound_tsdb_leg(
    state: &Arc<RwLock<ServerState>>,
    scope: Option<(String, String)>,
) -> (
    Option<Arc<medulla_tsdb::store::SeriesStore>>,
    Option<String>,
    Option<String>,
) {
    let store = if scope.is_some() {
        state.read().await.tsdb_store.clone()
    } else {
        None
    };
    let (tenant, graph) = scope.map_or((None, None), |(tenant, graph)| (Some(tenant), Some(graph)));
    (store, tenant, graph)
}

/// Wrap `finish` so it runs over a ctx with the caller's decision log bound (EH-066).
#[cfg(all(feature = "query", feature = "decide"))]
fn with_decision_log<T>(
    log: Option<Arc<dyn medulla_exec::exec::DecisionSource>>,
    finish: impl FnOnce(&medulla_exec::algebra::Plan, &medulla_exec::exec::PlanCtx) -> Result<T, String>,
) -> impl FnOnce(&medulla_exec::algebra::Plan, &medulla_exec::exec::PlanCtx) -> Result<T, String> {
    move |plan, ctx| match log.as_deref() {
        Some(log) => finish(plan, &ctx.clone().with_decisions(log)),
        None => finish(plan, ctx),
    }
}

// EH-563 — the served path runs foreign leaves through the federation optimizer.
#[cfg(all(test, feature = "federation"))]
mod federation_served_tests;

/// CONCEPT:MEDULLA-KG.storage.derived-tensor-writeback-sink — served-path proof that
/// `run_unified` (not just `medulla-plan`'s own internal executor, already proven by
/// `crates/medulla-plan/src/tensor_tests.rs`) now binds a tensor store: an
/// `Op::TensorScan` + `Op::TensorOp` plan run through the SAME entry point every
/// `UnifiedQuery`/`Uql` request uses now executes and returns rows
/// instead of the "TensorOp requires a bound tensor store" error `run_unified`
/// deterministically returned before the `.with_tensor_store(...)` binding was
/// added.
#[cfg(all(test, feature = "tensor"))]
mod tensor_served_round_trip_tests {
    use super::*;
    use medulla_semantic_index::semantic::SemanticStore;
    use medulla_graph_core::graph::GraphCore;
    use medulla_tensor::{Buffer, Tensor};
    use medulla_types::wire::{TensorOpKind, TensorReduceKind};

    fn blob(v: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&v).unwrap()
    }

    /// A `Frame` layer of three nodes each holding the same dense 2×3 tensor in
    /// their conventional `tensor` property, mirroring
    /// `medulla_plan::tensor_tests::frames()`.
    fn frames_view() -> medulla_graph_core::graph::GraphView {
        let core = GraphCore::new();
        let t = Tensor::new(vec![2, 3], Buffer::F32(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0])).unwrap();
        let tv = serde_json::to_value(&t).unwrap();
        for id in ["F1", "F2", "F3"] {
            core.add_node(
                id.into(),
                blob(serde_json::json!({ "type": "Frame", "tensor": tv })),
            );
        }
        core.analysis_snapshot()
    }

    fn served_indexes() -> ServedIndexes<'static> {
        ServedIndexes {
            #[cfg(feature = "text")]
            text: None,
            #[cfg(feature = "geo")]
            spatial: None,
            #[cfg(feature = "federation")]
            foreign: None,
            #[cfg(all(feature = "shacl", feature = "owl-plan"))]
            shapes: None,
            #[cfg(not(any(feature = "text", feature = "geo")))]
            _marker: std::marker::PhantomData,
        }
    }

    fn call_run_unified(plan: medulla_exec::algebra::Plan) -> Result<Vec<(String, Option<f32>)>, String> {
        let view = frames_view();
        let semantic = SemanticStore::new();
        run_unified_with(
            plan,
            &view,
            &semantic,
            served_indexes(),
            #[cfg(feature = "tsdb")]
            TsdbLegBind {
                tsdb: None,
                tsdb_tenant: None,
                tsdb_graph: None,
                staged_series: None,
            },
            execute_rows,
        )
    }

    #[test]
    fn served_tensor_scan_and_op_executes_instead_of_erroring() {
        let plan = medulla_exec::algebra::Plan::new(vec![
            medulla_exec::algebra::Op::TensorScan {
                layer: "Frame".into(),
            },
            medulla_exec::algebra::Op::TensorOp {
                kind: TensorOpKind::Reduce {
                    axis: 1,
                    kind: TensorReduceKind::Mean,
                },
            },
        ]);
        let rows = call_run_unified(plan).expect(
            "served TensorOp must execute now that run_unified binds a tensor store, \
             not error with 'TensorOp requires a bound tensor store'",
        );
        let mut ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["F1", "F2", "F3"]);
    }

    /// Before the fix, `run_unified` had no `tensor_store` binding at all, so this
    /// exact plan deterministically failed with "TensorOp requires a bound tensor
    /// store" regardless of input — the gap this test closes.
    #[test]
    fn served_tensor_op_without_the_fix_would_have_errored() {
        let plan = medulla_exec::algebra::Plan::new(vec![
            medulla_exec::algebra::Op::TensorScan {
                layer: "Frame".into(),
            },
            medulla_exec::algebra::Op::TensorOp {
                kind: TensorOpKind::Elementwise {
                    op: medulla_types::wire::TensorElementwiseOp::Mul,
                    scalar: 2.0,
                },
            },
        ]);
        assert!(
            call_run_unified(plan).is_ok(),
            "TensorOp over the served path must not deterministically error"
        );
    }
}
