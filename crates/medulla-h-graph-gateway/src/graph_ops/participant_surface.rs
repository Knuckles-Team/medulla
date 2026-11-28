//! Served participant writes (MEDULLA-DECISION-ENGINE-R617.9).
//!
//! `Method::Participant` is gateway-routed (`Route::Participant`): each operation is admitted
//! against the stored graph, writes nothing when refused, and otherwise commits as ONE node upsert
//! or ONE edge through the staged gateway, so it is durable and replicated like every other graph
//! write. One `match` over [`ParticipantOp`] picks the resolver; the stored shape is the one the
//! assistant profile store uses (a `json` property under the verified tenant scope).
//!
//! - `Put`: a participant is keyed by its IRI. An equal body is a no-op (nothing is committed). A
//!   different body for the same IRI updates the display name and `acts_as` only; the kind and
//!   `represents` never change (`PARTICIPANT_KIND_INVALID`). A principal that is not in the stored
//!   `acts_as` reads the participant as absent (`PARTICIPANT_NOT_VISIBLE`).
//! - `AddMember`: the session is found through its transcript thread
//!   (`MEMBERSHIP_SESSION_UNKNOWN` when no thread has that session id); the participant lives in
//!   the workspace the thread names (its connector). The edge is the one `hasParticipant` edge
//!   ingest writes, so the pair is idempotent.
//! - `PutScope`: only the owner changes a stored scope (`SCOPE_NOT_VISIBLE` otherwise); a scope
//!   names at most `participant.scope_max_sessions` sessions (`SCOPE_SESSION_LIMIT`).
//! - `SetObservation`: only a principal in the participant's `acts_as`, or the owner of a scope
//!   that names the session, may set it (`OBSERVER_NOT_PERMITTED`).
//!
//! Ingest writes its speakers' memberships through [`commit_memberships`], the same edge shape and
//! the same gateway commit (MEDULLA-DECISION-ENGINE-R617.8).

use std::collections::BTreeSet;

use super::assistant_profile::node_blob;
use super::*;
use medulla_decision_types::claims::ObserverRef;
use medulla_decision_types::decision::knob_families::participant::KNOBS as PARTICIPANT_KNOBS;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_isolation::profile_access::read_stored;
use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::result_contract::coordination::{
    ParticipantAddMember, ParticipantCompleteDerivation, ParticipantPut, ParticipantPutScope,
    ParticipantSetObservation,
};
use medulla_types::contract::ResourceId;
use medulla_work_types::memory::ThreadAnchor;
use medulla_work_types::memory_scope::{
    observation_node_id, resolve_scope_sessions, scope_node_id, scope_node_prefix, MemoryScope,
    ObservationConfig, ScopeRef,
};
use medulla_work_types::participant::{
    membership_edges, participant_iri, Membership, Participant, ParticipantId, ParticipantOp,
    ADDED_AT_PROPERTY, HAS_PARTICIPANT_EDGE, PARTICIPANT_NODE_TYPE,
};

const SCOPE_LABEL: &str = "MemoryScope";
const OBSERVATION_LABEL: &str = "ObservationConfig";
const SCOPE_LIMIT_KNOB: &str = "participant.scope_max_sessions";
/// The node id prefix of a committed transcript thread.
pub const THREAD_NODE_PREFIX: &str = "transcript-thread:";
/// The `type` property of a committed transcript thread node.
const THREAD_NODE_TYPE: &str = "Transcript";

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The one non-disclosing envelope of an absent or unreadable participant.
pub fn participant_unavailable(id: &str) -> String {
    refuse(
        "PARTICIPANT_NOT_VISIBLE",
        format!("participant {id} is not available to the caller"),
    )
}

/// The default of the decision knob `participant.scope_max_sessions`.
pub fn scope_max_sessions() -> usize {
    PARTICIPANT_KNOBS
        .iter()
        .find(|spec| spec.name == SCOPE_LIMIT_KNOB)
        .and_then(|spec| match spec.default {
            KnobValue::Integer(value) => usize::try_from(value).ok(),
            _ => None,
        })
        .unwrap_or(usize::MAX)
}

/// What an admitted operation answers, and the one write that commits it (`None` when the
/// operation changes nothing).
pub struct Admitted {
    pub write: Option<Method>,
    pub out: Out,
}

/// The typed answer of one operation.
#[derive(Debug)]
pub enum Out {
    Participant(Participant),
    Member(Membership),
    Scope(MemoryScope),
    Observation(ObservationConfig),
    Derivation(medulla_decision_types::memory_events::DerivationCompleted),
}

fn invalid(detail: impl std::fmt::Display) -> String {
    refuse("INVALID_ARGUMENT", detail)
}

fn upsert(
    node_id: String,
    label: &str,
    scope: &str,
    object: &impl serde::Serialize,
) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id,
        properties_msgpack: node_blob(label, scope, object)?,
    })
}

/// The decoded properties of one node, when it is stored.
fn node_properties(
    core: &GraphCore,
    node_id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let blob = core.get_node_properties(node_id)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

/// The transcript thread nodes of `session_id`, in node-id order, each with the workspace its
/// connector names.
fn threads_of(core: &GraphCore, session_id: &str) -> Vec<(String, ResourceId)> {
    let mut ids: Vec<String> = core
        .node_ids()
        .into_iter()
        .filter(|id| id.starts_with(THREAD_NODE_PREFIX))
        .collect();
    ids.sort();
    ids.into_iter()
        .filter_map(|id| node_properties(core, &id).map(|props| (id, props)))
        .filter(|(_, props)| {
            props.get("type").and_then(serde_json::Value::as_str) == Some(THREAD_NODE_TYPE)
                && props.get("session_id").and_then(serde_json::Value::as_str) == Some(session_id)
        })
        .filter_map(|(id, props)| {
            let workspace = props.get("connector").and_then(serde_json::Value::as_str)?;
            Some((id, ResourceId::new(workspace).ok()?))
        })
        .collect()
}

/// The thread of `session_id` that holds `participant_id` as a stored participant of its workspace
/// the caller acts as: the thread node id, the participant's IRI and the stored participant. A
/// session with no thread is `MEMBERSHIP_SESSION_UNKNOWN`; a participant the caller cannot read is
/// `PARTICIPANT_NOT_VISIBLE`.
fn participant_in_session(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    session_id: &str,
    participant_id: &ParticipantId,
) -> Result<(String, String, Participant), String> {
    let threads = threads_of(core, session_id);
    if threads.is_empty() {
        return Err(refuse(
            "MEMBERSHIP_SESSION_UNKNOWN",
            format!("no thread has session id {session_id}"),
        ));
    }
    for (thread_id, workspace) in threads {
        let iri = participant_iri(&workspace, participant_id);
        let stored: Option<Participant> =
            read_stored(core, &iri, PARTICIPANT_NODE_TYPE, tenant_scope);
        if let Some(stored) = stored.filter(|p| p.acts_as.iter().any(|a| a == agent_id)) {
            return Ok((thread_id, iri, stored));
        }
    }
    Err(participant_unavailable(participant_id.as_str()))
}

/// Admit one operation against the stored graph, writing nothing.
pub fn admit(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    op: &ParticipantOp,
) -> Result<Admitted, String> {
    match op {
        ParticipantOp::Put { participant } => admit_put(core, tenant_scope, agent_id, participant),
        ParticipantOp::AddMember {
            session_id,
            participant_id,
        } => admit_member(core, tenant_scope, agent_id, session_id, participant_id),
        ParticipantOp::PutScope { scope } => admit_scope(core, tenant_scope, agent_id, scope),
        ParticipantOp::SetObservation { config } => {
            admit_observation(core, tenant_scope, agent_id, config)
        }
        ParticipantOp::CompleteDerivation {
            payload_ref,
            claims_written,
        } => {
            let at_ms = authoritative_now_ms();
            let (write, completed) = super::participant_derive::advance_checkpoint(
                core,
                tenant_scope,
                payload_ref,
                *claims_written,
                at_ms,
            )?;
            Ok(Admitted {
                write,
                out: Out::Derivation(completed),
            })
        }
    }
}

fn admit_put(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    participant: &Participant,
) -> Result<Admitted, String> {
    participant.validate().map_err(invalid)?;
    let tenant = medulla_types::contract::TenantId::new(tenant_scope).map_err(invalid)?;
    let iri = participant.iri();
    let stored: Option<Participant> = read_stored(core, &iri, PARTICIPANT_NODE_TYPE, tenant_scope);
    // A node another tenant stored under this IRI is never overwritten.
    if stored.is_none() && core.get_node_properties(&iri).is_some() {
        return Err(participant_unavailable(participant.participant_id.as_str()));
    }
    let mut saved = participant.clone();
    saved.tenant = tenant;
    if let Some(stored) = stored {
        if !stored.acts_as.iter().any(|a| a == agent_id) {
            return Err(participant_unavailable(participant.participant_id.as_str()));
        }
        if saved.kind != stored.kind || saved.represents != stored.represents {
            return Err(refuse(
                "PARTICIPANT_KIND_INVALID",
                "the kind and represents of a stored participant never change",
            ));
        }
        // An update changes the name and acts_as only.
        let mut updated = stored.clone();
        updated.display_name = saved.display_name;
        updated.acts_as = saved.acts_as;
        if updated == stored {
            return Ok(Admitted {
                write: None,
                out: Out::Participant(stored),
            });
        }
        let write = upsert(iri, PARTICIPANT_NODE_TYPE, tenant_scope, &updated)?;
        return Ok(Admitted {
            write: Some(write),
            out: Out::Participant(updated),
        });
    }
    let write = upsert(iri, PARTICIPANT_NODE_TYPE, tenant_scope, &saved)?;
    Ok(Admitted {
        write: Some(write),
        out: Out::Participant(saved),
    })
}

/// The `hasParticipant` edge from a thread to a participant.
fn membership_edge(thread_id: String, iri: String, added_at_ms: i64) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: thread_id,
        target_id: iri,
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
            "relationship": HAS_PARTICIPANT_EDGE,
            ADDED_AT_PROPERTY: added_at_ms,
        }))
        .map_err(|error| invalid(format!("membership encoding failed: {error}")))?,
    })
}

fn admit_member(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    session_id: &str,
    participant_id: &ParticipantId,
) -> Result<Admitted, String> {
    let (thread_id, iri, _) =
        participant_in_session(core, tenant_scope, agent_id, session_id, participant_id)?;
    let added_at_ms = i64::try_from(authoritative_now_ms()).map_err(invalid)?;
    let membership = Membership {
        session_id: session_id.to_string(),
        participant_id: participant_id.clone(),
        added_at_ms,
    };
    // The pair is idempotent: an existing edge is the answer and commits nothing.
    if core.has_edge(&thread_id, &iri) {
        return Ok(Admitted {
            write: None,
            out: Out::Member(membership),
        });
    }
    let write = membership_edge(thread_id, iri, added_at_ms)?;
    Ok(Admitted {
        write: Some(write),
        out: Out::Member(membership),
    })
}

fn admit_scope(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    scope: &MemoryScope,
) -> Result<Admitted, String> {
    if scope.scope_id.trim().is_empty() || scope.name.trim().is_empty() {
        return Err(invalid("scope_id and name are required"));
    }
    if scope.session_ids.len() > scope_max_sessions() {
        return Err(refuse(
            "SCOPE_SESSION_LIMIT",
            format!(
                "scope {} names {} sessions, above {SCOPE_LIMIT_KNOB}",
                scope.scope_id,
                scope.session_ids.len()
            ),
        ));
    }
    let node_id = scope_node_id(tenant_scope, &scope.scope_id);
    let stored: Option<MemoryScope> = read_stored(core, &node_id, SCOPE_LABEL, tenant_scope);
    match &stored {
        None if scope.owner_principal != agent_id => {
            return Err(refuse(
                "SCOPE_NOT_VISIBLE",
                "a new scope is owned by the caller that saves it",
            ));
        }
        Some(stored) if stored.owner_principal != agent_id => {
            return Err(refuse(
                "SCOPE_NOT_VISIBLE",
                format!("scope {} is not available to the caller", scope.scope_id),
            ));
        }
        _ => {}
    }
    let mut saved = scope.clone();
    saved.tenant = tenant_scope.to_string();
    saved.owner_principal = agent_id.to_string();
    if stored.as_ref() == Some(&saved) {
        return Ok(Admitted {
            write: None,
            out: Out::Scope(saved),
        });
    }
    let write = upsert(node_id, SCOPE_LABEL, tenant_scope, &saved)?;
    Ok(Admitted {
        write: Some(write),
        out: Out::Scope(saved),
    })
}

/// The sessions a read narrowed by `reference` may touch (MEDULLA-DECISION-ENGINE-R651.9): the
/// stored scope's sessions intersected with `visible`, the sessions the caller's view already
/// allows, so a scope never widens a read. A scope the caller's tenant never stored, including
/// another tenant's, is `SCOPE_NOT_FOUND`; a scope that exists but is neither the caller's nor
/// shared with it is `SCOPE_NOT_VISIBLE`; a scope above `participant.scope_max_sessions` is
/// `SCOPE_SESSION_LIMIT`. Every memory read that receives a `ScopeRef` calls this one function.
pub fn resolve_scope_ref(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    reference: &ScopeRef,
    visible: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let node_id = scope_node_id(tenant_scope, &reference.scope_id);
    let Some(scope) = read_stored::<MemoryScope>(core, &node_id, SCOPE_LABEL, tenant_scope) else {
        return Err(refuse(
            "SCOPE_NOT_FOUND",
            format!("scope {} does not exist", reference.scope_id),
        ));
    };
    if scope.owner_principal != agent_id && !scope.shared_with.contains(agent_id) {
        return Err(refuse(
            "SCOPE_NOT_VISIBLE",
            format!(
                "scope {} is not available to the caller",
                reference.scope_id
            ),
        ));
    }
    if scope.session_ids.len() > scope_max_sessions() {
        return Err(refuse(
            "SCOPE_SESSION_LIMIT",
            format!(
                "scope {} names {} sessions, above {SCOPE_LIMIT_KNOB}",
                reference.scope_id,
                scope.session_ids.len()
            ),
        ));
    }
    Ok(resolve_scope_sessions(&scope, visible))
}

/// The stored observation setting of one participant in one session, when one is stored.
pub fn observation_stored(
    core: &GraphCore,
    tenant_scope: &str,
    session_id: &str,
    participant_id: &ParticipantId,
) -> Option<ObservationConfig> {
    let node_id = observation_node_id(tenant_scope, session_id, participant_id);
    read_stored(core, &node_id, OBSERVATION_LABEL, tenant_scope)
}

fn admit_observation(
    core: &GraphCore,
    tenant_scope: &str,
    agent_id: &str,
    config: &ObservationConfig,
) -> Result<Admitted, String> {
    let threads = threads_of(core, &config.session_id);
    if threads.is_empty() {
        return Err(refuse(
            "MEMBERSHIP_SESSION_UNKNOWN",
            format!("no thread has session id {}", config.session_id),
        ));
    }
    // The participant must exist in the session's workspace; the caller acts as it, or owns a
    // scope that names the session.
    let participant = threads.iter().find_map(|(_, workspace)| {
        read_stored::<Participant>(
            core,
            &participant_iri(workspace, &config.participant_id),
            PARTICIPANT_NODE_TYPE,
            tenant_scope,
        )
    });
    let Some(participant) = participant else {
        return Err(participant_unavailable(config.participant_id.as_str()));
    };
    let acts_as = participant.acts_as.iter().any(|a| a == agent_id);
    let owns_scope = core
        .node_ids()
        .into_iter()
        .filter(|id| id.starts_with(&scope_node_prefix(tenant_scope)))
        .filter_map(|id| read_stored::<MemoryScope>(core, &id, SCOPE_LABEL, tenant_scope))
        .any(|scope| {
            scope.owner_principal == agent_id && scope.session_ids.contains(&config.session_id)
        });
    if !acts_as && !owns_scope {
        return Err(refuse(
            "OBSERVER_NOT_PERMITTED",
            format!(
                "the caller may not act as participant {} in session {}",
                config.participant_id.as_str(),
                config.session_id
            ),
        ));
    }
    let node_id = observation_node_id(tenant_scope, &config.session_id, &config.participant_id);
    let stored: Option<ObservationConfig> =
        read_stored(core, &node_id, OBSERVATION_LABEL, tenant_scope);
    if stored.as_ref() == Some(config) {
        return Ok(Admitted {
            write: None,
            out: Out::Observation(config.clone()),
        });
    }
    let write = upsert(node_id, OBSERVATION_LABEL, tenant_scope, config)?;
    Ok(Admitted {
        write: Some(write),
        out: Out::Observation(config.clone()),
    })
}

fn payload(out: Out) -> Result<ResultPayload, String> {
    match out {
        Out::Participant(value) => ResultPayload::of::<ParticipantPut>(value),
        Out::Member(value) => ResultPayload::of::<ParticipantAddMember>(value),
        Out::Scope(value) => ResultPayload::of::<ParticipantPutScope>(value),
        Out::Observation(value) => ResultPayload::of::<ParticipantSetObservation>(value),
        Out::Derivation(value) => ResultPayload::of::<ParticipantCompleteDerivation>(value),
    }
}

/// Most participant nodes one read scans.
const PARTICIPANT_SCAN_CAP: usize = 100_000;

/// Every participant `tenant_scope` stores, in node-id order. A node of another tenant never
/// decodes under this scope, so it is never listed.
fn stored_participants(core: &GraphCore, tenant_scope: &str) -> Vec<(String, Participant)> {
    let mut nodes: Vec<String> = core
        .get_nodes_by_label(PARTICIPANT_NODE_TYPE, PARTICIPANT_SCAN_CAP)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    nodes.sort();
    nodes
        .into_iter()
        .filter_map(|id| {
            read_stored::<Participant>(core, &id, PARTICIPANT_NODE_TYPE, tenant_scope)
                .map(|participant| (id, participant))
        })
        .collect()
}

/// Every participant of `tenant_scope` the principal acts as, in node-id order: the ones it may
/// read (MEDULLA-DECISION-ENGINE-R655.4). A participant it does not act as reads as absent.
pub fn readable_participants(
    core: &GraphCore,
    tenant_scope: &str,
    principal: &str,
) -> Vec<Participant> {
    stored_participants(core, tenant_scope)
        .into_iter()
        .map(|(_, participant)| participant)
        .filter(|participant| participant.acts_as.iter().any(|name| name == principal))
        .collect()
}

/// Whether `tenant_scope` stores a participant with this slug in any workspace.
pub fn participant_exists(core: &GraphCore, tenant_scope: &str, slug: &ParticipantId) -> bool {
    stored_participants(core, tenant_scope)
        .iter()
        .any(|(_, participant)| &participant.participant_id == slug)
}

/// The view a principal reads in: the observer it acts as and the sessions that observer belongs
/// to (MEDULLA-DECISION-ENGINE-R652.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalView {
    pub observer: ObserverRef,
    pub sessions: BTreeSet<String>,
}

/// The sessions of every thread that holds a stored participant of `tenant_scope` with this slug,
/// in any workspace (MEDULLA-DECISION-ENGINE-R655.3).
pub fn sessions_of_participant(
    core: &GraphCore,
    tenant_scope: &str,
    slug: &ParticipantId,
) -> BTreeSet<String> {
    stored_participants(core, tenant_scope)
        .into_iter()
        .filter(|(_, participant)| &participant.participant_id == slug)
        .flat_map(|(iri, _)| sessions_of(core, &iri))
        .collect()
}

/// The sessions of every thread that holds a participant the principal acts as: the sessions it
/// may read about (MEDULLA-DECISION-ENGINE-R655.4).
pub fn sessions_visible_to(
    core: &GraphCore,
    tenant_scope: &str,
    principal: &str,
) -> BTreeSet<String> {
    stored_participants(core, tenant_scope)
        .into_iter()
        .filter(|(_, participant)| participant.acts_as.iter().any(|name| name == principal))
        .flat_map(|(iri, _)| sessions_of(core, &iri))
        .collect()
}

/// The sessions of every thread that holds the participant node `iri`.
fn sessions_of(core: &GraphCore, iri: &str) -> BTreeSet<String> {
    core.get_predecessors(iri)
        .unwrap_or_default()
        .into_iter()
        .filter(|id| id.starts_with(THREAD_NODE_PREFIX))
        .filter_map(|id| node_properties(core, &id))
        .filter(|props| {
            props.get("type").and_then(serde_json::Value::as_str) == Some(THREAD_NODE_TYPE)
        })
        .filter_map(|props| {
            props
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The view `principal` reads in over the graph of `tenant_scope`. The participant is the stored
/// one whose `acts_as` lists the principal; its sessions are the threads that hold it. Nothing a
/// request carries widens the view: `requested` only chooses among the participants the principal
/// acts as, and any other name, or an unnamed read by a principal acting as several, is
/// `OBSERVER_NOT_PERMITTED`. Naming the system observer, or acting as no participant, reads as the
/// shared system observer over no session. Participants of another tenant never bind.
pub fn principal_view(
    core: &GraphCore,
    tenant_scope: &str,
    principal: &str,
    requested: Option<&ObserverRef>,
) -> Result<PrincipalView, String> {
    let not_permitted = |why: &str| refuse("OBSERVER_NOT_PERMITTED", why);
    let acting: Vec<(String, Participant)> = stored_participants(core, tenant_scope)
        .into_iter()
        .filter(|(_, participant)| participant.acts_as.iter().any(|name| name == principal))
        .collect();
    let chosen = match requested {
        Some(ObserverRef::System) => None,
        Some(ObserverRef::Participant(wanted)) => Some(
            acting
                .iter()
                .find(|(_, participant)| &participant.participant_id == wanted)
                .ok_or_else(|| {
                    not_permitted(&format!(
                        "the principal does not act as {}",
                        wanted.as_str()
                    ))
                })?,
        ),
        None => match acting.as_slice() {
            [] => None,
            [only] => Some(only),
            _ => {
                return Err(not_permitted(
                    "the principal acts as several participants; name one in as_observer",
                ))
            }
        },
    };
    Ok(match chosen {
        None => PrincipalView {
            observer: ObserverRef::System,
            sessions: BTreeSet::new(),
        },
        Some((iri, participant)) => PrincipalView {
            observer: ObserverRef::Participant(participant.participant_id.clone()),
            sessions: sessions_of(core, iri),
        },
    })
}

/// The `Route::Participant` entry: admit the operation, then commit its one write through the
/// coalescable gateway.
pub async fn handle(ctx: &MutationCtx<'_>, _tenant_id: &str, method: &Method) -> Option<Response> {
    let Method::Participant { op } = method else {
        return None;
    };
    let Some(agent_id) = ctx.caller.filter(|caller| !caller.trim().is_empty()) else {
        return Some(Response::err(
            ctx.req_id,
            refuse(
                "ACCESS_DENIED",
                "a participant write needs a verified caller",
            ),
        ));
    };
    let Admitted { write, out } = match admit(ctx.core, ctx.tenant_scope, agent_id, op) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let Some(write) = write else {
        return Some(match payload(out) {
            Ok(payload) => Response::ok(ctx.req_id, payload),
            Err(error) => Response::err(ctx.req_id, error),
        });
    };
    let plan = MutationPlan::for_method(&write);
    let owned = write.clone();
    // A derivation checkpoint is one batch (the checkpoint node and its event node), which the
    // gateway applies like the memberships of an ingested thread.
    if let Method::BatchUpdate { operations_msgpack } = write.clone() {
        return Some(
            medulla_server_ops::api::ingest::graph::commit_gateway(
                ctx,
                &plan,
                &write,
                move |core| {
                    medulla_graph_algos::algorithms::batch_update(core, &operations_msgpack)?;
                    payload(out)
                },
            )
            .await,
        );
    }
    Some(
        super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
            medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
            payload(out)
        })
        .await,
    )
}

/// The memberships one ingested thread's speakers hold, as the one batch that writes them
/// (MEDULLA-DECISION-ENGINE-R617.8). Every speaker must be a stored participant of the thread's
/// workspace under the caller's tenant; one that is not is `PARTICIPANT_NOT_VISIBLE`, the envelope
/// of an absent participant, and nothing is written. A speaker already on the thread is not
/// written again, so a replay of the thread writes nothing.
pub fn plan_memberships(
    core: &GraphCore,
    tenant_scope: &str,
    anchor: &ThreadAnchor,
    conversation: &medulla_work_types::memory::ConversationRecord,
) -> Result<Option<(Method, Membership)>, String> {
    let mut operations = Vec::new();
    let mut first = None;
    for membership in membership_edges(conversation) {
        let iri = participant_iri(&anchor.workspace, &membership.participant_id);
        if read_stored::<Participant>(core, &iri, PARTICIPANT_NODE_TYPE, tenant_scope).is_none() {
            return Err(participant_unavailable(membership.participant_id.as_str()));
        }
        if core.has_edge(&anchor.thread_node_id, &iri) {
            continue;
        }
        operations.push(serde_json::json!({
            "op": "upsert_edge",
            "source": anchor.thread_node_id,
            "target": iri,
            "properties": {
                "relationship": HAS_PARTICIPANT_EDGE,
                ADDED_AT_PROPERTY: membership.added_at_ms,
            },
        }));
        first.get_or_insert(membership);
    }
    let Some(first) = first else {
        return Ok(None);
    };
    let operations_msgpack = rmp_serde::to_vec_named(&serde_json::Value::Array(operations))
        .map_err(|error| invalid(format!("membership encoding failed: {error}")))?;
    Ok(Some((Method::BatchUpdate { operations_msgpack }, first)))
}

/// Commit the memberships of one ingested thread (MEDULLA-DECISION-ENGINE-R617.8): one batch, the
/// gateway's normal commit. Nothing is committed when every speaker is already a member.
pub async fn commit_memberships(
    ctx: &MutationCtx<'_>,
    anchor: &ThreadAnchor,
    conversation: &medulla_work_types::memory::ConversationRecord,
) -> Result<usize, String> {
    let Some((write, first)) = plan_memberships(ctx.core, ctx.tenant_scope, anchor, conversation)?
    else {
        return Ok(0);
    };
    let Method::BatchUpdate { operations_msgpack } = write.clone() else {
        return Err(invalid("memberships are one batch"));
    };
    let written = membership_edges(conversation).len();
    let plan = MutationPlan::for_method(&write);
    let response =
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
            medulla_graph_algos::algorithms::batch_update(core, &operations_msgpack)?;
            ResultPayload::of::<ParticipantAddMember>(first)
        })
        .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(written),
    }
}
