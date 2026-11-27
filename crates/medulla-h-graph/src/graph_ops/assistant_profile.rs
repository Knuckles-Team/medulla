//! Served assistant profile and document-set store
//! (MEDULLA-DECISION-ENGINE-R656.2).
//!
//! `AssistantProfile` puts are gateway-routed (`Route::AssistantProfile`): the
//! object is admitted against the stored node, then commits as the one
//! `AddNode` upsert through the staged gateway, so it is durable and
//! replicated like every other graph write. A refused put writes nothing.
//! Gets and lists are graph reads over the raw tenant graph that return only
//! objects the caller may read: the owner, or a role holding a `Read` (or
//! `Admin`) share evaluated by the RBAC evaluator of `medulla-isolation`.
//! Each `ProfileShare` becomes one `Grant` on the resource
//! `assistant_profile:<id>` (`document_set:<id>` for a set).
//!
//! The stored object carries the verified tenant scope; the caller's `tenant`
//! field is overwritten by the server, so another tenant never reads or saves
//! into it. A missing object and one the caller may not read answer with one
//! identical, non-disclosing envelope.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_identity_types::acl::RbacAction;
use medulla_identity_types::assistant_profile::{
    document_set_node_id, document_set_resource, profile_node_id, profile_resource,
    resolve_document_set, AssistantProfile, DocumentSet, DocumentSetKind, ProfileOp,
};
use medulla_isolation::isolation::IsolationLayer;
use medulla_isolation::profile_access::{may, profile_unavailable, read_stored, Caller};
use medulla_protocol_types::result_contract::coordination::{
    AssistantProfileGetDocumentSet, AssistantProfileGetProfile, AssistantProfileListDocumentSets,
    AssistantProfileListProfiles, AssistantProfilePutDocumentSet, AssistantProfilePutProfile,
};
use medulla_server_api::knob_config::knobs;

const PROFILE_PREFIX: &str = "assistant_profile:";
const SET_PREFIX: &str = "document_set:";

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The one non-disclosing envelope of an absent or unreadable document set.
fn set_unavailable(set_id: &str) -> String {
    refuse(
        "DOCUMENT_SET_NOT_FOUND",
        format!("document set {set_id} is not available to the caller"),
    )
}

pub(super) fn node_blob<T: serde::Serialize>(
    label: &str,
    tenant_scope: &str,
    object: &T,
) -> Result<Vec<u8>, String> {
    let json = serde_json::to_string(object)
        .map_err(|error| format!("INVALID_ARGUMENT: assistant profile encoding failed: {error}"))?;
    rmp_serde::to_vec_named(&serde_json::json!({
        "type": label,
        "tenant_scope": tenant_scope,
        "json": json,
    }))
    .map_err(|error| format!("INVALID_ARGUMENT: assistant profile encoding failed: {error}"))
}

/// What an admitted put does: the upsert to commit and the stored object.
pub(super) enum Admitted {
    Profile(Box<Method>, AssistantProfile),
    Set(Box<Method>, DocumentSet),
}

/// Admit one put against the stored node, writing nothing. `stored_*` is the
/// node as it is now; the commit re-checks the revision inside the gateway.
pub(super) fn admit_put(
    core: &GraphCore,
    isolation: &IsolationLayer,
    tenant_scope: &str,
    agent_id: &str,
    op: &ProfileOp,
) -> Result<Admitted, String> {
    let caller = Caller::of(isolation, agent_id);
    match op {
        ProfileOp::PutProfile { profile } => {
            let max = usize::try_from(knobs().profile_instructions_max_chars).unwrap_or(usize::MAX);
            profile
                .check(max)
                .map_err(|detail| refuse("INVALID_ARGUMENT", detail))?;
            let node_id = profile_node_id(&profile.profile_id);
            let stored: Option<AssistantProfile> =
                read_stored(core, &node_id, "AssistantProfile", tenant_scope);
            // A node another tenant stored under this id is never overwritten.
            if stored.is_none() && core.get_node_properties(&node_id).is_some() {
                return Err(profile_unavailable(&profile.profile_id));
            }
            let stored_revision = stored.as_ref().map_or(0, |s| s.revision);
            match &stored {
                None if profile.revision > 0 => {
                    return Err(refuse(
                        "PROFILE_NOT_FOUND",
                        format!("assistant profile {} does not exist", profile.profile_id),
                    ));
                }
                None => {
                    if profile.owner_principal != caller.agent_id {
                        return Err(refuse(
                            "ACCESS_DENIED",
                            "a new profile is owned by the caller that saves it",
                        ));
                    }
                }
                Some(stored) => {
                    let resource = profile_resource(&stored.profile_id);
                    if !may(
                        &caller,
                        &stored.owner_principal,
                        &stored.shares,
                        &resource,
                        RbacAction::Read,
                    ) {
                        return Err(profile_unavailable(&profile.profile_id));
                    }
                    if !may(
                        &caller,
                        &stored.owner_principal,
                        &stored.shares,
                        &resource,
                        RbacAction::Admin,
                    ) {
                        return Err(refuse(
                            "ACCESS_DENIED",
                            "only the owner or an Admin grant may save this profile",
                        ));
                    }
                    if profile.owner_principal != stored.owner_principal {
                        return Err(refuse(
                            "ACCESS_DENIED",
                            "the owner of a stored profile does not change on save",
                        ));
                    }
                }
            }
            if profile.revision != stored_revision {
                return Err(refuse(
                    "PROFILE_REVISION_CONFLICT",
                    format!(
                        "profile {} is at revision {stored_revision}, the save was derived from {}",
                        profile.profile_id, profile.revision
                    ),
                ));
            }
            let mut saved = profile.clone();
            saved.tenant = tenant_scope.to_string();
            saved.revision = stored_revision + 1;
            let write = Method::AddNode {
                node_id,
                properties_msgpack: node_blob("AssistantProfile", tenant_scope, &saved)?,
            };
            Ok(Admitted::Profile(Box::new(write), saved))
        }
        ProfileOp::PutDocumentSet { set } => {
            if set.set_id.trim().is_empty()
                || set.name.trim().is_empty()
                || set.owner_principal.trim().is_empty()
            {
                return Err(refuse(
                    "INVALID_ARGUMENT",
                    "set_id, name and owner_principal are required",
                ));
            }
            if let DocumentSetKind::SavedQuery(text) = &set.kind {
                if let Err(error) = medulla_uql::parse(text) {
                    return Err(refuse(
                        "DOCUMENT_SET_QUERY_NOT_READ_ONLY",
                        format!("the saved query is not a read-only query: {error}"),
                    ));
                }
            }
            let node_id = document_set_node_id(&set.set_id);
            let stored: Option<DocumentSet> =
                read_stored(core, &node_id, "DocumentSet", tenant_scope);
            if stored.is_none() && core.get_node_properties(&node_id).is_some() {
                return Err(set_unavailable(&set.set_id));
            }
            match &stored {
                None => {
                    if set.owner_principal != caller.agent_id {
                        return Err(refuse(
                            "ACCESS_DENIED",
                            "a new document set is owned by the caller that saves it",
                        ));
                    }
                }
                Some(stored) => {
                    let resource = document_set_resource(&stored.set_id);
                    if !may(
                        &caller,
                        &stored.owner_principal,
                        &stored.shares,
                        &resource,
                        RbacAction::Read,
                    ) {
                        return Err(set_unavailable(&set.set_id));
                    }
                    if !may(
                        &caller,
                        &stored.owner_principal,
                        &stored.shares,
                        &resource,
                        RbacAction::Admin,
                    ) {
                        return Err(refuse(
                            "ACCESS_DENIED",
                            "only the owner or an Admin grant may save this document set",
                        ));
                    }
                    if set.owner_principal != stored.owner_principal {
                        return Err(refuse(
                            "ACCESS_DENIED",
                            "the owner of a stored document set does not change on save",
                        ));
                    }
                }
            }
            let mut saved = set.clone();
            saved.tenant = tenant_scope.to_string();
            let write = Method::AddNode {
                node_id,
                properties_msgpack: node_blob("DocumentSet", tenant_scope, &saved)?,
            };
            Ok(Admitted::Set(Box::new(write), saved))
        }
        _ => Err(refuse(
            "INVALID_ARGUMENT",
            "AssistantProfile reads are served by the graph read path",
        )),
    }
}

/// The serialisation lane of one stored object: every save of it admits and
/// commits while holding this lane, so the revision compare-and-set is judged
/// against the state no other save can change before this one commits. `None`
/// for an op that is not a save.
pub(super) async fn lock_save_lane(
    graph: &str,
    op: &ProfileOp,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    static LANES: std::sync::OnceLock<Vec<std::sync::Arc<tokio::sync::Mutex<()>>>> =
        std::sync::OnceLock::new();
    let node_id = match op {
        ProfileOp::PutProfile { profile } => profile_node_id(&profile.profile_id),
        ProfileOp::PutDocumentSet { set } => document_set_node_id(&set.set_id),
        _ => return None,
    };
    let key = format!("{graph}\u{0}{node_id}");
    Some(medulla_server_api::routed_write_coalescer::striped_lock(&LANES, &key).await)
}

/// The `Route::AssistantProfile` entry: admit the put, then commit the node
/// upsert through the coalescable gateway, both inside the object's save lane,
/// so two racing saves of one revision cannot both win.
pub(super) async fn handle_put(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::AssistantProfile { op } = method else {
        return None;
    };
    let Some(agent_id) = ctx.caller.filter(|caller| !caller.trim().is_empty()) else {
        return Some(Response::err(
            ctx.req_id,
            refuse(
                "ACCESS_DENIED",
                "an assistant profile save needs a verified caller",
            ),
        ));
    };
    // Held until the commit below returns: admission reads the pre-write state
    // and no other save of this object commits in between.
    let _lane = lock_save_lane(ctx.graph_name, op).await;
    let admitted = match admit_put(ctx.core, ctx.isolation, ctx.tenant_scope, agent_id, op) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    // The compare-and-set ran in `admit_put` under the save lane: the closure
    // below runs only after the batch is durable, so a check there could never
    // stop the write, only misreport it as refused.
    let write = match &admitted {
        Admitted::Profile(write, _) | Admitted::Set(write, _) => write.as_ref().clone(),
    };
    let plan = MutationPlan::for_method(&write);
    let owned = write.clone();
    Some(
        super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
            medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
            match admitted {
                Admitted::Profile(_, saved) => {
                    ResultPayload::of::<AssistantProfilePutProfile>(saved)
                }
                Admitted::Set(_, saved) => {
                    ResultPayload::of::<AssistantProfilePutDocumentSet>(saved)
                }
            }
        })
        .await,
    )
}

/// What one read answers.
#[derive(Debug, PartialEq)]
pub(super) enum ReadOut {
    Profile(AssistantProfile),
    Set(DocumentSet),
    Profiles(Vec<AssistantProfile>),
    Sets(Vec<DocumentSet>),
}

/// The read half of the store: gets and lists over the raw tenant graph,
/// filtered to what the caller may read.
pub(super) fn read_op(
    core: &GraphCore,
    isolation: &IsolationLayer,
    tenant_scope: &str,
    agent_id: &str,
    op: &ProfileOp,
) -> Result<ReadOut, String> {
    let caller = Caller::of(isolation, agent_id);
    let readable_profile = |profile: &AssistantProfile| {
        may(
            &caller,
            &profile.owner_principal,
            &profile.shares,
            &profile_resource(&profile.profile_id),
            RbacAction::Read,
        )
    };
    let readable_set = |set: &DocumentSet| {
        may(
            &caller,
            &set.owner_principal,
            &set.shares,
            &document_set_resource(&set.set_id),
            RbacAction::Read,
        )
    };
    match op {
        ProfileOp::GetProfile { profile_id } => read_stored::<AssistantProfile>(
            core,
            &profile_node_id(profile_id),
            "AssistantProfile",
            tenant_scope,
        )
        .filter(readable_profile)
        .map(ReadOut::Profile)
        .ok_or_else(|| profile_unavailable(profile_id)),
        ProfileOp::GetDocumentSet { set_id } => read_stored::<DocumentSet>(
            core,
            &document_set_node_id(set_id),
            "DocumentSet",
            tenant_scope,
        )
        .filter(readable_set)
        .map(ReadOut::Set)
        .ok_or_else(|| set_unavailable(set_id)),
        ProfileOp::ListProfiles => {
            let mut profiles: Vec<AssistantProfile> = core
                .node_ids()
                .iter()
                .filter(|id| id.starts_with(PROFILE_PREFIX))
                .filter_map(|id| read_stored(core, id, "AssistantProfile", tenant_scope))
                .filter(readable_profile)
                .collect();
            profiles.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
            Ok(ReadOut::Profiles(profiles))
        }
        ProfileOp::ListDocumentSets => {
            let mut sets: Vec<DocumentSet> = core
                .node_ids()
                .iter()
                .filter(|id| id.starts_with(SET_PREFIX))
                .filter_map(|id| read_stored(core, id, "DocumentSet", tenant_scope))
                .filter(readable_set)
                .collect();
            sets.sort_by(|a, b| a.set_id.cmp(&b.set_id));
            Ok(ReadOut::Sets(sets))
        }
        ProfileOp::PutProfile { .. } | ProfileOp::PutDocumentSet { .. } => Err(refuse(
            "INVALID_ARGUMENT",
            "an AssistantProfile put is served by the mutation gateway",
        )),
    }
}

/// The profile `profile_id` as `agent_id` may read it: the caller-facing read
/// other decision surfaces use (MEDULLA-DECISION-ENGINE-R036.107). A missing
/// profile, another tenant's and one the caller may not read all answer with
/// the one `PROFILE_NOT_VISIBLE` envelope.
pub fn read_visible_profile(
    core: &GraphCore,
    isolation: &IsolationLayer,
    tenant_scope: &str,
    agent_id: &str,
    profile_id: &str,
) -> Result<AssistantProfile, String> {
    let op = ProfileOp::GetProfile {
        profile_id: profile_id.to_string(),
    };
    match read_op(core, isolation, tenant_scope, agent_id, &op)? {
        ReadOut::Profile(profile) => Ok(profile),
        _ => Err(profile_unavailable(profile_id)),
    }
}

fn serve_read(
    req_id: u64,
    core: &GraphCore,
    isolation: &IsolationLayer,
    tenant_scope: &str,
    agent_id: &str,
    op: &ProfileOp,
) -> Response {
    match read_op(core, isolation, tenant_scope, agent_id, op) {
        Ok(ReadOut::Profile(p)) => {
            Response::ok(req_id, ResultPayload::of::<AssistantProfileGetProfile>(p))
        }
        Ok(ReadOut::Set(s)) => Response::ok(
            req_id,
            ResultPayload::of::<AssistantProfileGetDocumentSet>(s),
        ),
        Ok(ReadOut::Profiles(p)) => {
            Response::ok(req_id, ResultPayload::of::<AssistantProfileListProfiles>(p))
        }
        Ok(ReadOut::Sets(s)) => Response::ok(
            req_id,
            ResultPayload::of::<AssistantProfileListDocumentSets>(s),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Route the profile reads served from the graph.
pub(super) async fn try_handle_assistant_profile(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::AssistantProfile { op } = &method else {
        return ControlFlow::Continue(method);
    };
    let Some(tenant_scope) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            refuse(
                "ACCESS_DENIED",
                "an assistant profile read needs a verified tenant",
            ),
        ));
    };
    let agent_id = match ctx.read_authority.verified_actor() {
        Ok(actor) => actor.to_string(),
        Err(error) => return ControlFlow::Break(Response::err(ctx.req_id, error)),
    };
    let state = ctx.state.read().await;
    ControlFlow::Break(serve_read(
        ctx.req_id,
        ctx.raw_core,
        &state.isolation,
        &tenant_scope,
        &agent_id,
        op,
    ))
}

/// The IRIs the named document sets allow, as the caller may read them
/// (MEDULLA-DECISION-ENGINE-R656.3). An empty `set_ids` applies no filter
/// (`Ok(None)`). Each set loads under the caller's read authority, so an
/// unknown set, another tenant's and one the caller may not read all refuse
/// `DOCUMENT_SET_NOT_FOUND`. A `Members` set intersects with the IRIs of the
/// caller's own projection; a `SavedQuery` set first runs as a read-only query
/// under the caller's authority (the one leg that runs UQL) and its result IRIs
/// enter the same intersection. The per-set results are unioned.
pub(super) async fn resolve_document_sets(
    ctx: &GraphOpsContext<'_>,
    set_ids: &[String],
) -> Result<Option<std::collections::BTreeSet<String>>, String> {
    if set_ids.is_empty() {
        return Ok(None);
    }
    let Some(tenant_scope) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return Err(refuse(
            "ACCESS_DENIED",
            "a document-set read needs a verified tenant",
        ));
    };
    let agent_id = ctx.read_authority.verified_actor()?.to_string();
    let max_members =
        usize::try_from(knobs().profile_document_set_max_members).unwrap_or(usize::MAX);
    let mut sets = Vec::with_capacity(set_ids.len());
    {
        let state = ctx.state.read().await;
        for set_id in set_ids {
            let op = ProfileOp::GetDocumentSet {
                set_id: set_id.clone(),
            };
            match read_op(
                ctx.raw_core,
                &state.isolation,
                &tenant_scope,
                &agent_id,
                &op,
            ) {
                Ok(ReadOut::Set(set)) => sets.push(set),
                _ => return Err(set_unavailable(set_id)),
            }
        }
    }
    let visible: std::collections::BTreeSet<String> = ctx.core.node_ids().into_iter().collect();
    let mut allowed = std::collections::BTreeSet::new();
    for set in &sets {
        let in_view = match &set.kind {
            DocumentSetKind::Members(_) => visible.clone(),
            DocumentSetKind::SavedQuery(text) => {
                #[cfg(feature = "query")]
                {
                    let Some(seam) =
                        medulla_server_ops::api::query_txn::query_seam(ctx.state).await
                    else {
                        return Err(refuse(
                            "ENGINE_UNAVAILABLE",
                            "the query surface is not installed",
                        ));
                    };
                    let iris = seam
                        .saved_query_iris(
                            ctx.state,
                            ctx.graph_name,
                            ctx.read_authority,
                            &agent_id,
                            ctx.core,
                            text,
                            max_members,
                        )
                        .await?;
                    iris.into_iter()
                        .filter(|iri| visible.contains(iri))
                        .collect()
                }
                #[cfg(not(feature = "query"))]
                {
                    let _ = text;
                    return Err(refuse(
                        "ENGINE_UNAVAILABLE",
                        "a saved-query document set needs the `query` feature",
                    ));
                }
            }
        };
        allowed.extend(resolve_document_set(set, &in_view, max_members)?);
    }
    Ok(Some(allowed))
}
