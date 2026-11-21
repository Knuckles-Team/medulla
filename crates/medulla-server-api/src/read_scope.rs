//! Read scope of a served read: the visible knowledge units of the verified
//! caller (MEDULLA-IDENTITY-R023.3/.4) and the tenant predicate (MEDULLA-IDENTITY-R610.3/.4).
//!
//! Both are derived from the verified request carrier, never from request
//! fields or query text. The view is narrowed once, at the read chokepoint, so
//! every row, count and aggregate of the answer is computed over what the caller
//! may see.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_graph_core::graph::GraphView;
use medulla_identity_types::identity::{
    visible_units, KnowledgeUnit, ShareScope, UnitOutOfScope, UnitPrincipal, MAX_AUDITED_UNIT_IDS,
};
use medulla_identity_types::tenancy::{
    scope_predicate, tenant_graph, TenancyError, TenantId, TenantParent, TenantTree,
};
use serde_json::{Map, Value};

use crate::carrier::GraphReadAuthority;

/// The node key that holds the markings a promoted classification claim stamped
/// (MEDULLA-TYPED-PACKS-R652.3). The one definition; the claim handler writes it.
pub const MARKINGS_KEY: &str = "_markings";
/// The property a redacted node keeps in place of its stored properties.
pub const REDACTED_KEY: &str = "_redacted";
/// The scope prefix that grants one marking to a caller: a scope `marking:<name>`
/// grants the marking `<name>`. A wildcard scope grants none, so a marking is
/// always an explicit grant.
pub const MARKING_SCOPE_PREFIX: &str = "marking:";

/// The label of a principal-owned knowledge unit node.
pub const KNOWLEDGE_UNIT_LABEL: &str = "KnowledgeUnit";
/// The label of a stored tenant parent link node (keys `tenant`, `parent`, `shared`).
pub const TENANT_PARENT_LABEL: &str = "TenantParent";
/// The node key that places a node in a tenant's graph.
pub const TENANT_KEY: &str = "_tenant";
/// The node key naming the tenant of a unit's owner.
pub const OWNER_TENANT_KEY: &str = "owner_tenant";

/// The verified reader of one served read.
#[derive(Debug, Clone)]
pub struct ReadCaller {
    /// The reader as the unit visibility rules see it.
    pub principal: UnitPrincipal,
    /// The verified tenant, when it is a valid tenant id.
    pub tenant: Option<TenantId>,
    /// The markings the caller holds: one per `marking:<name>` scope of the verified
    /// carrier.
    pub held_markings: BTreeSet<String>,
}

impl ReadCaller {
    /// The caller of a read authority. The placement of a unit is the graph it is
    /// stored in, so the reader's placement is the graph the read targets.
    pub fn from_authority(authority: Option<&GraphReadAuthority>, graph_name: &str) -> Self {
        let carrier = authority.and_then(GraphReadAuthority::carrier);
        let tenant_raw = carrier
            .map(|c| c.log_owner().0.to_owned())
            .unwrap_or_default();
        // MEDULLA-IDENTITY-R023.3: the unit principal is the verified request
        // principal, never the agent carrying the request.
        let id = authority
            .map(GraphReadAuthority::verified_principal)
            .unwrap_or_default()
            .to_owned();
        Self {
            held_markings: carrier
                .map(|c| {
                    c.scopes()
                        .iter()
                        .filter_map(|scope| scope.strip_prefix(MARKING_SCOPE_PREFIX))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            tenant: TenantId::new(&tenant_raw).ok(),
            principal: UnitPrincipal {
                id,
                placement: graph_name.to_owned(),
                tenant: tenant_raw,
            },
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

fn is_label(map: &Map<String, Value>, label: &str) -> bool {
    ["node_type", "type"]
        .iter()
        .any(|key| text(map, key) == Some(label))
}

/// A `KnowledgeUnit` node and its owner as the visibility rules see them.
pub fn node_unit(id: &str, map: &Map<String, Value>) -> Option<(KnowledgeUnit, UnitPrincipal)> {
    if !is_label(map, KNOWLEDGE_UNIT_LABEL) {
        return None;
    }
    let owner = text(map, "owner")?.to_owned();
    let share_scope: ShareScope = serde_json::from_value(map.get("share_scope")?.clone()).ok()?;
    let access_labels: BTreeSet<String> = map
        .get("access_labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let unit = KnowledgeUnit {
        id: id.to_owned(),
        owner: owner.clone(),
        version: map.get("version").and_then(Value::as_u64).unwrap_or(1),
        supersedes: text(map, "supersedes").map(str::to_owned),
        access_labels,
        share_scope,
    };
    let owner_principal = UnitPrincipal {
        id: owner,
        // Units live in one graph, so owner and reader share the placement.
        placement: String::new(),
        tenant: text(map, OWNER_TENANT_KEY).unwrap_or_default().to_owned(),
    };
    Some((unit, owner_principal))
}

/// Decoded node objects of `view` whose blob contains `needle`.
fn nodes_with<'a>(
    view: &'a GraphView,
    needle: &'a [u8],
) -> impl Iterator<Item = (&'a String, Map<String, Value>)> + 'a {
    view.node_properties
        .iter()
        .filter(move |(_, blob)| contains(blob, needle))
        .filter_map(|(id, _)| view.node_row_object(id).map(|map| (id, map)))
}

/// What a unit scan found: ids the caller may see and ids it may not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct UnitScan {
    /// Visible unit ids, ordered by id.
    pub visible: Vec<String>,
    /// Unit ids the caller may not see (foreign private, superseded, other tenant).
    pub hidden: BTreeSet<String>,
}

/// Split the `KnowledgeUnit` nodes of `view` into visible and hidden for `caller`.
pub fn scan_units(view: &GraphView, caller: &ReadCaller) -> UnitScan {
    let mut units = Vec::new();
    // Keyed by unit id: an owner is the pair (tenant, principal), never the principal id alone.
    let mut owners: BTreeMap<String, UnitPrincipal> = BTreeMap::new();
    for (id, map) in nodes_with(view, KNOWLEDGE_UNIT_LABEL.as_bytes()) {
        if let Some((unit, owner)) = node_unit(id, &map) {
            let mut owner = owner;
            owner.placement.clone_from(&caller.principal.placement);
            owners.insert(unit.id.clone(), owner);
            units.push(unit);
        }
    }
    let visible: Vec<String> = visible_units(&caller.principal, &owners, &units)
        .into_iter()
        .map(|u| u.id.clone())
        .collect();
    let seen: BTreeSet<&str> = visible.iter().map(String::as_str).collect();
    let hidden = units
        .iter()
        .map(|u| u.id.clone())
        .filter(|id| !seen.contains(id.as_str()))
        .collect();
    UnitScan { visible, hidden }
}

impl ReadCaller {
    /// Whether the caller holds every marking in `markings`.
    pub fn holds_all(&self, markings: &[String]) -> bool {
        markings.iter().all(|m| self.held_markings.contains(m))
    }
}

/// The node ids whose stamped `_markings` the caller does not hold
/// (MEDULLA-TYPED-PACKS-R652.6). A marked node the caller lacks clearance for is
/// redacted, not refused: it stays in the view as an id with no properties.
fn redacted_nodes(view: &GraphView, caller: &ReadCaller) -> Vec<String> {
    nodes_with(view, MARKINGS_KEY.as_bytes())
        .filter_map(|(id, map)| {
            let markings: Vec<String> = map
                .get(MARKINGS_KEY)?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            (!markings.is_empty() && !caller.holds_all(&markings)).then(|| id.clone())
        })
        .collect()
}

/// The typed principal metadata labels (MEDULLA-IDENTITY-R023.8).
const METADATA_LABELS: [&str; 3] = ["Preference", "ClientMetadata", "InteractionNetwork"];

/// Metadata node ids the caller may not read: a record is its owner's, except a
/// `Preference` shared at `tenant` or `placement` scope, which reaches readers in
/// the owner's tenant (every reader of the graph for `placement`).
fn hidden_metadata(view: &GraphView, caller: &ReadCaller) -> BTreeSet<String> {
    // Metadata rows hold the owner's hashed identity and its tenant scope
    // (MEDULLA-IDENTITY-R023.10), so the reader is compared in that domain.
    let reader =
        crate::mutation_batch::identity_metadata::principal_persistence_id(&caller.principal.id);
    let reader_tenant = crate::carrier::tenant_scope_of(&caller.principal.tenant);
    let mut hidden = BTreeSet::new();
    for label in METADATA_LABELS {
        for (id, map) in nodes_with(view, label.as_bytes()) {
            if !is_label(&map, label) {
                continue;
            }
            let owner = text(&map, "owner").unwrap_or_default();
            let owner_tenant = text(&map, OWNER_TENANT_KEY).unwrap_or_default();
            // Ownership is the pair (tenant, principal): the same principal id in
            // another tenant is a different owner.
            if !caller.principal.id.is_empty() && owner == reader && owner_tenant == reader_tenant {
                continue;
            }
            let shared = label == "Preference"
                && match text(&map, "share") {
                    Some("placement") => true,
                    Some("tenant") => {
                        !caller.principal.tenant.is_empty() && owner_tenant == reader_tenant
                    }
                    _ => false,
                };
            if !shared {
                hidden.insert(id.clone());
            }
        }
    }
    hidden
}

/// The node variables a plan binds: each row source, in plan order.
#[cfg(feature = "query")]
pub fn bound_variables(plan: &medulla_query_types::wire::Plan) -> Vec<String> {
    use medulla_query_types::wire::Op;
    plan.ops
        .iter()
        .filter_map(|op| match op {
            Op::Scan { label } if !label.is_empty() => Some(label.clone()),
            Op::Scan { .. } | Op::ScanAll {} => Some("n".to_owned()),
            Op::Filter { .. }
            | Op::Traverse { .. }
            | Op::Expand { .. }
            | Op::Propagate { .. }
            | Op::Rank { .. }
            | Op::RankEmbed { .. }
            | Op::RankNodeDistance { .. }
            | Op::RankMentions {}
            | Op::RankMmr { .. }
            | Op::AsOf { .. }
            | Op::Window { .. }
            | Op::WindowAgg { .. } => None,
            _ => Some("n".to_owned()),
        })
        .collect()
}

/// The tenant tree stored in the view as `TenantParent` link nodes.
fn stored_tenant_tree(view: &GraphView) -> TenantTree {
    let mut tree = TenantTree::default();
    let mut shared = Vec::new();
    for (_, map) in nodes_with(view, TENANT_PARENT_LABEL.as_bytes()) {
        if !is_label(&map, TENANT_PARENT_LABEL) {
            continue;
        }
        let (Some(tenant), Some(parent)) = (text(&map, "tenant"), text(&map, "parent")) else {
            continue;
        };
        if let Ok(link) = TenantParent::new(tenant, parent) {
            let _ = tree.link(link);
        }
        if map.get("shared").and_then(Value::as_bool) == Some(true) {
            if let Ok(parent) = TenantId::new(parent) {
                shared.push(parent);
            }
        }
    }
    for tenant in shared {
        tree.share(tenant);
    }
    tree
}

/// The outcome of narrowing a view to what the caller may read.
pub struct ScopedView {
    /// The narrowed view (the input view when nothing was hidden).
    pub view: Arc<GraphView>,
    /// Unit ids hidden from the caller.
    pub hidden_count: usize,
    /// Up to [`MAX_AUDITED_UNIT_IDS`] ids of the units the caller can see.
    pub unit_ids: Vec<String>,
}

/// Narrow `view` to the caller's tenant and visible units. Refuses an unscopable
/// plan (`QUERY_UNSCOPABLE`) before anything runs.
pub fn scope_view(
    view: Arc<GraphView>,
    caller: &ReadCaller,
    plan_variables: &[String],
) -> Result<ScopedView, TenancyError> {
    let mut hidden: BTreeSet<String> = BTreeSet::new();
    if let Some(tenant) = caller.tenant.as_ref() {
        // The predicate binds to the first node variable of the plan; the view is
        // narrowed so it holds for every variable and every entry point alike.
        scope_predicate(plan_variables, tenant)?;
        let tree = stored_tenant_tree(&view);
        let accessible = tree.accessible_graphs(tenant);
        for (id, map) in nodes_with(&view, TENANT_KEY.as_bytes()) {
            if let Some(owner_tenant) = text(&map, TENANT_KEY).and_then(|t| TenantId::new(t).ok()) {
                if !accessible.graphs.contains(&tenant_graph(&owner_tenant)) {
                    hidden.insert(id.clone());
                }
            }
        }
    }
    let scan = scan_units(&view, caller);
    let hidden_units = scan.hidden.len();
    hidden.extend(scan.hidden);
    hidden.extend(hidden_metadata(&view, caller));
    let mut unit_ids = scan.visible;
    unit_ids.truncate(MAX_AUDITED_UNIT_IDS);
    let redacted: Vec<String> = redacted_nodes(&view, caller)
        .into_iter()
        .filter(|id| !hidden.contains(id))
        .collect();
    if hidden.is_empty() && redacted.is_empty() {
        return Ok(ScopedView {
            view,
            hidden_count: 0,
            unit_ids,
        });
    }
    let mut narrowed = (*view).clone();
    for id in &hidden {
        if let Some(index) = narrowed.node_map.remove(id) {
            narrowed.graph.remove_node(index);
        }
        narrowed.node_properties.remove(id);
    }
    narrowed
        .edge_properties
        .retain(|(s, t), _| !hidden.contains(s) && !hidden.contains(t));
    if !redacted.is_empty() {
        let placeholder = Arc::new(
            rmp_serde::to_vec_named(&serde_json::json!({ (REDACTED_KEY): true }))
                .unwrap_or_default(),
        );
        for id in &redacted {
            narrowed
                .node_properties
                .insert(id.clone(), Arc::clone(&placeholder));
        }
    }
    Ok(ScopedView {
        view: Arc::new(narrowed),
        hidden_count: hidden_units,
        unit_ids,
    })
}

/// Point read of one present node: a `KnowledgeUnit` the caller may not see
/// refuses `UNIT_OUT_OF_SCOPE` naming the id only. Any other node passes. The
/// view is built only for a unit.
pub fn check_unit_read(
    id: &str,
    blob: &[u8],
    view: impl FnOnce() -> Arc<GraphView>,
    caller: &ReadCaller,
) -> Result<(), UnitOutOfScope> {
    if !contains(blob, KNOWLEDGE_UNIT_LABEL.as_bytes()) {
        return Ok(());
    }
    let view = view();
    let is_unit = view
        .node_row_object(id)
        .is_some_and(|map| is_label(&map, KNOWLEDGE_UNIT_LABEL));
    if !is_unit || scan_units(&view, caller).visible.iter().any(|v| v == id) {
        Ok(())
    } else {
        Err(UnitOutOfScope(id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;

    fn put(core: &GraphCore, id: &str, props: Value) {
        core.add_node(id.into(), rmp_serde::to_vec_named(&props).unwrap());
    }

    fn unit(core: &GraphCore, id: &str, owner: &str, scope: &str) {
        put(
            core,
            id,
            serde_json::json!({
                "node_type": "KnowledgeUnit", "owner": owner, "owner_tenant": "acme",
                "version": 1, "share_scope": scope,
            }),
        );
    }

    fn caller(id: &str, tenant: &str) -> ReadCaller {
        ReadCaller {
            principal: UnitPrincipal {
                id: id.into(),
                placement: "g".into(),
                tenant: tenant.into(),
            },
            tenant: TenantId::new(tenant).ok(),
            held_markings: BTreeSet::new(),
        }
    }

    /// Verifies MEDULLA-IDENTITY-R023.3: Wiring: visible-unit filter on the served read path
    // spec: MEDULLA-IDENTITY-R023.3
    #[test]
    fn view_is_narrowed_to_visible_units() {
        let core = GraphCore::new();
        unit(&core, "private", "a", "private");
        unit(&core, "shared", "a", "tenant");
        put(&core, "plain", serde_json::json!({"node_type": "Doc"}));
        let view = Arc::new(core.analysis_snapshot());
        let vars = vec!["n".to_owned()];
        let b = scope_view(view.clone(), &caller("b", "acme"), &vars).unwrap();
        assert_eq!(b.hidden_count, 1);
        assert!(b.view.has_node("shared") && b.view.has_node("plain"));
        assert!(!b.view.has_node("private"));
        let c = scope_view(view, &caller("c", "other"), &vars).unwrap();
        assert!(!c.view.has_node("shared") && !c.view.has_node("private"));
    }

    /// Verifies MEDULLA-IDENTITY-R610.4: Refusal: unscopable read query
    // spec: MEDULLA-IDENTITY-R610.4
    #[test]
    fn plan_without_variable_is_unscopable() {
        let view = Arc::new(GraphCore::new().analysis_snapshot());
        let refused = scope_view(view, &caller("a", "acme"), &[]).err().unwrap();
        assert_eq!(refused.code(), "QUERY_UNSCOPABLE");
    }

    /// Verifies MEDULLA-IDENTITY-R023.4: Refusal: point read outside scope
    // spec: MEDULLA-IDENTITY-R023.4
    #[test]
    fn hidden_unit_point_read_names_id_only() {
        let core = GraphCore::new();
        unit(&core, "private", "a", "private");
        let blob = core.get_node_properties("private").unwrap();
        let view = Arc::new(core.analysis_snapshot());
        let refused = check_unit_read("private", &blob, || view, &caller("b", "acme")).unwrap_err();
        assert_eq!(refused.to_string(), "UNIT_OUT_OF_SCOPE: private");
    }

    /// Verifies MEDULLA-IDENTITY-R726: ownership is keyed by (tenant, principal)
    // spec: MEDULLA-IDENTITY-R726
    #[test]
    fn same_principal_id_in_two_tenants_sees_only_its_own_units() {
        let core = GraphCore::new();
        for (id, tenant) in [("acme-private", "acme"), ("other-private", "other")] {
            put(
                &core,
                id,
                serde_json::json!({
                    "node_type": "KnowledgeUnit", "owner": "ann", "owner_tenant": tenant,
                    "version": 1, "share_scope": "private",
                }),
            );
        }
        let view = Arc::new(core.analysis_snapshot());
        let vars = vec!["n".to_owned()];
        for (tenant, own, foreign) in [
            ("acme", "acme-private", "other-private"),
            ("other", "other-private", "acme-private"),
        ] {
            let scoped = scope_view(view.clone(), &caller("ann", tenant), &vars).unwrap();
            assert!(scoped.view.has_node(own), "{tenant} sees its own unit");
            assert!(
                !scoped.view.has_node(foreign),
                "{tenant} must not see {foreign}"
            );
            let scan = scan_units(&view, &caller("ann", tenant));
            assert_eq!(scan.visible, [own]);
        }
    }

    /// Verifies MEDULLA-TYPED-PACKS-R652.6: a marked node is redacted for a caller who
    /// lacks the marking and read in full by a holder.
    // spec: MEDULLA-TYPED-PACKS-R652.6
    #[test]
    fn marked_node_is_redacted_for_a_non_holder() {
        let core = GraphCore::new();
        put(
            &core,
            "marked",
            serde_json::json!({"type": "Column", "_markings": ["pii"]}),
        );
        put(&core, "plain", serde_json::json!({"type": "Column"}));
        let view = Arc::new(core.analysis_snapshot());
        let vars = vec!["n".to_owned()];
        let mut holder = caller("a", "acme");
        holder.held_markings.insert("pii".to_owned());
        let held = scope_view(view.clone(), &holder, &vars).unwrap();
        assert_eq!(held.view.node_row_object("marked").unwrap()["type"], "Column");
        let denied = scope_view(view, &caller("b", "acme"), &vars).unwrap();
        let redacted = denied.view.node_row_object("marked").unwrap();
        assert_eq!(redacted.len(), 1);
        assert_eq!(redacted[REDACTED_KEY], true);
        assert_eq!(denied.view.node_row_object("plain").unwrap()["type"], "Column");
    }
}
