//! `Method::DashboardAsset`: the served dashboard catalog asset (MEDULLA-TYPED-PACKS-R109.3).
//!
//! A dashboard is the node `dashboard:<id>` (label `DashboardAsset`) with one history node
//! `dashboard_revision:<id>:<n>` per save, written through the ordinary graph gateway
//! (`dispatch_graph_op`) so each write is ACL-checked, durable, audited and CDC-emitted. The rules
//! are the pure functions of `medulla_core::dashboard`; this module reads the stored asset, checks
//! the caller's grant, decides every refusal before the first write and commits one batch.
//!
//! * `Save` stores the body as the next revision, writes the history node and rebuilds the
//!   lineage links (`cat:lineageTo` from each panel's data source, with its `cat:LineageEdge`
//!   node) and the term links (`definesAsset`) in the same batch. A source or term with no node
//!   in the graph gets no link.
//! * `Grant` and `Revoke` change one grantee's grant and leave the others as stored. They commit
//!   as a compare-and-set on the stored grants and re-decide on a lost race, so two callers
//!   changing different grantees both land and no caller has to read-modify-write the list.
//! * `Share` replaces the grants, `Delete` soft-deletes, `Restore` brings a deleted dashboard back
//!   inside its retention; each needs the owner.
//! * `Get` checks the grant, counts the view and answers the body.
//!
//! The asset leaves search on delete (its ranking text is emptied) and re-enters on save and
//! restore. Reads of many dashboards are UQL over the asset nodes. The purge of a body after its
//! retention runs from the executor's schedule tick, not from here.

use std::sync::Arc;

use medulla_core::dashboard::{
    delete, grant, node_properties, restore, retention_ends_ms, revoke, save, share,
    state_from_properties, view, Caller, DashboardRefusal, DashboardState,
};
use medulla_protocol_types::protocol::{Method, Response};
use medulla_protocol_types::result_contract::ingestion::{
    DashboardAssetDelete, DashboardAssetGet, DashboardAssetGrant, DashboardAssetRestore,
    DashboardAssetRevoke, DashboardAssetSave, DashboardAssetShare,
};
use medulla_server_api::state::ServerState;
use medulla_types::contract::ServerErrorCode;
use medulla_types::dashboard::{
    dashboard_node_id, dashboard_retention_days, dashboard_revision_node_id, DashboardAssetOp,
    DashboardGrant, DashboardReceipt, DASHBOARD_REVISION_LABEL,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::graph_client::{
    encode, read_node, took_effect, write, write_batch, GraphTarget, Properties, TENANT_PROPERTY,
};
use super::shared::typed_response;

const CAT_LINEAGE_TO: &str = "https://knuckles-team.github.io/ontology/catalog#lineageTo";
const LINEAGE_EDGE_LABEL: &str = "cat:LineageEdge";
/// The hop type of a lineage edge from a panel's data source to the dashboard.
const HOP_RESOURCE: &str = "resource";
const LINK_SOURCES: &str = "link_sources";
const LINK_TERMS: &str = "link_terms";

/// Serve one `DashboardAsset` op.
pub async fn dispatch_dashboard(
    state: &Arc<RwLock<ServerState>>,
    target: GraphTarget<'_>,
    op: DashboardAssetOp,
) -> Response {
    let req_id = target.req_id;
    let result = serve(state, &target, &op).await;
    match op {
        DashboardAssetOp::Save { .. } => typed_response::<DashboardAssetSave>(req_id, result),
        DashboardAssetOp::Share { .. } => typed_response::<DashboardAssetShare>(req_id, result),
        DashboardAssetOp::Grant { .. } => typed_response::<DashboardAssetGrant>(req_id, result),
        DashboardAssetOp::Revoke { .. } => typed_response::<DashboardAssetRevoke>(req_id, result),
        DashboardAssetOp::Delete { .. } => typed_response::<DashboardAssetDelete>(req_id, result),
        DashboardAssetOp::Restore { .. } => typed_response::<DashboardAssetRestore>(req_id, result),
        DashboardAssetOp::Get { .. } => typed_response::<DashboardAssetGet>(req_id, result),
    }
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

fn refused(refusal: DashboardRefusal) -> String {
    refusal.to_string()
}

fn receipt(state: &DashboardState) -> DashboardReceipt {
    DashboardReceipt {
        id: state.asset.id.clone(),
        revision: state.asset.revision,
        owner: state.asset.owner.clone(),
        grants: state.asset.grants.clone(),
        body: (!state.purged).then(|| state.asset.body.clone()),
        removed_at_ms: state.removed_at_ms,
        usage_count: state.usage_count,
        last_viewed_ms: state.last_viewed_ms,
    }
}

fn body_text(state: &DashboardState) -> Result<String, String> {
    serde_json::to_string(&state.asset.body)
        .map_err(|error| invalid(format!("the stored body does not encode: {error}")))
}

fn text_list(properties: &Properties, key: &str) -> Vec<String> {
    properties
        .get(key)
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}

/// The stored dashboard and its raw properties; `None` when no such node exists. A node of
/// another tenant is refused as not granted, never answered.
async fn load(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
) -> Result<Option<(DashboardState, Properties)>, String> {
    if id.trim().is_empty() || id.len() > 512 {
        return Err(invalid("a dashboard id is 1 to 512 bytes"));
    }
    let Some(properties) = read_node(state, target, &dashboard_node_id(id)).await? else {
        return Ok(None);
    };
    if properties.get(TENANT_PROPERTY).and_then(Value::as_str) != Some(target.verified.tenant()) {
        return Err(refused(DashboardRefusal::NotGranted));
    }
    let Some(stored) = state_from_properties(&properties) else {
        return Err(invalid(format!(
            "node '{}' is not a dashboard asset",
            dashboard_node_id(id)
        )));
    };
    Ok(Some((stored, properties)))
}

fn operation(id: &str, properties: Properties) -> Value {
    json!({"op": "upsert_node", "id": id, "properties": properties})
}

async fn serve(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    op: &DashboardAssetOp,
) -> Result<DashboardReceipt, String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err(ServerErrorCode::TenantScopeRequired.as_str().to_string());
    }
    let groups: Vec<String> = target.verified.roles().to_vec();
    let caller = Caller {
        principal: target.verified.agent_id(),
        groups: &groups,
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    match op {
        DashboardAssetOp::Save { id, body, revision } => {
            save_dashboard(state, target, &caller, id, body.clone(), *revision, now_ms).await
        }
        DashboardAssetOp::Share { id, grants } => {
            let (stored, _) = required(load(state, target, id).await?, id)?;
            let grants = share(&stored, grants.clone(), &caller).map_err(refused)?;
            let mut next = stored.clone();
            next.asset.grants = grants;
            let mut properties = Properties::new();
            properties.insert(
                "grants".into(),
                serde_json::to_string(&next.asset.grants)
                    .map_err(|error| invalid(error))?
                    .into(),
            );
            properties.insert("grants_by".into(), caller.principal.into());
            properties.insert("grants_at_ms".into(), now_ms.into());
            commit(
                state,
                target,
                vec![operation(&dashboard_node_id(id), properties)],
            )
            .await?;
            Ok(receipt(&next))
        }
        DashboardAssetOp::Grant { id, grant: one } => {
            change_grants(state, target, &caller, id, now_ms, |stored| {
                grant(stored, one.clone(), &caller)
            })
            .await
        }
        DashboardAssetOp::Revoke {
            id,
            principal_or_group,
        } => {
            change_grants(state, target, &caller, id, now_ms, |stored| {
                revoke(stored, principal_or_group, &caller)
            })
            .await
        }
        DashboardAssetOp::Delete {
            id,
            expected_revision,
        } => {
            let (stored, _) = required(load(state, target, id).await?, id)?;
            let removed = delete(&stored, &caller, now_ms, *expected_revision).map_err(refused)?;
            let mut next = stored.clone();
            next.removed_at_ms = Some(removed);
            let mut properties = node_properties(&next, Some(&body_text(&next)?), tenant);
            properties.insert(
                "retention_ends_ms".into(),
                retention_ends_ms(removed, dashboard_retention_days()).into(),
            );
            properties.insert("removed_by".into(), caller.principal.into());
            commit(
                state,
                target,
                vec![operation(&dashboard_node_id(id), properties)],
            )
            .await?;
            Ok(receipt(&next))
        }
        DashboardAssetOp::Restore { id } => {
            let (stored, _) = required(load(state, target, id).await?, id)?;
            restore(&stored, &caller, now_ms, dashboard_retention_days()).map_err(refused)?;
            let mut next = stored.clone();
            next.removed_at_ms = None;
            let mut properties = node_properties(&next, Some(&body_text(&next)?), tenant);
            properties.insert("retention_ends_ms".into(), Value::Null);
            properties.insert("removed_by".into(), Value::Null);
            commit(
                state,
                target,
                vec![operation(&dashboard_node_id(id), properties)],
            )
            .await?;
            Ok(receipt(&next))
        }
        DashboardAssetOp::Get { id } => {
            let (stored, _) = required(load(state, target, id).await?, id)?;
            let (usage_count, viewed_ms) = view(&stored, &caller, now_ms).map_err(refused)?;
            let mut properties = Properties::new();
            properties.insert("usage_count".into(), usage_count.into());
            properties.insert("last_viewed_ms".into(), viewed_ms.into());
            commit(
                state,
                target,
                vec![operation(&dashboard_node_id(id), properties)],
            )
            .await?;
            let mut next = stored;
            next.usage_count = usage_count;
            next.last_viewed_ms = Some(viewed_ms);
            Ok(receipt(&next))
        }
    }
}

/// How many times a `Grant` or `Revoke` re-reads and re-decides after losing the compare-and-set
/// to a concurrent change of the same dashboard's grants.
const MAX_GRANT_ATTEMPTS: usize = 8;

/// Change the grants of one dashboard by `decide`, a pure function of the stored state. The
/// write is a compare-and-set on the `grants` and `removed_at_ms` the decision was made from: a
/// concurrent change makes it lose, and the call reads again and decides again. A decision that
/// leaves the grants as stored writes nothing.
async fn change_grants(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    caller: &Caller<'_>,
    id: &str,
    now_ms: u64,
    decide: impl Fn(&DashboardState) -> Result<Vec<DashboardGrant>, DashboardRefusal>,
) -> Result<DashboardReceipt, String> {
    for _ in 0..MAX_GRANT_ATTEMPTS {
        let (stored, properties) = required(load(state, target, id).await?, id)?;
        let grants = decide(&stored).map_err(refused)?;
        if grants == stored.asset.grants {
            return Ok(receipt(&stored));
        }
        let mut conditions = Properties::new();
        conditions.insert(TENANT_PROPERTY.into(), target.verified.tenant().into());
        for key in ["grants", "removed_at_ms"] {
            conditions.insert(
                key.into(),
                properties.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        let mut updates = Properties::new();
        updates.insert(
            "grants".into(),
            serde_json::to_string(&grants).map_err(invalid)?.into(),
        );
        updates.insert("grants_by".into(), caller.principal.into());
        updates.insert("grants_at_ms".into(), now_ms.into());
        let committed = write(
            state,
            target,
            Method::CompareAndSetNodeFields {
                node_id: dashboard_node_id(id),
                conditions_msgpack: encode(&conditions)?,
                updates_msgpack: encode(&updates)?,
            },
        )
        .await?;
        if took_effect(&committed) {
            let mut next = stored;
            next.asset.grants = grants;
            return Ok(receipt(&next));
        }
    }
    Err(invalid(format!(
        "the grants of '{id}' kept changing; retry the call"
    )))
}

/// An unknown dashboard reads as no grant: the caller learns nothing about ids it cannot read.
fn required(
    found: Option<(DashboardState, Properties)>,
    _id: &str,
) -> Result<(DashboardState, Properties), String> {
    found.ok_or_else(|| refused(DashboardRefusal::NotGranted))
}

async fn commit(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    operations: Vec<Value>,
) -> Result<(), String> {
    write_batch(state, target, &operations).await
}

async fn save_dashboard(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    caller: &Caller<'_>,
    id: &str,
    body: medulla_types::dashboard::DashboardBody,
    read_revision: u64,
    now_ms: u64,
) -> Result<DashboardReceipt, String> {
    use sha2::{Digest, Sha256};
    let tenant = target.verified.tenant();
    let current = load(state, target, id).await?;
    let plan = save(
        current.as_ref().map(|(stored, _)| stored),
        id,
        body,
        read_revision,
        caller,
        now_ms,
    )
    .map_err(refused)?;
    let asset_node = dashboard_node_id(id);
    let next = DashboardState {
        asset: plan.asset.clone(),
        removed_at_ms: None,
        usage_count: current.as_ref().map_or(0, |(stored, _)| stored.usage_count),
        last_viewed_ms: current
            .as_ref()
            .and_then(|(stored, _)| stored.last_viewed_ms),
        purged: false,
    };

    // The links this save rebuilds: only the sources and terms the graph holds as nodes.
    let (old_sources, old_terms) = current
        .as_ref()
        .map_or((Vec::new(), Vec::new()), |(_, props)| {
            (text_list(props, LINK_SOURCES), text_list(props, LINK_TERMS))
        });
    let mut linked_sources = Vec::new();
    for source in &plan.links.sources {
        if read_node(state, target, source).await?.is_some() {
            linked_sources.push(source.clone());
        }
    }
    let mut linked_terms = Vec::new();
    let mut term_edges = Vec::new();
    for term in &plan.links.terms {
        if let Ok(Some((stored_term, _))) = super::glossary::stored_term(state, target, term).await
        {
            if let Ok(link) = medulla_core::glossary::bind(term, &stored_term, &asset_node) {
                linked_terms.push(term.clone());
                term_edges.push((link.term, link.predicate));
            }
        }
    }

    let mut operations = Vec::new();
    for source in old_sources.iter().filter(|s| !linked_sources.contains(s)) {
        operations.push(json!({"op": "remove_edge", "source": source, "target": asset_node}));
        operations.push(json!({"op": "remove_node", "id": lineage_edge_id(source, &asset_node)}));
    }
    for term in old_terms.iter().filter(|t| !linked_terms.contains(t)) {
        operations.push(json!({"op": "remove_edge", "source": term, "target": asset_node}));
    }

    let mut asset = node_properties(&next, Some(&plan.body_json), tenant);
    asset.insert(
        LINK_SOURCES.into(),
        serde_json::to_string(&linked_sources)
            .map_err(invalid)?
            .into(),
    );
    asset.insert(
        LINK_TERMS.into(),
        serde_json::to_string(&linked_terms)
            .map_err(invalid)?
            .into(),
    );
    asset.insert("saved_by".into(), plan.saved_by.clone().into());
    asset.insert("saved_at_ms".into(), plan.saved_at_ms.into());
    operations.push(operation(&asset_node, asset));

    let mut revision = Properties::new();
    revision.insert("type".into(), DASHBOARD_REVISION_LABEL.into());
    revision.insert(TENANT_PROPERTY.into(), tenant.into());
    revision.insert("dashboard".into(), id.into());
    revision.insert("revision".into(), plan.asset.revision.into());
    revision.insert("prior_revision".into(), plan.prior_revision.into());
    revision.insert(
        "body_sha256".into(),
        format!("sha256:{:x}", Sha256::digest(plan.body_json.as_bytes())).into(),
    );
    revision.insert("saved_by".into(), plan.saved_by.clone().into());
    revision.insert("saved_at_ms".into(), plan.saved_at_ms.into());
    revision.insert("body".into(), plan.body_json.clone().into());
    operations.push(operation(
        &dashboard_revision_node_id(id, plan.asset.revision),
        revision,
    ));

    for source in &linked_sources {
        let mut edge = Properties::new();
        edge.insert("relationship".into(), CAT_LINEAGE_TO.into());
        edge.insert(TENANT_PROPERTY.into(), tenant.into());
        operations.push(json!({
            "op": "upsert_edge", "source": source, "target": asset_node, "properties": edge,
        }));
        let mut hop = Properties::new();
        hop.insert("type".into(), LINEAGE_EDGE_LABEL.into());
        hop.insert(TENANT_PROPERTY.into(), tenant.into());
        hop.insert("from".into(), source.clone().into());
        hop.insert("to".into(), asset_node.clone().into());
        hop.insert("hop".into(), HOP_RESOURCE.into());
        operations.push(operation(&lineage_edge_id(source, &asset_node), hop));
    }
    for (term, predicate) in &term_edges {
        let mut edge = Properties::new();
        edge.insert("relationship".into(), (*predicate).into());
        edge.insert("predicate".into(), (*predicate).into());
        edge.insert(TENANT_PROPERTY.into(), tenant.into());
        operations.push(json!({
            "op": "upsert_edge", "source": term, "target": asset_node, "properties": edge,
        }));
    }
    commit(state, target, operations).await?;
    Ok(receipt(&next))
}

fn lineage_edge_id(source: &str, asset_node: &str) -> String {
    format!("lineage_edge:{source}>{asset_node}")
}
