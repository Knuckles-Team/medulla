//! Verified carrier and graph-read authority: tenant, actor and row-level visibility derived from a [`VerifiedRequestContext`].

use crate::authority_context::VerifiedRequestContext;
use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::graph::GraphView;
#[cfg(feature = "security")]
use medulla_graph_core::rls::row_visibility;
use medulla_isolation::isolation::IsolationLayer;
use medulla_types::contract::{Digest256, Nonce};
use std::sync::Arc;

/// Verified ownership carried into stores that are not naturally graph-scoped.
///
/// A method body's `tenant`, `actor`, namespace, cursor, or job id is never an
/// authority claim.  This object can only be derived from the verified v2 request
/// context and supplies stable opaque tenant/actor keys for durable ownership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarrierAuthority {
    tenant_scope: String,
    actor_scope: String,
    /// The verified tenant id and principal persistence id exactly as the Agent
    /// Library keys a tenant's decision log and its principal-visible records (EH-066).
    log_owner: (String, String),
    owner_scope: String,
    agent_id: String,
    /// Digest of the verified delegation chain. Owner scope intentionally
    /// remains the durable tenant/principal catalog key; foreign executions
    /// bind this effective actor/delegation identity separately.
    delegation_digest: Digest256,
    /// Digest of the verified authority fields used to open a request-scoped
    /// foreign execution. It excludes attempt nonce/idempotency so each phase
    /// can use a fresh authenticated envelope while retaining the same grant.
    authority_digest: Digest256,
    /// Per-request replay identity copied from the already-verified envelope.
    /// Stable carrier ownership fields above remain independent of this
    /// attempt-scoped value, but self-routed stores still need both pieces to
    /// compile the kernel-owned mutation envelope.
    attempt_nonce: Option<Nonce>,
    idempotency_key: String,
    admin: bool,
    can_read: bool,
    can_write: bool,
    /// The verified identity actor: the identity store decides from it, against
    /// its current state, whether its SQL relations are visible.
    #[cfg(feature = "security")]
    identity_actor: medulla_identity_types::identity::IdentityActor,
}

impl CarrierAuthority {
    /// A verified carrier for `agent` in the deployment's one tenant (the owner-scoped
    /// catalog test fixtures).
    #[cfg(all(any(test, feature = "test-support"), any(feature = "federation", feature = "wasm-udf")))]
    pub fn verified_for_test(agent: &str) -> Self {
        Self::from_verified(&VerifiedRequestContext::verified_for_test(agent))
            .expect("verified test carrier")
    }

    pub fn from_verified(context: &VerifiedRequestContext) -> Result<Self, String> {
        let tenant = context.tenant().trim();
        let principal = context.principal().trim();
        let agent_id = context.agent_id().trim();
        if tenant.is_empty() || principal.is_empty() || agent_id.is_empty() {
            medulla_kernel::metrics::access_denied();
            return Err(
                "ACCESS_DENIED: verified data carrier is missing tenant, principal, or actor"
                    .to_string(),
            );
        }
        let tenant_scope = if opaque_scope(tenant, "carrier-tenant") {
            tenant.to_string()
        } else {
            crate::batch_compile::opaque_coordinator_key("carrier-tenant", "verified", tenant)
        };
        let actor_scope = if opaque_scope(principal, "principal:sha256") {
            principal.to_string()
        } else {
            context.principal_persistence_id()
        };
        let owner_scope = crate::batch_compile::opaque_coordinator_key(
            "carrier-owner",
            &tenant_scope,
            &actor_scope,
        );
        let delegation_fields = context
            .claims()
            .delegation
            .iter()
            .map(String::as_bytes)
            .collect::<Vec<_>>();
        let delegation_digest = Digest256::framed(b"eg/carrier-delegation/v1", &delegation_fields)?;
        // Roles and scopes are set-like claims. Canonicalize their order before
        // deriving the phase binding so equivalent verified contexts have one
        // authority digest while the effective actor and delegation remain
        // explicit inputs.
        let mut roles = context.claims().roles.clone();
        roles.sort();
        let mut scopes = context.claims().scopes.clone();
        scopes.sort();
        let mut authority_fields: Vec<&[u8]> = vec![
            tenant.as_bytes(),
            principal.as_bytes(),
            agent_id.as_bytes(),
            tenant_scope.as_bytes(),
            actor_scope.as_bytes(),
            delegation_digest.as_bytes(),
            context.claims().policy_version.as_bytes(),
        ];
        authority_fields.extend(roles.iter().map(String::as_bytes));
        authority_fields.extend(scopes.iter().map(String::as_bytes));
        let authority_digest = Digest256::framed(b"eg/carrier-authority/v1", &authority_fields)?;
        let scopes = &context.claims().scopes;
        let admin = scopes
            .iter()
            .any(|scope| scope == "*" || scope == "kg:admin");
        // Coarse per-verb capability, independent of `admin`: every existing
        // carrier-minting site (SigV4, KV-cache, native SQL, broker, ...) mints
        // BOTH `kg:read` and `kg:write` unconditionally, so these two remain
        // `true` for all of them exactly as before this field existed. The
        // Iceberg-REST bearer path (`auth::authenticated_iceberg_bearer`,
        // NE-048) is the first minting site whose scopes are NOT hardcoded --
        // they are projected from the verified bearer's own claim -- so a
        // `kg:read`-only bearer now yields `can_write() == false` here, closing
        // the P0 privilege escalation where every Iceberg-REST bearer silently
        // received write authority regardless of what it was actually issued.
        let can_read = admin || scopes.iter().any(|scope| scope == "kg:read");
        let can_write = admin || scopes.iter().any(|scope| scope == "kg:write");
        Ok(Self {
            log_owner: (
                context.tenant().to_string(),
                context.principal_persistence_id(),
            ),
            tenant_scope,
            actor_scope,
            owner_scope,
            agent_id: agent_id.to_string(),
            delegation_digest,
            authority_digest,
            attempt_nonce: context.attempt_nonce(),
            idempotency_key: context.idempotency_key().to_string(),
            admin,
            can_read,
            can_write,
            #[cfg(feature = "security")]
            identity_actor: crate::identity_view::identity_actor(context),
        })
    }

    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    pub fn actor_scope(&self) -> &str {
        &self.actor_scope
    }

    pub fn owner_scope(&self) -> &str {
        &self.owner_scope
    }

    /// `(verified tenant id, principal persistence id)` — the keys of the caller's
    /// decision log and of the records only it may see (EH-066).
    pub fn log_owner(&self) -> (&str, &str) {
        (&self.log_owner.0, &self.log_owner.1)
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// The verified effective actor for a request. This is deliberately kept
    /// separate from `actor_scope`, which is the durable principal catalog key.
    pub fn effective_actor_scope(&self) -> &str {
        &self.agent_id
    }

    pub fn delegation_digest(&self) -> Digest256 {
        self.delegation_digest
    }

    pub fn authority_digest(&self) -> Digest256 {
        self.authority_digest
    }

    /// The authenticated transport nonce for the request that minted this
    /// carrier. Auxiliary mutation stores use this exact value when they
    /// compile their kernel replay identity; it is never derived from a
    /// caller-controlled method field.
    pub fn attempt_nonce(&self) -> Option<Nonce> {
        self.attempt_nonce
    }

    /// Stable caller-supplied idempotency key from the verified envelope.
    /// Self-routed stores use it as the operation replay key rather than
    /// inventing a second key from request metadata.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Collision-proof tenant+actor namespace for a caller-controlled local name.
    pub fn namespace(&self, domain: &str, local: &str) -> String {
        crate::batch_compile::opaque_coordinator_key(domain, &self.owner_scope, local)
    }

    pub fn owns(&self, tenant_scope: &str, actor_scope: &str) -> bool {
        self.tenant_scope == tenant_scope && self.actor_scope == actor_scope
    }

    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// The verified identity actor, for the identity store's own check.
    #[cfg(feature = "security")]
    pub fn identity_actor(&self) -> &medulla_identity_types::identity::IdentityActor {
        &self.identity_actor
    }

    /// Coarse read capability (`kg:read` or admin). See the field's doc
    /// comment in [`Self::from_verified`] for why this is `true` for every
    /// pre-existing carrier-minting site and only meaningfully varies for the
    /// Iceberg-REST bearer path (NE-048).
    pub fn can_read(&self) -> bool {
        self.can_read
    }

    /// Coarse write capability (`kg:write` or admin). See [`Self::can_read`].
    pub fn can_write(&self) -> bool {
        self.can_write
    }

    pub fn require_admin(&self, domain: &str) -> Result<(), String> {
        if self.admin {
            Ok(())
        } else {
            medulla_kernel::metrics::access_denied();
            Err(format!(
                "ACCESS_DENIED: {domain} has no per-row ownership and requires kg:admin"
            ))
        }
    }
}

fn opaque_scope(value: &str, namespace: &str) -> bool {
    value
        .strip_prefix(namespace)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

/// Any carrier that has not produced a verified [`CarrierAuthority`] is denied
/// before touching engine state (A18). Each auxiliary surface must first attempt
/// to authenticate the caller through its OWN protocol-appropriate mechanism —
/// SigV4 for the S3-compatible REST surface, the `eg2.` envelope for SPARQL
/// mutations, the bearer/JWT guard for the KV-cache surface, ... — and mint a
/// `CarrierAuthority` only on success (mirroring how `pgwire`/`mysql-wire`/
/// `bolt-wire` bind a server-owned tenant+actor authority only after their own
/// cryptographic proof succeeds). This function's only job is to ask whether
/// that succeeded; it cannot itself authenticate anything, since a per-request
/// credential is never available at this layer.
pub fn unauthenticated_carrier_denied(carrier: Option<&CarrierAuthority>) -> bool {
    carrier.is_none()
}

/// The single row-level authority carried by every served graph read.
///
/// The actor is derived from [`VerifiedRequestContext`], never from the unsigned
/// display fields on the request envelope.  When row isolation is active, the
/// authority projects a detached graph containing only visible nodes, edges whose
/// two endpoints are visible, and embeddings belonging to visible nodes.  Passing
/// that projection to primitive/algorithm handlers closes existence, count,
/// batch, semantic, topology, and path side channels at the lowest shared read
/// boundary instead of relying on every response serializer to remember a filter.
#[derive(Clone)]
pub struct GraphReadAuthority {
    carrier: Option<CarrierAuthority>,
    #[cfg(feature = "security")]
    actor: String,
    #[cfg(feature = "security")]
    isolation: Arc<IsolationLayer>,
}

impl GraphReadAuthority {
    /// Build a read authority after request verification and graph ACL checks.
    /// The context is already verified before this constructor is reachable.
    pub fn from_verified(
        context: &VerifiedRequestContext,
        isolation: &IsolationLayer,
    ) -> Result<Self, String> {
        let carrier = Some(CarrierAuthority::from_verified(context)?);
        #[cfg(feature = "security")]
        {
            let actor = context.agent_id().trim().to_string();
            if actor.is_empty() {
                medulla_kernel::metrics::access_denied();
                return Err(
                    "ACCESS_DENIED: verified row-level graph read has no actor identity"
                        .to_string(),
                );
            }
            Ok(Self {
                carrier,
                actor,
                isolation: Arc::new(isolation.clone()),
            })
        }
        #[cfg(not(feature = "security"))]
        {
            let _ = (context, isolation);
            Ok(Self { carrier })
        }
    }

    /// Verified tenant+actor authority for non-graph legs fused into a graph read.
    pub fn carrier(&self) -> Option<&CarrierAuthority> {
        self.carrier.as_ref()
    }

    /// Effective actor for graph ACL checks on secondary/cross-graph reads.
    pub fn actor(&self) -> Option<&str> {
        #[cfg(feature = "security")]
        {
            (!self.actor.is_empty()).then_some(self.actor.as_str())
        }
        #[cfg(not(feature = "security"))]
        {
            None
        }
    }

    /// Return the non-empty actor bound to this verified read authority.
    /// Optional transport identity exists only before authorization; served
    /// handlers use this strict accessor before cache, RLS, admission, or state
    /// ownership logic.
    pub fn verified_actor(&self) -> Result<&str, String> {
        #[cfg(feature = "security")]
        let actor = self.actor.as_str();
        #[cfg(not(feature = "security"))]
        let actor = match self.carrier.as_ref() {
            Some(carrier) => carrier.agent_id(),
            None => {
                medulla_kernel::metrics::access_denied();
                return Err("ACCESS_DENIED: verified row-level actor is required".to_string());
            }
        };
        if actor.trim().is_empty() {
            medulla_kernel::metrics::access_denied();
            Err("ACCESS_DENIED: verified row-level actor is required".to_string())
        } else {
            Ok(actor)
        }
    }

    /// Whether per-row projection is required for this request.
    pub fn is_active(&self) -> bool {
        #[cfg(feature = "security")]
        {
            true
        }
        #[cfg(not(feature = "security"))]
        {
            false
        }
    }

    /// Apply the same lowest-level visibility predicate used by SQL/Cypher/RDF.
    pub fn filter_view(&self, view: &mut GraphView) {
        #[cfg(feature = "security")]
        self.isolation.filter_view(&self.actor, view);
        #[cfg(not(feature = "security"))]
        let _ = view;
    }

    /// perf/row-visibility-index (B-sweep) — the `FilteredViewCache`-aware
    /// equivalent of a caller's own `core.analysis_snapshot()` + `self.filter_view(&mut
    /// view)` pair, for a call site that wants a PLAIN (no write-set overlay)
    /// RLS-filtered read of the current graph. Mirrors `Method::CypherQuery`'s
    /// probe-then-build shape (`src/server/handlers/query.rs`) using THIS
    /// authority's own actor: a hit returns the SAME shared `Arc<GraphView>` a
    /// concurrent request for this actor at this version already built; a miss
    /// builds+filters exactly as before and publishes the result for the next
    /// caller. `version` is read BEFORE the snapshot is taken — a safe LOWER
    /// bound (an under-estimate only risks one extra rebuild, never a stale
    /// serve — see `medulla_server::handlers::query::rls_snapshot`'s doc for the
    /// full argument), since [`GraphReadAuthority`] does not hold the topology
    /// lock across both reads the way `GraphCore::analysis_snapshot_versioned`
    /// does.
    ///
    /// Callers that overlay buffered writes onto the view before use (a staged
    /// transaction's own read-your-own-writes path) MUST NOT use this — the
    /// cached entry is a pure function of (actor, version) alone and is shared
    /// across every caller at that version; mutating it would corrupt another
    /// caller's cached read. Build an owned snapshot via
    /// `core.analysis_snapshot()` + [`Self::filter_view`] instead for that case,
    /// exactly as today.
    #[cfg(feature = "security")]
    pub fn cached_filter_view(&self, core: &Arc<GraphCore>) -> Arc<GraphView> {
        let probe_version = core.version();
        if let Some(cached) = core.cached_filtered_view(&self.actor, probe_version) {
            return cached;
        }
        let generation = core.filtered_view_cache_generation();
        let mut fresh = core.analysis_snapshot();
        self.filter_view(&mut fresh);
        let fresh = Arc::new(fresh);
        core.put_cached_filtered_view(self.actor.clone(), probe_version, generation, fresh.clone());
        fresh
    }

    /// Answer ONE node-membership question without materializing a projection.
    ///
    /// `project_core` builds a whole filtered `GraphCore`
    /// (`O(V log V + E log E + V*d)`), which a point lookup does not need: a
    /// `HasNode` used to pay the same cost as a full graph dump, and on a live
    /// 56k-node graph that measured ~1.2s per call even WITH the projection cache
    /// in front of it (the cache is keyed on `GraphCore::version()`, so an ingest
    /// that interleaves reads and writes misses on essentially every read).
    ///
    /// Visibility is delegated to `IsolationLayer::can_see_node` — the SAME
    /// function `filter_view` applies to every node — so this fast path cannot
    /// drift from the bulk path it short-circuits. It is id-aware, which the
    /// blob-only `can_see_node` below deliberately is not: a node present in the
    /// topology with NO property blob must be judged `default_public` (what
    /// `filter_view` does), and `row_visibility(&[])` is not that. `analysis_snapshot()` is the
    /// cheap half of `build_projection` (it clones concurrent-map handles and the
    /// `schema_node_ids` reverse index, not every node's property blob); the
    /// expensive half is the sort + per-node clone into a second core, which is
    /// exactly what this skips.
    pub fn node_visible(&self, core: &GraphCore, node_id: &str) -> bool {
        let view = core.analysis_snapshot();
        if !view.node_map.contains_key(node_id) {
            return false;
        }
        #[cfg(feature = "security")]
        {
            self.isolation.can_see_node(&self.actor, &view, node_id)
        }
        #[cfg(not(feature = "security"))]
        {
            true
        }
    }

    /// Evaluate one CDC before/after property blob with the exact graph-row RLS
    /// predicate.  An absent image is not evidence of visibility; callers normally
    /// authorize an event when either its before or after image is visible.
    pub fn can_see_blob(&self, blob: &[u8]) -> bool {
        #[cfg(feature = "security")]
        {
            self.isolation
                .can_see_row(&self.actor, &medulla_graph_core::rls::row_visibility(blob))
        }
        #[cfg(not(feature = "security"))]
        {
            let _ = blob;
            true
        }
    }

    /// Return the original core when RLS is inactive, otherwise a detached,
    /// fully filtered core safe for arbitrary primitive/algorithm reads.
    ///
    /// D-OP-1 / D-OB-20: the filtered core is expensive to build (sort + clone every
    /// visible node/edge, copy every visible embedding — `O(V log V + E log E +
    /// V*d)`), so it is cached per (actor, `GraphCore::version()`) and reused across
    /// repeat calls between writes — see `crate::rls_projection_cache` for the full
    /// rationale. A cache hit returns byte-identical content to a fresh rebuild for
    /// the same (actor, version) pair (proven by
    /// `alice_and_bob_same_tenant_shared_graph_cannot_observe_each_others_rows` and
    /// `project_core_reflects_a_mutation_after_the_previous_projection_was_cached`,
    /// both below) — this amortizes the existing guarantee, it does not relax it.
    pub fn project_core(&self, core: &Arc<GraphCore>) -> Arc<GraphCore> {
        if !self.is_active() {
            return core.clone();
        }
        // `is_active()` is only ever `true` when the `security` feature is compiled
        // in (see its own doc), so the actor field + cache accessors this delegates
        // to — both `security`-gated — are always present past this point. Split
        // into its own `#[cfg]`-gated method rather than inlining `self.actor` here
        // directly: this function itself is NOT `#[cfg]`-gated (it must exist and
        // type-check in a `not(security)` build too, where it is simply dead code
        // behind the early return above).
        self.project_core_active(core)
    }

    /// The cache-checking half of [`Self::project_core`], split out because it
    /// touches the `security`-only `actor` field and cache accessors — see that
    /// method's doc. Only ever called when `is_active()` is `true`.
    #[cfg(feature = "security")]
    fn project_core_active(&self, core: &Arc<GraphCore>) -> Arc<GraphCore> {
        let actor = self.actor.clone();
        let current_version = core.version();
        if let Some(cached) = core.cached_projection(&actor, current_version) {
            medulla_kernel::metrics::projection_cache_hit();
            return cached;
        }

        // U-142/U-143/U-145 (BUG-130): capture the whole-image generation BEFORE the
        // (potentially slow, unlocked) rebuild starts, not after. A whole-image
        // replacement/clear (`GraphCore::replace_snapshot`/`clear`/`hibernate`) that
        // lands while this build is in flight bumps the generation; `put_cached_projection`
        // then refuses to publish this now-stale result. See
        // `GraphCore::projection_cache_generation`'s doc.
        let generation = core.projection_cache_generation();
        // D-EGP-1 (t1-grounding-0802): time the rebuild a miss triggers — see
        // `medulla_kernel::metrics::projection_cache_miss` for why this is the decisive
        // measurement for the RLS-cache-thrash hypothesis.
        let build_started = std::time::Instant::now();
        let projected = self.build_projection(core);
        medulla_kernel::metrics::projection_cache_miss(build_started.elapsed().as_secs_f64());
        core.put_cached_projection(actor, current_version, generation, projected.clone());
        projected
    }

    /// Unreachable in practice (`is_active()` is always `false` without `security`,
    /// so `project_core` already returned above) — exists only so `project_core`
    /// type-checks in a `not(security)` build.
    #[cfg(not(feature = "security"))]
    fn project_core_active(&self, core: &Arc<GraphCore>) -> Arc<GraphCore> {
        core.clone()
    }

    /// The single-row sibling of [`Self::filter_view`] (which `build_projection`
    /// uses in bulk): may this authority's actor see `properties_msgpack`?
    ///
    /// `build_projection`'s RAM-only `analysis_snapshot()` can never discover a
    /// node `EvictLRU` fully evicted from the live topology (its id is gone from
    /// `topo.node_map`, not merely its properties — see
    /// `GraphCore::evict_resident_nodes`), so a point read that falls back to the
    /// RAW core's `read_through` for such a node (`Method::GetNodeProperties`)
    /// must re-derive the SAME visibility decision `filter_view` would have made,
    /// on exactly that one row, rather than serving it unfiltered.
    ///
    /// `is_schema` (BUG A3, 2026-08-12) is the caller-supplied, DERIVED
    /// TBox/ABox answer for this exact node id (`GraphCore::is_schema_node`) —
    /// this function has no id, only a blob, so it cannot look it up itself;
    /// `row_visibility`'s own `.schema` is always `false` (it no longer
    /// decodes a `_schema` property, which nothing clears on axiom deletion).
    /// See `IsolationLayer::filter_view` for the bulk-path sibling of this
    /// same derivation.
    #[cfg(feature = "security")]
    pub fn can_see_node(&self, properties_msgpack: &[u8], is_schema: bool) -> bool {
        let mut vis = row_visibility(properties_msgpack);
        vis.schema = is_schema;
        self.isolation.can_see_row(&self.actor, &vis)
    }

    /// Unreachable in practice (`is_active()` is always `false` without
    /// `security`, so the projection is never bypassed) — exists only so callers
    /// type-check in a `not(security)` build, mirroring `project_core_active`.
    #[cfg(not(feature = "security"))]
    pub fn can_see_node(&self, _properties_msgpack: &[u8], _is_schema: bool) -> bool {
        true
    }

    /// The actual (expensive) materialization [`Self::project_core_active`] caches. Kept as
    /// its own method so the cache lookup/store bracketing it stays visually obvious
    /// at the call site and so a cache bypass (e.g. a future forced-fresh caller) has
    /// a name to call directly.
    ///
    /// D-EGP-1 (t1-grounding-0802): uses `add_node_no_ledger`/`add_edge_no_ledger`
    /// rather than the ordinary `add_node`/`add_edge`. Construction used to go
    /// through the ordinary mutation helpers, which append synthetic `ADD_*`
    /// records that this method then threw away wholesale (`ledger.lock().clear()`)
    /// because the original ledger cannot be safely row-filtered from its
    /// unstructured string representation, so active RLS serves no ledger rather
    /// than a fabricated or cross-row history. Every one of those records was a
    /// `HexLedger`-formatted (per-byte `write!`) copy of the node/edge's full
    /// property blob — on a live pod this measured as the DOMINANT cost of a cache
    /// miss (`medulla_projection_cache_miss_build_seconds`: 2.4-15s per
    /// miss on a 27,969-node graph). The `_no_ledger` variants skip the format +
    /// hex-encode + ledger-mutex-push entirely; there is no longer a ledger to
    /// clear, and the resulting `GraphCore` is byte-for-byte identical in every
    /// OTHER respect (node_map/graph/node_properties/edge_properties/bloom).
    fn build_projection(&self, core: &Arc<GraphCore>) -> Arc<GraphCore> {
        let mut view = core.analysis_snapshot();
        self.filter_view(&mut view);
        let projected = GraphCore::new();

        // Stable ordering makes serialized projections and adversarial tests
        // deterministic even though the source maps are concurrent hash maps.
        let mut node_ids: Vec<String> = view.node_map.keys().cloned().collect();
        node_ids.sort();
        for node_id in &node_ids {
            let properties = view
                .node_properties
                .get(node_id)
                .map(|value| value.as_ref().clone())
                .unwrap_or_default();
            projected.add_node_no_ledger(node_id.clone(), properties);
            // BUG A3 follow-up (2026-08-12): a fresh `GraphCore::new()` starts
            // with an EMPTY `schema_refs`, so without this, a schema node that
            // legitimately survived `filter_view` above (via the SOURCE
            // core's real TBox membership) would fail RLS again on the very
            // next `analysis_snapshot()` taken of THIS projected core — every
            // read surface that calls `rls.filter_view` a second time against
            // an already-projected core (SQL/UQL, `handlers/query.rs`) would
            // silently re-hide it, exactly reproducing BUG A3's symptom one
            // layer downstream of the fix in `medulla_isolation::isolation`. Mirror the
            // SOURCE core's TBox membership onto the projection for every node
            // that survived filtering; a single ref is enough (the projection
            // is a throwaway, read-only snapshot — never mutated further, so
            // exact refcount fidelity does not matter here).
            if core.is_schema_node(node_id) {
                projected.mark_schema_ref(node_id);
            }
        }

        let mut edge_keys: Vec<(String, String)> = view.edge_properties.keys().cloned().collect();
        edge_keys.sort();
        for (source, target) in edge_keys {
            if let Some(properties) = view.edge_properties.get(&(source.clone(), target.clone())) {
                for property in properties {
                    // Both endpoints survived `filter_view`; failure would indicate
                    // an internally inconsistent snapshot, so simply omit that edge
                    // rather than reintroducing a topology side channel.
                    let _ = projected.add_edge_no_ledger(
                        source.clone(),
                        target.clone(),
                        property.as_ref().clone(),
                    );
                }
            }
        }

        // Semantic search is a row read too. Rebuild only the visible portion so
        // ANN candidates cannot reveal hidden ids or alter result cardinality via a
        // post-hoc serializer filter.
        {
            let source = core.semantic_store.read();
            let mut target = projected.semantic_store.write();
            for node_id in &node_ids {
                if let Some(embedding) = source.get_embedding(node_id) {
                    // `target` starts empty and every embedding copied here comes
                    // from the SAME already-valid, uniform-dimension `source`, so
                    // this cannot legitimately mismatch (CONCEPT:MEDULLA-KG.compute.rank-dim-mismatch-guard,
                    // BUG-007). `build_projection` returns `Arc<GraphCore>` (not a
                    // Result), so a surprise here degrades gracefully — the node is
                    // logged and simply absent from this read's kNN candidates —
                    // rather than panicking a read path over a should-never-happen.
                    if let Err(error) = target.add_embedding(node_id.clone(), embedding) {
                        tracing::warn!(
                            node_id = node_id.as_str(),
                            %error,
                            "RLS projection dropped a node's embedding — source store had an unexpected dimension"
                        );
                    }
                }
            }
        }

        Arc::new(projected)
    }
}
