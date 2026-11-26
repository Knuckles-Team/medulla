//! Dashboard asset rules (MEDULLA-TYPED-PACKS-R109.2).
//!
//! Pure functions over `medulla_types::dashboard`: who may do what to a stored dashboard, how a
//! save advances the revision and rebuilds the links, how a delete keeps the body for the
//! retention window and when a purge is due. The handler (`Method::DashboardAsset`) and the
//! scheduler tick (`purge_due_dashboards`) import this module and never the other way round.

use std::collections::BTreeSet;

use medulla_types::contract::ServerErrorCode;
use medulla_types::dashboard::{
    DashboardAsset, DashboardBody, DashboardGrant, DashboardRole, DASHBOARD_BODY_TOO_LARGE,
};

const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

/// The stored state of one dashboard asset node.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardState {
    /// The asset: id, body, revision, owner and grants. The body is empty once purged.
    pub asset: DashboardAsset,
    /// When it was deleted; `None` while live.
    pub removed_at_ms: Option<u64>,
    /// How many reads the handler has counted.
    pub usage_count: u64,
    /// When the handler last served a read.
    pub last_viewed_ms: Option<u64>,
    /// True once the purge nulled the body.
    pub purged: bool,
}

/// The verified caller: the principal and the groups its roles name.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    /// The verified principal.
    pub principal: &'a str,
    /// The groups the principal belongs to.
    pub groups: &'a [String],
}

/// What the caller may do to a dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// Read.
    View,
    /// Read and save.
    Edit,
    /// Read, save, share, delete and restore.
    Owner,
}

/// Why a dashboard operation was refused. A refused operation writes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardRefusal {
    /// The save carried a stale revision.
    RevisionConflict {
        /// The revision the store holds.
        current: u64,
    },
    /// No grant covers the call.
    NotGranted,
    /// A panel with no query, or a body over the size knob.
    BodyInvalid(String),
    /// The body was purged after its retention.
    Purged,
    /// Any other malformed request.
    Invalid(String),
}

impl DashboardRefusal {
    /// The wire code of the refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RevisionConflict { .. } => ServerErrorCode::DashboardRevisionConflict.as_str(),
            Self::NotGranted => ServerErrorCode::DashboardNotGranted.as_str(),
            Self::BodyInvalid(_) => ServerErrorCode::DashboardBodyInvalid.as_str(),
            Self::Purged => ServerErrorCode::DashboardPurged.as_str(),
            Self::Invalid(_) => "INVALID_ARGUMENT",
        }
    }
}

impl std::fmt::Display for DashboardRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionConflict { current } => write!(
                f,
                "{}: the dashboard is at revision {current}; read it again before saving",
                self.code()
            ),
            Self::NotGranted => write!(f, "{}: no grant covers this call", self.code()),
            Self::BodyInvalid(detail) | Self::Invalid(detail) => {
                write!(f, "{}: {detail}", self.code())
            }
            Self::Purged => write!(f, "{}: the body was purged after its retention", self.code()),
        }
    }
}

/// What the caller may do: the owner owns, a grant naming the principal or one of its groups
/// gives its role.
pub fn access_of(asset: &DashboardAsset, caller: &Caller<'_>) -> Option<Access> {
    if asset.owner == caller.principal {
        return Some(Access::Owner);
    }
    asset
        .grants
        .iter()
        .filter(|grant| {
            grant.principal_or_group == caller.principal
                || caller.groups.iter().any(|g| *g == grant.principal_or_group)
        })
        .map(|grant| match grant.role {
            DashboardRole::View => Access::View,
            DashboardRole::Edit => Access::Edit,
        })
        .max()
}

/// The links one save rebuilds: each panel's data source, and the terms and concepts it names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DashboardLinks {
    /// Data sources the panels read, in sorted order.
    pub sources: Vec<String>,
    /// Glossary terms the panels name, in sorted order.
    pub terms: Vec<String>,
    /// Ontology concepts the panels name, in sorted order.
    pub concepts: Vec<String>,
}

/// The links a body names.
pub fn links_of(body: &DashboardBody) -> DashboardLinks {
    let mut sources = BTreeSet::new();
    let mut terms = BTreeSet::new();
    let mut concepts = BTreeSet::new();
    for panel in &body.panels {
        sources.insert(panel.source.clone());
        terms.extend(panel.terms.iter().cloned());
        concepts.extend(panel.concepts.iter().cloned());
    }
    DashboardLinks {
        sources: sources.into_iter().collect(),
        terms: terms.into_iter().collect(),
        concepts: concepts.into_iter().collect(),
    }
}

/// An accepted save: what the handler writes.
#[derive(Debug, Clone, PartialEq)]
pub struct SavePlan {
    /// The asset as stored after the save.
    pub asset: DashboardAsset,
    /// The body as the JSON text the nodes store.
    pub body_json: String,
    /// The revision the save replaced; 0 for a new dashboard.
    pub prior_revision: u64,
    /// The principal that saved.
    pub saved_by: String,
    /// The request time.
    pub saved_at_ms: u64,
    /// The links the save rebuilds.
    pub links: DashboardLinks,
}

fn check_body(body: &DashboardBody) -> Result<(), DashboardRefusal> {
    if body.panels.iter().any(|panel| panel.query.trim().is_empty()) {
        return Err(DashboardRefusal::BodyInvalid(
            "a panel names no query".to_string(),
        ));
    }
    Ok(())
}

fn live_or_refuse(state: &DashboardState) -> Result<(), DashboardRefusal> {
    if state.purged {
        return Err(DashboardRefusal::Purged);
    }
    Ok(())
}

/// Accept or refuse a save. A save carries the revision it read (0 for a new dashboard); the
/// engine accepts it only when that revision is current, stores the body and raises the
/// revision by one. A refused save never writes.
pub fn save(
    current: Option<&DashboardState>,
    id: &str,
    body: DashboardBody,
    read_revision: u64,
    caller: &Caller<'_>,
    now_ms: u64,
) -> Result<SavePlan, DashboardRefusal> {
    let (owner, grants, prior_revision) = match current {
        None => {
            if read_revision != 0 {
                return Err(DashboardRefusal::RevisionConflict { current: 0 });
            }
            (caller.principal.to_string(), Vec::new(), 0)
        }
        Some(state) => {
            match access_of(&state.asset, caller) {
                Some(Access::Edit | Access::Owner) => {}
                _ => return Err(DashboardRefusal::NotGranted),
            }
            live_or_refuse(state)?;
            if state.removed_at_ms.is_some() {
                return Err(DashboardRefusal::Invalid(
                    "the dashboard is deleted; restore it before saving".to_string(),
                ));
            }
            if read_revision != state.asset.revision {
                return Err(DashboardRefusal::RevisionConflict {
                    current: state.asset.revision,
                });
            }
            (
                state.asset.owner.clone(),
                state.asset.grants.clone(),
                state.asset.revision,
            )
        }
    };
    check_body(&body)?;
    let links = links_of(&body);
    let asset = DashboardAsset {
        id: id.to_string(),
        body,
        revision: prior_revision + 1,
        owner,
        grants,
    };
    asset.validate().map_err(|message| {
        if message.starts_with(DASHBOARD_BODY_TOO_LARGE) {
            DashboardRefusal::BodyInvalid(message)
        } else {
            DashboardRefusal::Invalid(message)
        }
    })?;
    let body_json = serde_json::to_string(&asset.body)
        .map_err(|error| DashboardRefusal::Invalid(format!("body does not encode: {error}")))?;
    Ok(SavePlan {
        asset,
        body_json,
        prior_revision,
        saved_by: caller.principal.to_string(),
        saved_at_ms: now_ms,
        links,
    })
}

fn owner_only(state: &DashboardState, caller: &Caller<'_>) -> Result<(), DashboardRefusal> {
    match access_of(&state.asset, caller) {
        Some(Access::Owner) => Ok(()),
        _ => Err(DashboardRefusal::NotGranted),
    }
}

/// Replace the grants. Only the owner may; a grant names its principal or group once.
pub fn share(
    state: &DashboardState,
    grants: Vec<DashboardGrant>,
    caller: &Caller<'_>,
) -> Result<Vec<DashboardGrant>, DashboardRefusal> {
    owner_only(state, caller)?;
    live_or_refuse(state)?;
    if state.removed_at_ms.is_some() {
        return Err(DashboardRefusal::Invalid(
            "the dashboard is deleted; restore it before sharing".to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    for grant in &grants {
        if grant.principal_or_group.trim().is_empty() || !seen.insert(grant.principal_or_group.as_str())
        {
            return Err(DashboardRefusal::Invalid(
                "a grant needs a principal or group, once".to_string(),
            ));
        }
    }
    Ok(grants)
}

fn grantable(state: &DashboardState, caller: &Caller<'_>) -> Result<(), DashboardRefusal> {
    owner_only(state, caller)?;
    live_or_refuse(state)?;
    if state.removed_at_ms.is_some() {
        return Err(DashboardRefusal::Invalid(
            "the dashboard is deleted; restore it before changing grants".to_string(),
        ));
    }
    Ok(())
}

/// Set one grantee's role (MEDULLA-TYPED-PACKS-R109.8): the stored grants with the grant of
/// `grant.principal_or_group` replaced in place, or appended when there is none. Only the owner
/// may; the grantee must be named. The result is a pure function of the stored grants and the
/// one grant, so concurrent calls for different grantees commute.
pub fn grant(
    state: &DashboardState,
    grant: DashboardGrant,
    caller: &Caller<'_>,
) -> Result<Vec<DashboardGrant>, DashboardRefusal> {
    grantable(state, caller)?;
    if grant.principal_or_group.trim().is_empty() {
        return Err(DashboardRefusal::Invalid(
            "a grant needs a principal or group, once".to_string(),
        ));
    }
    let mut grants = state.asset.grants.clone();
    match grants
        .iter_mut()
        .find(|stored| stored.principal_or_group == grant.principal_or_group)
    {
        Some(stored) => stored.role = grant.role,
        None => grants.push(grant),
    }
    Ok(grants)
}

/// Remove one grantee's grant (MEDULLA-TYPED-PACKS-R109.8): the stored grants without the grant
/// of `principal_or_group`. A grantee with no grant leaves the list as stored. Only the owner
/// may; the grantee must be named.
pub fn revoke(
    state: &DashboardState,
    principal_or_group: &str,
    caller: &Caller<'_>,
) -> Result<Vec<DashboardGrant>, DashboardRefusal> {
    grantable(state, caller)?;
    if principal_or_group.trim().is_empty() {
        return Err(DashboardRefusal::Invalid(
            "a revoke needs a principal or group".to_string(),
        ));
    }
    Ok(state
        .asset
        .grants
        .iter()
        .filter(|stored| stored.principal_or_group != principal_or_group)
        .cloned()
        .collect())
}

/// Soft delete: returns the `removed_at_ms` to store. Only the owner may. When
/// `expected_revision` is given it must be the stored revision, else the delete refuses
/// `RevisionConflict` naming the current one (MEDULLA-TYPED-PACKS-R109.7). A delete is not a
/// view: it neither reads nor changes `usage_count` and `last_viewed_ms`.
pub fn delete(
    state: &DashboardState,
    caller: &Caller<'_>,
    now_ms: u64,
    expected_revision: Option<u64>,
) -> Result<u64, DashboardRefusal> {
    owner_only(state, caller)?;
    live_or_refuse(state)?;
    if let Some(expected) = expected_revision {
        if expected != state.asset.revision {
            return Err(DashboardRefusal::RevisionConflict {
                current: state.asset.revision,
            });
        }
    }
    if state.removed_at_ms.is_some() {
        return Err(DashboardRefusal::Invalid(
            "the dashboard is already deleted".to_string(),
        ));
    }
    Ok(now_ms)
}

/// Whether a deleted dashboard's retention has ended: its body may be purged. A live or already
/// purged dashboard is never due.
pub fn purge_due(state: &DashboardState, now_ms: u64, retention_days: u64) -> bool {
    match state.removed_at_ms {
        Some(removed) if !state.purged => {
            now_ms >= retention_ends_ms(removed, retention_days)
        }
        _ => false,
    }
}

/// When the retention of a dashboard deleted at `removed_at_ms` ends.
pub fn retention_ends_ms(removed_at_ms: u64, retention_days: u64) -> u64 {
    removed_at_ms.saturating_add(retention_days.saturating_mul(MS_PER_DAY))
}

/// Bring a deleted dashboard back inside its retention. Only the owner may; after the retention
/// the body is gone and the call is refused as purged.
pub fn restore(
    state: &DashboardState,
    caller: &Caller<'_>,
    now_ms: u64,
    retention_days: u64,
) -> Result<(), DashboardRefusal> {
    owner_only(state, caller)?;
    live_or_refuse(state)?;
    if state.removed_at_ms.is_none() {
        return Err(DashboardRefusal::Invalid(
            "the dashboard is not deleted".to_string(),
        ));
    }
    if purge_due(state, now_ms, retention_days) {
        return Err(DashboardRefusal::Purged);
    }
    Ok(())
}

/// The counters a read stores: `usage_count` raised by one and `last_viewed_ms` set.
pub fn view(
    state: &DashboardState,
    caller: &Caller<'_>,
    now_ms: u64,
) -> Result<(u64, u64), DashboardRefusal> {
    let Some(access) = access_of(&state.asset, caller) else {
        return Err(DashboardRefusal::NotGranted);
    };
    // A deleted dashboard is read only by its owner, who may restore it.
    if state.removed_at_ms.is_some() && access != Access::Owner {
        return Err(DashboardRefusal::NotGranted);
    }
    live_or_refuse(state)?;
    Ok((state.usage_count.saturating_add(1), now_ms))
}

/// The node properties of a dashboard asset node, one typed property per field. The body and
/// the grants are JSON text; `removed_at_ms` and `last_viewed_ms` are null while unset.
pub fn node_properties(
    state: &DashboardState,
    body_json: Option<&str>,
    tenant: &str,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::{json, Value};
    let live = state.removed_at_ms.is_none() && !state.purged;
    let mut terms = BTreeSet::new();
    let mut concepts = BTreeSet::new();
    if live {
        for panel in &state.asset.body.panels {
            terms.extend(panel.terms.iter().cloned());
            concepts.extend(panel.concepts.iter().cloned());
        }
    }
    // A deleted dashboard leaves search: its ranking text is emptied, so no query scores it.
    let label = if live { state.asset.id.as_str() } else { "" };
    let mut props = serde_json::Map::new();
    props.insert("type".into(), medulla_types::dashboard::DASHBOARD_ASSET_LABEL.into());
    props.insert("tenant_id".into(), tenant.into());
    props.insert("dashboard_id".into(), state.asset.id.clone().into());
    props.insert("name".into(), label.into());
    props.insert(
        "http://www.w3.org/2004/02/skos/core#prefLabel".into(),
        label.into(),
    );
    props.insert("term_labels".into(), json!(terms.into_iter().collect::<Vec<_>>()));
    props.insert("concepts".into(), json!(concepts.into_iter().collect::<Vec<_>>()));
    props.insert("revision".into(), state.asset.revision.into());
    props.insert(
        "body".into(),
        body_json.map_or(Value::Null, |text| Value::String(text.to_string())),
    );
    props.insert("owner".into(), state.asset.owner.clone().into());
    props.insert(
        "grants".into(),
        Value::String(serde_json::to_string(&state.asset.grants).unwrap_or_else(|_| "[]".into())),
    );
    props.insert(
        "removed_at_ms".into(),
        state.removed_at_ms.map_or(Value::Null, Value::from),
    );
    props.insert("usage_count".into(), state.usage_count.into());
    props.insert(
        "last_viewed_ms".into(),
        state.last_viewed_ms.map_or(Value::Null, Value::from),
    );
    props
}

/// Read a dashboard state back from its node properties; `None` when the node is not a
/// dashboard asset or is malformed. A null body reads as purged.
pub fn state_from_properties(
    props: &serde_json::Map<String, serde_json::Value>,
) -> Option<DashboardState> {
    use serde_json::Value;
    if props.get("type").and_then(Value::as_str)
        != Some(medulla_types::dashboard::DASHBOARD_ASSET_LABEL)
    {
        return None;
    }
    let text = |key: &str| props.get(key).and_then(Value::as_str);
    let number = |key: &str| props.get(key).and_then(Value::as_u64);
    let (body, purged) = match props.get("body") {
        Some(Value::String(raw)) => (serde_json::from_str::<DashboardBody>(raw).ok()?, false),
        Some(Value::Null) | None => (
            DashboardBody {
                panels: Vec::new(),
                layout: Value::Null,
                parameters: Value::Null,
            },
            true,
        ),
        _ => return None,
    };
    let grants = match props.get("grants") {
        Some(Value::String(raw)) => serde_json::from_str::<Vec<DashboardGrant>>(raw).ok()?,
        _ => Vec::new(),
    };
    Some(DashboardState {
        asset: DashboardAsset {
            id: text("dashboard_id")?.to_string(),
            body,
            revision: number("revision")?,
            owner: text("owner")?.to_string(),
            grants,
        },
        removed_at_ms: number("removed_at_ms"),
        usage_count: number("usage_count").unwrap_or(0),
        last_viewed_ms: number("last_viewed_ms"),
        purged,
    })
}

/// The batch operations of a purge: the body becomes null on the asset node and on every
/// revision node, and the revision nodes themselves stay (revision, digest, saver, time).
pub fn purge_operations(id: &str, revision: u64) -> Vec<serde_json::Value> {
    use medulla_types::dashboard::{dashboard_node_id, dashboard_revision_node_id};
    let null_body = || serde_json::json!({"body": serde_json::Value::Null});
    let mut operations = vec![serde_json::json!({
        "op": "upsert_node",
        "id": dashboard_node_id(id),
        "properties": null_body(),
    })];
    for n in 1..=revision {
        operations.push(serde_json::json!({
            "op": "upsert_node",
            "id": dashboard_revision_node_id(id, n),
            "properties": null_body(),
        }));
    }
    operations
}
