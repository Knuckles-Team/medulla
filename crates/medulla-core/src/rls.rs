//! Row-level security vocabulary (CONCEPT:MEDULLA-KG.sharding.row-level-security): the
//! reserved RLS property keys, a row's decoded [`RowVisibility`] and the
//! [`row_visibility`] parser. The graph core's row-visibility index and the isolation
//! layer's row filter both read these, so they sit below both.

/// Per-row owner/visibility derived from a node's property blob (CONCEPT:MEDULLA-KG.sharding.row-level-security).
/// The reserved property keys (`_owner` / `_visibility` / `_grants`) form this
/// crate's own native RLS convention, enforced by [`IsolationLayer::filter_view`].
///
/// **BUG-052 / GOC-61 read-both compatibility.** `agent-utilities`' independent
/// governed-write chokepoint (`knowledge_graph/core/tenant_sharing.py`) stamps a
/// DIFFERENTLY-NAMED but semantically equivalent pair — `_owner_id` (owner) and
/// `_shared_scope` (`private`/`org`/`commons`, superset of this crate's binary
/// `_visibility`) — and is, in practice, the authority that decides ownership for
/// virtually all traffic that reaches the engine through `agent-utilities` (that
/// caller's connection is provisioned as `AgentRole::System`, which bypasses
/// `can_see_row` before a single row is inspected — see
/// `plans/graph-os-completion-program/decisions/GOC-61-ownership-property-convention.md`
/// for the full analysis of which layer actually decides a live access). Any
/// caller that reaches THIS engine directly — Bolt/pgwire/mysql-wire/GraphQL/
/// SPARQL/AMQP/MQTT/STOMP, none of which route through `tenant_sharing.py` — sees
/// ONLY this crate's own `row_visibility` decision, so a node `agent-utilities`
/// stamped `_owner_id`/`_shared_scope='private'` was, before this change,
/// invisible to `can_see_row` as an *owner* row and fell through to the
/// untagged/tagged-public branches instead — a real, not cosmetic, visibility gap
/// for any direct caller. `row_visibility` below now falls back to `_owner_id`/
/// `_shared_scope` whenever `_owner`/`_visibility` are absent, so a node tagged by
/// EITHER convention resolves to the same [`RowVisibility`] regardless of which
/// wire path reads it. This is READ-ONLY compatibility: nothing in this crate
/// writes `_owner`/`_visibility`/`_grants` in production (grep-confirmed — every
/// production write path that could stamp RLS properties is `agent-utilities`',
/// which already writes only `_owner_id`/`_shared_scope`), so there is no
/// persisted-format migration here, and the two decision records intentionally
/// name `_owner_id`/`_shared_scope` the canonical convention going forward — this
/// crate's own `_owner`/`_visibility`/`_grants` constants stay as a read-side
/// fallback for any pre-existing/direct-write data, not the preferred write shape.
#[cfg(feature = "security")]
#[derive(Debug, Clone, Default)]
pub struct RowVisibility {
    /// Owning agent_id (`_owner`, falling back to `_owner_id`); `None` requires
    /// both [`Self::tagged`] and [`Self::public`] under the default-deny posture.
    pub owner: Option<String>,
    /// `true` when visible beyond the owner alone. Derived from `_visibility`
    /// (absent-or-`"public"` ⇒ true, `"private"` ⇒ false) when present; else from
    /// `_shared_scope` (`"org"`/`"commons"` ⇒ true, `"private"` ⇒ false) when
    /// present; else `true` (the pre-existing bare-absent default).
    ///
    /// **BUG-064 trust boundary.** A `_visibility='public'` reading is NOT
    /// taken at face value when it cannot legitimately have been written: a
    /// row also carrying `_owner_id`/`_shared_scope` defers to that
    /// convention's own value instead (only it has a known production
    /// writer), and a fully unowned row with no `_grants` treats a bare
    /// `_visibility` tag alone as uncorroborated. See [`row_visibility`]'s
    /// doc comment for the full incident this closes.
    pub public: bool,
    /// Agent_ids explicitly granted read (`_grants`, comma-separated). No
    /// `agent-utilities`-side equivalent exists yet (GOC-61 designs a distinct
    /// `grant_id`-keyed model, not a per-node CSV list), so there is nothing to
    /// fall back to here.
    pub grants: Vec<String>,
    /// Whether this row carried ANY explicit RLS metadata at all, under EITHER
    /// convention — i.e. the decoded property blob contained at least one of
    /// `_owner` / `_visibility` / `_grants` / `_owner_id` / `_shared_scope`.
    /// `false` for an undecodable blob OR a blob that decoded fine but declares
    /// none of the five keys. Untagged rows fail the default-deny decision.
    pub tagged: bool,
    /// A18 TBox/ABox RLS distinction: `true` when the row's property blob
    /// carries [`RLS_SCHEMA_KEY`] — an OWL/RDFS axiom/class/property-definition
    /// node stamped by `medulla_rdf`'s lowering, structurally, never a name
    /// convention. [`IsolationLayer::can_see_row`] treats a schema row as
    /// visible regardless of `owner`/`public`/`tagged`/`grants` — see
    /// [`RLS_SCHEMA_KEY`]'s doc comment for the full ABox/TBox reasoning and
    /// why graph-level ACL (unaffected by this field) remains the real gate.
    /// An ABox row (`schema: false`, the default) is completely unaffected:
    /// default-deny applies exactly as it did before this field existed.
    pub schema: bool,
}

/// Reserved RLS property keys (this crate's own native convention).
#[cfg(feature = "security")]
pub const RLS_OWNER_KEY: &str = "_owner";

#[cfg(feature = "security")]
pub const RLS_VISIBILITY_KEY: &str = "_visibility";

#[cfg(feature = "security")]
pub const RLS_GRANTS_KEY: &str = "_grants";

/// `agent-utilities`' `tenant_sharing.py` convention (BUG-052 / GOC-61
/// read-both compatibility — see [`RowVisibility`]'s doc comment). Named
/// canonical going forward by
/// `decisions/GOC-61-ownership-property-convention.md`; read here as a
/// fallback, never written by this crate.
#[cfg(feature = "security")]
pub const RLS_OWNER_ID_KEY: &str = "_owner_id";

#[cfg(feature = "security")]
pub const RLS_SHARED_SCOPE_KEY: &str = "_shared_scope";

/// Reserved node-property key marking a row as ontology SCHEMA (TBox) rather
/// than instance data (ABox) — CONCEPT:MEDULLA-KG.sharding.row-level-security, A18 TBox/ABox RLS
/// distinction.
///
/// Row-level default-deny ([`IsolationLayer::can_see_row`]) exists to protect
/// ABox rows — a row ABOUT someone/something whose owner should control its
/// visibility. An OWL axiom / class / property-definition node is SCHEMA, not
/// a row about anyone; applying row-level default-deny to it is a category
/// error, and doing so made an entire graph's schema invisible to every
/// non-`System` actor (see `tests/pgwire_roundtrip.rs::wire_reason_iri_bridges_string_typed_node`).
/// Schema is already protected at the correct granularity by GRAPH-level ACL
/// (`server::access::check_graph_access`, enforced upstream of every read
/// that reaches row filtering, UNCHANGED by this key's existence): if a
/// caller may read the graph at all, it may see the graph's schema; if it may
/// not, [`IsolationLayer::filter_view`] is never reached for that caller in
/// the first place.
///
/// This key is stamped ONLY by the RDF/OWL lowering layer (`medulla_rdf::mapping`,
/// `medulla_rdf::update`) on a node that is STRUCTURALLY a class/property/axiom
/// reference — the subject or object of a recognized RDFS/OWL schema
/// predicate (`rdfs:subClassOf`, `owl:equivalentClass`, `owl:onProperty`, …),
/// or the subject of an explicit `rdf:type owl:Class`/`rdfs:Class`/
/// `owl:ObjectProperty`/… declaration — NEVER a name convention on the node
/// id. `medulla-core` deliberately does not know RDF/OWL vocabulary (`medulla-rdf`
/// depends on `medulla-core`, not the reverse); this crate only trusts this ONE
/// narrow, explicit marker set by the layer that DOES know the vocabulary,
/// and does nothing to widen visibility for any row that lacks it — an
/// untagged ABox row is denied exactly as it was before this key existed.
///
/// Deliberately NOT gated on feature `security` (unlike the other `RLS_*_KEY`
/// constants below) — `medulla-rdf`'s lowering (feature `rdf`, which does not
/// itself depend on `medulla-core/security`) must be able to name this key
/// whenever RDF/OWL lowering compiles at all, independent of whether THIS
/// build additionally happens to enable row-level enforcement. Only the
/// constant is unconditional; every place that INTERPRETS it
/// ([`RowVisibility`], [`row_visibility`], [`IsolationLayer::can_see_row`])
/// stays exactly as gated as it always was.
pub const RLS_SCHEMA_KEY: &str = "_schema";

/// Parse a node's msgpack property blob into its [`RowVisibility`].
///
/// A blob that cannot be decoded as a string-keyed map, or that declares none of
/// the RLS keys (under EITHER naming convention — see [`RowVisibility`]), yields
/// `tagged = false` and is denied. Explicit ownership, visibility, and grants
/// remain the only row authorization inputs.
#[cfg(feature = "security")]
pub fn row_visibility(blob: &[u8]) -> RowVisibility {
    use serde_json::Value;
    let map: std::collections::BTreeMap<String, Value> =
        match medulla_types::msgpack::decode_bounded(
            blob,
            medulla_types::msgpack::MsgpackLimits::new(
                medulla_types::msgpack::MAX_PROPERTY_BYTES,
                medulla_types::msgpack::MAX_PROPERTY_ITEMS,
                medulla_types::msgpack::DEFAULT_MAX_DEPTH,
            ),
        ) {
            Ok(m) => m,
            Err(_) => return RowVisibility::default_public(),
        };
    let owner = map
        .get(RLS_OWNER_KEY)
        .or_else(|| map.get(RLS_OWNER_ID_KEY))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    // BUG-064: whether an au (`tenant_sharing.py`) convention key is present
    // on this row at all — `_owner_id` or `_shared_scope`. Used below to
    // decide how much to trust a native `_visibility` key on the SAME row.
    let au_tagged = map.contains_key(RLS_OWNER_ID_KEY) || map.contains_key(RLS_SHARED_SCOPE_KEY);
    let shared_scope_public = map
        .get(RLS_SHARED_SCOPE_KEY)
        .and_then(|v| v.as_str())
        .map(|v| v.eq_ignore_ascii_case("org") || v.eq_ignore_ascii_case("commons"));
    let public = match map.get(RLS_VISIBILITY_KEY).and_then(|v| v.as_str()) {
        Some(v) => {
            let native_public = !v.eq_ignore_ascii_case("private");
            // BUG-064 (see BUG-LEDGER.md#BUG-064): the native `_visibility`
            // key has ZERO known legitimate production writers in either
            // repo (grep-confirmed across all history) — its only
            // real-world occurrence to date was an out-of-band admin RPC
            // (`engine_lifecycle_batch_update`) that blanket-executed
            // `MATCH (n) WHERE n._visibility IS NULL SET n._visibility =
            // 'public'` on 2026-08-07, stamping 45,478 nodes irrespective of
            // their real ownership/scope. Because this branch used to be
            // consulted BEFORE `_shared_scope` unconditionally, that one
            // write silently converted every reachable owned row —
            // including ones genuinely `_shared_scope='private'` — into
            // `public` for any non-System caller, and made every untagged
            // unowned row look "explicitly public". A `_visibility='public'`
            // read here is therefore trusted only when there is a
            // legitimate story for how it could have been written:
            // * a row also tagged by the au convention (`_owner_id` /
            //   `_shared_scope`) is decided by THAT convention's own
            //   `_shared_scope` instead — the one write chokepoint proven to
            //   write these keys in production (`tenant_sharing.stamp_ownership`).
            //   A `_visibility` key can only have arrived out-of-band on
            //   such a row.
            // * a row with NO owner at all and no `_grants` has no
            //   tenant/ACL context establishing WHO decided it should be
            //   public, so a bare tag alone is not corroboration — see the
            //   BUG-064 disposition: "a bare `_visibility='public'` with no
            //   real owner/tenant must not by itself grant a non-System
            //   caller anything".
            // A genuinely NATIVE `_owner`-tagged row (no au tagging) keeps
            // trusting `_visibility` as before: the incident's writer never
            // sets `_owner`, so that combination cannot be its output, and
            // this is the only pre-existing "owner marks own row public"
            // path this crate itself ever exercises.
            // `_visibility='private'` is always honored outright — fewer
            // false grants is the safe direction and needs no override.
            //
            // BUG-193 follow-up (found during review, before landing —
            // `bug193_stamped_visibility_interaction` pins it): the
            // `unwrap_or` default below only fires when `_shared_scope` is
            // ABSENT (`shared_scope_public == None`) despite `au_tagged`
            // being true via `_owner_id` alone. Before BUG-193, that
            // combination could not occur in production: the only known
            // `_owner_id` writer (`tenant_sharing.stamp_ownership`) ALWAYS
            // sets `_shared_scope` in the same write (§7 of
            // `decisions/GOC-61-ownership-property-convention.md` measures
            // this as a live invariant — 24,414/24,414). `unwrap_or(false)`
            // was therefore a defensive default for a combination that
            // "shouldn't happen" for that population, not a load-bearing
            // decision. BUG-193's own write-side stamp
            // (`stamp_owner_id_if_absent`) breaks that invariant on
            // purpose: it stamps `_owner_id` from the caller's identity but
            // deliberately never invents a `_shared_scope` (visibility
            // breadth is caller-supplied content, not an identity fact the
            // stamp has authority to invent — see that fn's doc). So a
            // native caller who explicitly writes `_visibility: "public"`
            // with no ownership key of their own now ALSO ends up
            // `au_tagged` (via the stamp) with `_shared_scope` still
            // absent — and `unwrap_or(false)` would silently downgrade
            // their explicit "public" declaration to visible-to-owner-only
            // for every other caller, a real regression this exact edit
            // closes. `unwrap_or(native_public)` instead: when there is no
            // au `_shared_scope` to contradict it, trust the row's own
            // explicit native `_visibility` value — exactly the same
            // "trust the caller's stated visibility absent a corroborating,
            // more-authoritative signal" posture the `None` arm below
            // already takes for a row with no `_visibility` key at all.
            // Does NOT reopen BUG-064: every BUG-064 incident row has
            // `_owner_id IS NULL` (confirmed live, `decisions/
            // GOC-61-ownership-property-convention.md` §7 and
            // `BUG-LEDGER.md#BUG-064`), so `au_tagged` is `false` for that
            // population — it never reaches this branch at all and keeps
            // hitting the `owner.is_none()` branch below unchanged
            // (`unowned_bare_visibility_tag_alone_is_no_longer_trusted`
            // pins that). And whenever `_shared_scope` IS present (the
            // ordinary au-mediated-write shape), `shared_scope_public` is
            // `Some(..)` and this default is never consulted — the
            // `au_shared_scope_private_still_overrides_a_conflicting_
            // native_public_tag` test pins that a real, explicit
            // `_shared_scope: private` still wins over a conflicting native
            // tag, unaffected by this change.
            if native_public && au_tagged {
                shared_scope_public.unwrap_or(native_public)
            } else if native_public && owner.is_none() && !map.contains_key(RLS_GRANTS_KEY) {
                false
            } else {
                native_public
            }
        }
        // `tenant_sharing.SCOPE_ORG`/`SCOPE_COMMONS` — visible beyond the
        // owner (tenant/graph-level isolation already happened upstream of
        // this row, at graph selection, so "org" here correctly maps to
        // "public within this graph" exactly as `visibility_predicate`
        // treats it). `SCOPE_PRIVATE` ⇒ false. Absent ⇒ the pre-existing
        // bare-absent default (true).
        None => shared_scope_public.unwrap_or(true),
    };
    let grants = map
        .get(RLS_GRANTS_KEY)
        .and_then(|v| v.as_str())
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let tagged = map.contains_key(RLS_OWNER_KEY)
        || map.contains_key(RLS_VISIBILITY_KEY)
        || map.contains_key(RLS_GRANTS_KEY)
        || map.contains_key(RLS_OWNER_ID_KEY)
        || map.contains_key(RLS_SHARED_SCOPE_KEY);
    // A18: schema (TBox) is a SEPARATE dimension from the owner/visibility/
    // grants tagging above — an axiom node is neither owned nor "tagged
    // public" in the ABox sense, it is simply not a row RLS should be
    // deciding at all. See `RLS_SCHEMA_KEY` for the full reasoning.
    //
    // BUG A3 (2026-08-12): `schema` is UNCONDITIONALLY `false` here, never
    // decoded from the blob. It used to read a `_schema: true` property this
    // crate stamped once (`medulla_rdf`'s lowering) and never cleared on axiom
    // deletion, so an IRI once used as a schema term stayed schema-visible
    // forever, even after being repurposed as an ordinary ABox individual.
    // Schema-ness is now DERIVED from `GraphCore::schema_refs` (a live
    // reverse index: `iri -> count of live schema-defining triples`),
    // consulted by node id, not decoded from this function's blob-only
    // input. A caller with a live snapshot must set `.schema` on the
    // returned value itself — see `IsolationLayer::filter_view` (the bulk
    // row-filtering path, via `GraphView::schema_node_ids`) and
    // `GraphReadAuthority::can_see_node` (the single-row fallback, via
    // `GraphCore::is_schema_node`) for the two call sites that do.
    let schema = false;
    RowVisibility {
        owner,
        public,
        grants,
        tagged,
        schema,
    }
}

#[cfg(feature = "security")]
impl RowVisibility {
    /// `pub(crate)` (not private) so `crate::graph`'s row-visibility-index
    /// maintenance (`GraphCore::visibility_index_set`, perf/row-visibility-index)
    /// can produce the IDENTICAL fallback value `can_see_node` used to compute
    /// inline for a node with no property blob at all, before that index existed.
    pub fn default_public() -> Self {
        RowVisibility {
            owner: None,
            public: true,
            grants: Vec::new(),
            tagged: false,
            schema: false,
        }
    }
}
