use super::*;

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::result_contract::ingestion::{
    GoalPut as GoalPutResult, ProjectModeSet as ProjectModeSetResult,
    ProposalDecide as ProposalDecideResult, ProposalList as ProposalListResult,
    ProposalPut as ProposalPutResult, RecordDeployment as RecordDeploymentResult,
    RecordRelease as RecordReleaseResult,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::{
    authorized_graph_read, ensure_graph_resident, timed_read, typed_response,
};
use medulla_source_types::source_changeset::{commit_node_id, COMMIT_TYPE};
use medulla_source_types::lifecycle::{
    deployment_node_id, handle_goal_put, handle_record_deployment, handle_record_release,
    handle_project_mode_set, handle_proposal_decide, handle_proposal_put, project_mode_node_id,
    LifecycleRefusal, Proposal, ProposalDecideReceipt, ProposalDecideRequest, ProposalEvidence,
    ProposalKind, ProposalListEntry, ProposalListRequest, ProposalReceipt, ProposalState,
    ProposalWrite, MAX_PROPOSAL_EVIDENCE, release_node_id, Deployment, DeploymentInput,
    DeploymentReceipt, Goal, GoalReceipt, GoalWrite, ProjectMode, ProjectModeReceipt,
    ProjectModeSet, ProjectModeWrite, Release, ReleaseReceipt,
};
use serde_json::{json, Map, Value};
use tokio::sync::RwLock;

/// The node type of a recorded release (a sealed record class).
const RELEASE_TYPE: &str = "Release";
/// The node type of a recorded deployment.
const DEPLOYMENT_TYPE: &str = "Deployment";
/// The node type of a goal record.
const GOAL_TYPE: &str = "Goal";
/// The node type of a project mode record.
const PROJECT_MODE_TYPE: &str = "ProjectMode";
/// The node type of a proposal record.
const PROPOSAL_TYPE: &str = "Proposal";
/// The property holding the canonical JSON of the release record.
const RELEASE_RECORD_PROPERTY: &str = "release_record";
const MAX_RELEASES_READ: usize = 100_000;
const MAX_PROPOSALS_READ: usize = 100_000;
const MAX_CHAIN: usize = 200_000;
const MAX_BATCH_OPERATIONS: usize = 50_000;

/// Served release and deployment records (MEDULLA-SDLC-R002.3 to R002.6): the
/// two methods self-route here. Each decides in
/// `medulla_source_types::lifecycle` and self-translates its records into one
/// gateway-routed `BatchUpdate` against the request graph, so the write is
/// ACL-checked, sealed, durable, audited and CDC-emitted. A refusal returns
/// before the batch is built, so nothing is written.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_sdlc_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let target = SdlcTarget {
        graph: &ctx.req.graph,
        req_id: ctx.req.id,
        caller: ctx.req.agent_id.as_deref(),
        verified: ctx.verified_context,
    };
    let state = ctx.state;
    let response = match method {
        Method::RecordRelease { release } => {
            dispatch_boxed(record_release(state, target, release)).await
        }
        Method::RecordDeployment { deployment } => {
            dispatch_boxed(record_deployment(state, target, deployment)).await
        }
        Method::GoalPut { goal } => dispatch_boxed(goal_put(state, target, goal)).await,
        Method::ProposalPut { proposal } => {
            dispatch_boxed(proposal_put(state, target, proposal)).await
        }
        Method::ProposalList { request } => {
            dispatch_boxed(proposal_list(state, target, request)).await
        }
        Method::ProposalDecide { request } => {
            dispatch_boxed(proposal_decide(state, target, request)).await
        }
        Method::ProjectModeSet { request } => {
            dispatch_boxed(project_mode_set(state, target, request)).await
        }
        other => return ControlFlow::Continue(other),
    };
    ControlFlow::Break(response)
}

struct SdlcTarget<'a> {
    graph: &'a str,
    req_id: u64,
    caller: Option<&'a str>,
    verified: &'a VerifiedRequestContext,
}

async fn record_release(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    release: Release,
) -> Response {
    let result = async {
        let (core, existing) = load_releases(state, &target).await?;
        let chain = commit_chain(&core, target.verified.tenant(), &release.changeset_head);
        let write = handle_record_release(&existing, &chain, release).map_err(|e| e.to_string())?;
        let operations = release_operations(&write, &target)?;
        commit(state, &target, operations).await?;
        Ok(ReleaseReceipt {
            release_id: write.release.id(),
            node_id: release_node_id(&write.release),
            seal: write.seal,
            contains: write.contains.len() as u64,
            replayed: write.replayed,
        })
    }
    .await;
    typed_response::<RecordReleaseResult>(target.req_id, result)
}

async fn record_deployment(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    input: DeploymentInput,
) -> Response {
    let result = async {
        let (_, releases) = load_releases(state, &target).await?;
        let deployment = handle_record_deployment(&releases, input);
        let operations = deployment_operations(&deployment, &target)?;
        commit(state, &target, operations).await?;
        Ok(DeploymentReceipt {
            deployment_id: deployment_node_id(&deployment),
            release_ref: deployment.release_ref,
            outcome: deployment.outcome,
        })
    }
    .await;
    typed_response::<RecordDeploymentResult>(target.req_id, result)
}

/// `GoalPut` (MEDULLA-SDLC-R017.1): a goal with no target refuses before any
/// write; otherwise one `BatchUpdate` writes the goal node and a `targetsGoal`
/// edge from each target to it.
async fn goal_put(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    goal: Goal,
) -> Response {
    let result = async {
        if target.verified.tenant().trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        if goal.project.trim().is_empty() || goal.statement.trim().is_empty() {
            return Err("INVALID_ARGUMENT: a goal names its project and a statement".to_string());
        }
        let write = handle_goal_put(goal).map_err(|e| e.to_string())?;
        let operations = goal_operations(&write, &target)?;
        commit(state, &target, operations).await?;
        Ok(GoalReceipt {
            node_id: write.node_id,
            targets: write.targets.len() as u64,
        })
    }
    .await;
    typed_response::<GoalPutResult>(target.req_id, result)
}

/// `ProposalPut` (MEDULLA-SDLC-R017.3): resolve each cited record through the caller's
/// visible store, decide, then write the `Proposal` node and one `evidences` edge from each
/// cited record in one `BatchUpdate`. A refusal returns before anything is written.
async fn proposal_put(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    proposal: Proposal,
) -> Response {
    let result = async {
        if target.verified.tenant().trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        // MEDULLA-SDLC-R017.5: the project mode decides whether the proposal is taken, before
        // its evidence is resolved. A blank project falls through to the handler's refusal.
        if !proposal.project.trim().is_empty() {
            let mode = stored_project_mode(state, &target, &proposal.project).await?;
            medulla_sdlc::lifecycle::proposals::admit_proposal(
                mode,
                proposal.origin,
                &proposal.project,
            )
            .map_err(|e| e.to_string())?;
        }
        let resolved = resolve_evidence(state, &target, &proposal).await?;
        let write = handle_proposal_put(proposal, &resolved).map_err(|e| e.to_string())?;
        let operations = proposal_operations(&write, &target)?;
        commit(state, &target, operations).await?;
        Ok(ProposalReceipt {
            node_id: write.node_id,
            evidence: write.proposal.evidence.len() as u64,
        })
    }
    .await;
    typed_response::<ProposalPutResult>(target.req_id, result)
}

/// `(record_id, content digest)` of each cited record the caller can see.
async fn resolve_evidence(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    proposal: &Proposal,
) -> Result<Vec<(String, String)>, String> {
    if proposal.evidence.is_empty() {
        return Ok(Vec::new());
    }
    ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    Ok(proposal
        .evidence
        .iter()
        .take(MAX_PROPOSAL_EVIDENCE + 1)
        .filter_map(|cited| {
            let blob = core.get_node_properties(&cited.record_id)?;
            authority
                .can_see_node(&blob, core.is_schema_node(&cited.record_id))
                .then(|| {
                    let (digest, _) = medulla_h_ingest::ingest_readback::observe(&blob);
                    (cited.record_id.clone(), digest.to_hex())
                })
        })
        .collect())
}

/// The `Proposal` node, then one `evidences` edge from each distinct cited record to it.
fn proposal_operations(
    write: &ProposalWrite,
    target: &SdlcTarget<'_>,
) -> Result<Vec<Value>, String> {
    let proposal = &write.proposal;
    let kind = serde_json::to_value(proposal.kind).map_err(|e| e.to_string())?;
    let evidence = serde_json::to_string(&proposal.evidence).map_err(|e| e.to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), PROPOSAL_TYPE.into());
    properties.insert("project".into(), proposal.project.clone().into());
    properties.insert("kind".into(), kind);
    properties.insert("target".into(), proposal.target.clone().into());
    properties.insert("statement".into(), proposal.statement.clone().into());
    properties.insert("origin".into(), proposal.origin.name().into());
    properties.insert("status".into(), ProposalState::Open.stored().into());
    properties.insert("evidence".into(), evidence.into());
    if let Some(marker) = &proposal.marker {
        properties.insert("marker".into(), marker.clone().into());
    }
    stamp(&mut properties, target);
    let mut operations =
        vec![json!({"op": "upsert_node", "id": write.node_id, "properties": properties})];
    let mut cited = BTreeSet::new();
    for entry in &proposal.evidence {
        if cited.insert(entry.record_id.as_str()) {
            operations.push(edge(&entry.record_id, "evidences", &write.node_id, target));
        }
    }
    bounded(operations)
}

/// A stored `Proposal` node of the verified tenant, decoded for review.
struct StoredProposal {
    project: String,
    entry: ProposalListEntry,
}

/// Decode one stored proposal node; `None` for a node of another tenant, another type or a
/// shape this method did not write.
fn stored_proposal(node_id: &str, properties: &[u8], tenant: &str) -> Option<StoredProposal> {
    let node = medulla_types::msgpack::decode_property_value(properties).ok()?;
    if node.get("type").and_then(Value::as_str) != Some(PROPOSAL_TYPE)
        || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
    {
        return None;
    }
    let text = |key: &str| node.get(key).and_then(Value::as_str);
    let kind: ProposalKind =
        serde_json::from_value(Value::String(text("kind")?.to_owned())).ok()?;
    let evidence: Vec<ProposalEvidence> = serde_json::from_str(text("evidence")?).ok()?;
    let state = ProposalState::from_stored(text("status")?)?;
    Some(StoredProposal {
        project: text("project")?.to_owned(),
        entry: ProposalListEntry {
            id: node_id.to_owned(),
            kind,
            target: text("target")?.to_owned(),
            evidence,
            state,
        },
    })
}

/// The verified tenant's proposals the caller can see, by node id.
async fn visible_proposals(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
) -> Result<Vec<StoredProposal>, String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err("TENANT_SCOPE_REQUIRED".to_string());
    }
    ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    let mut proposals: Vec<StoredProposal> = core
        .get_nodes_by_label(PROPOSAL_TYPE, MAX_PROPOSALS_READ)
        .into_iter()
        .filter(|(node_id, properties)| {
            authority.can_see_node(properties, core.is_schema_node(node_id))
        })
        .filter_map(|(node_id, properties)| stored_proposal(&node_id, &properties, tenant))
        .collect();
    proposals.sort_by(|a, b| a.entry.id.cmp(&b.entry.id));
    Ok(proposals)
}

/// `ProposalList` (MEDULLA-SDLC-R017.6): the proposals of one project, narrowed by the
/// optional state and kind. Reads only.
async fn proposal_list(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: ProposalListRequest,
) -> Response {
    let result = async {
        let proposals = visible_proposals(state, &target).await?;
        Ok(proposals
            .into_iter()
            .filter(|stored| stored.project == request.repo)
            .filter(|stored| request.state.is_none_or(|wanted| stored.entry.state == wanted))
            .filter(|stored| request.kind.is_none_or(|wanted| stored.entry.kind == wanted))
            .map(|stored| stored.entry)
            .collect::<Vec<_>>())
    }
    .await;
    typed_response::<ProposalListResult>(target.req_id, result)
}

/// `ProposalDecide` (MEDULLA-SDLC-R017.6): read the stored proposal, decide the new state,
/// then write the state and the reviewer in one `BatchUpdate`. The node merges these keys; its
/// owner and its content stay as `ProposalPut` wrote them. A refusal returns before any write.
async fn proposal_decide(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: ProposalDecideRequest,
) -> Response {
    let result = async {
        let proposals = visible_proposals(state, &target).await?;
        let stored = proposals
            .iter()
            .find(|stored| stored.entry.id == request.id)
            .ok_or_else(|| LifecycleRefusal::ProposalNotFound(request.id.clone()).to_string())?;
        let next = handle_proposal_decide(stored.entry.state, &request).map_err(|e| e.to_string())?;
        let mut properties = Map::new();
        properties.insert("type".into(), PROPOSAL_TYPE.into());
        properties.insert("tenant_id".into(), target.verified.tenant().into());
        properties.insert("status".into(), next.stored().into());
        properties.insert("reviewer".into(), request.reviewer.clone().into());
        properties.insert("review_reason".into(), request.reason.clone().into());
        let operations = bounded(vec![
            json!({"op": "upsert_node", "id": request.id, "properties": properties}),
        ])?;
        commit(state, &target, operations).await?;
        Ok(ProposalDecideReceipt {
            id: request.id,
            state: next,
            pull_request: None,
        })
    }
    .await;
    typed_response::<ProposalDecideResult>(target.req_id, result)
}

/// `ProjectModeSet` (MEDULLA-SDLC-R012.8): read the project's stored mode (none
/// reads as `reference`), decide, then write the one `ProjectMode` node in one
/// `BatchUpdate`. A refusal returns before anything is written.
async fn project_mode_set(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: ProjectModeSet,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let previous = stored_project_mode(state, &target, &request.project).await?;
        let write = handle_project_mode_set(
            request,
            previous,
            medulla_server_api::knob_config::knobs().sdlc_mode_evolving_requires_approval,
        )
        .map_err(|e| e.to_string())?;
        let operations = project_mode_operations(&write, &target)?;
        commit(state, &target, operations).await?;
        Ok(ProjectModeReceipt {
            node_id: write.node_id,
            mode: write.request.mode,
            previous: write.previous,
        })
    }
    .await;
    typed_response::<ProjectModeSetResult>(target.req_id, result)
}

/// The stored mode of `project` in the request graph, `reference` when no record
/// of the verified tenant exists.
async fn stored_project_mode(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    project: &str,
) -> Result<ProjectMode, String> {
    ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    let node_id = project_mode_node_id(project);
    let Some(properties) = core.get_node_properties(&node_id) else {
        return Ok(ProjectMode::default());
    };
    if !authority.can_see_node(&properties, core.is_schema_node(&node_id)) {
        return Ok(ProjectMode::default());
    }
    let Ok(node) = medulla_types::msgpack::decode_property_value(&properties) else {
        return Ok(ProjectMode::default());
    };
    if node.get("tenant_id").and_then(Value::as_str) != Some(target.verified.tenant()) {
        return Ok(ProjectMode::default());
    }
    Ok(node
        .get("mode")
        .and_then(Value::as_str)
        .and_then(|name| serde_json::from_value(Value::String(name.to_owned())).ok())
        .unwrap_or_default())
}

/// The one `ProjectMode` node; the approval properties only when one was given.
fn project_mode_operations(
    write: &ProjectModeWrite,
    target: &SdlcTarget<'_>,
) -> Result<Vec<Value>, String> {
    let mut properties = Map::new();
    properties.insert("type".into(), PROJECT_MODE_TYPE.into());
    properties.insert("project".into(), write.request.project.clone().into());
    properties.insert("mode".into(), write.request.mode.name().into());
    if let Some(approval) = &write.request.approval {
        properties.insert("approver".into(), approval.approver.clone().into());
        properties.insert("reason".into(), approval.reason.clone().into());
        properties.insert("approved_at_ms".into(), approval.approved_at_ms.into());
    }
    stamp(&mut properties, target);
    bounded(vec![
        json!({"op": "upsert_node", "id": write.node_id, "properties": properties}),
    ])
}

/// The goal node, then one `targetsGoal` edge from each target to it.
fn goal_operations(write: &GoalWrite, target: &SdlcTarget<'_>) -> Result<Vec<Value>, String> {
    let mut properties = Map::new();
    properties.insert("type".into(), GOAL_TYPE.into());
    properties.insert("project".into(), write.goal.project.clone().into());
    properties.insert("statement".into(), write.goal.statement.clone().into());
    stamp(&mut properties, target);
    let mut operations =
        vec![json!({"op": "upsert_node", "id": write.node_id, "properties": properties})];
    for goal_target in &write.targets {
        operations.push(edge(
            &goal_target.node_id(),
            "targetsGoal",
            &write.node_id,
            target,
        ));
    }
    bounded(operations)
}

/// The request graph and the verified tenant's stored releases visible to the caller.
async fn load_releases(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
) -> Result<(Arc<GraphCore>, Vec<Release>), String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err("TENANT_SCOPE_REQUIRED".to_string());
    }
    ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    let releases = core
        .get_nodes_by_label(RELEASE_TYPE, MAX_RELEASES_READ)
        .into_iter()
        .filter(|(node_id, properties)| {
            authority.can_see_node(properties, core.is_schema_node(node_id))
        })
        .filter_map(|(_, properties)| release_record(&properties, tenant))
        .collect();
    Ok((core, releases))
}

fn release_record(properties: &[u8], tenant: &str) -> Option<Release> {
    let node = medulla_types::msgpack::decode_property_value(properties).ok()?;
    if node.get("tenant_id").and_then(Value::as_str) != Some(tenant) {
        return None;
    }
    serde_json::from_str(node.get(RELEASE_RECORD_PROPERTY)?.as_str()?).ok()
}

/// The commits reachable from `head` through `parents`, oldest first (parents
/// before children), ending at `head`. Empty when `head` is not an indexed
/// commit, so the release refuses `RELEASE_HEAD_UNKNOWN`.
fn commit_chain(core: &GraphCore, tenant: &str, head: &str) -> Vec<String> {
    let parents_of = |digest: &str| -> Option<Vec<String>> {
        let properties = core.get_node_properties(&commit_node_id(digest))?;
        let node = medulla_types::msgpack::decode_property_value(&properties).ok()?;
        if node.get("type").and_then(Value::as_str) != Some(COMMIT_TYPE) {
            return None;
        }
        // A commit stamped for another tenant is not on this tenant's chain.
        if node
            .get("tenant_id")
            .and_then(Value::as_str)
            .is_some_and(|owner| owner != tenant)
        {
            return None;
        }
        let parents = node.get("parents").and_then(Value::as_str).unwrap_or("");
        Some(
            parents
                .split(',')
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    };
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    // Iterative post-order walk: (digest, parents still to visit).
    let Some(parents) = parents_of(head) else {
        return chain;
    };
    seen.insert(head.to_owned());
    let mut stack = vec![(head.to_owned(), parents.into_iter())];
    while let Some((digest, remaining)) = stack.last_mut() {
        match remaining.next() {
            Some(parent) => {
                if chain.len() + stack.len() > MAX_CHAIN || !seen.insert(parent.clone()) {
                    continue;
                }
                if let Some(grandparents) = parents_of(&parent) {
                    stack.push((parent, grandparents.into_iter()));
                }
            }
            None => {
                chain.push(digest.clone());
                stack.pop();
            }
        }
    }
    chain
}

fn stamp(properties: &mut Map<String, Value>, target: &SdlcTarget<'_>) {
    properties.insert("tenant_id".into(), target.verified.tenant().into());
    properties.insert("_owner".into(), target.verified.agent_id().into());
}

fn edge(source: &str, relationship: &str, destination: &str, target: &SdlcTarget<'_>) -> Value {
    let mut properties = Map::from_iter([("relationship".to_string(), relationship.into())]);
    stamp(&mut properties, target);
    json!({
        "op": "upsert_edge",
        "source": source,
        "target": destination,
        "properties": properties,
    })
}

/// The sealed release node, then one `contains` edge per changeset it contains.
fn release_operations(
    write: &medulla_source_types::lifecycle::ReleaseWrite,
    target: &SdlcTarget<'_>,
) -> Result<Vec<Value>, String> {
    let release = &write.release;
    let id = release_node_id(release);
    let body =
        serde_json::to_string(release).map_err(|_| "release record encoding failed".to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), RELEASE_TYPE.into());
    properties.insert(RELEASE_RECORD_PROPERTY.into(), body.into());
    properties.insert("repo".into(), release.repo.clone().into());
    properties.insert("version".into(), release.version.clone().into());
    properties.insert("git_ref".into(), release.git_ref.clone().into());
    properties.insert(
        "changeset_head".into(),
        release.changeset_head.clone().into(),
    );
    properties.insert("published_at".into(), release.published_at.into());
    properties.insert("release_seal".into(), write.seal.clone().into());
    stamp(&mut properties, target);
    let mut operations = vec![json!({"op": "upsert_node", "id": id, "properties": properties})];
    for changeset in &write.contains {
        operations.push(edge(
            &id,
            "contains",
            &commit_node_id(changeset),
            target,
        ));
    }
    bounded(operations)
}

/// The deployment node and, when a release names its image, the `deploys` edge from the
/// deployment to the release.
fn deployment_operations(
    deployment: &Deployment,
    target: &SdlcTarget<'_>,
) -> Result<Vec<Value>, String> {
    let id = deployment_node_id(deployment);
    let mut properties = Map::new();
    properties.insert("type".into(), DEPLOYMENT_TYPE.into());
    properties.insert("environment".into(), deployment.environment.clone().into());
    properties.insert("workload".into(), deployment.workload.clone().into());
    properties.insert(
        "image_digest".into(),
        deployment.image_digest.clone().into(),
    );
    properties.insert("started_at".into(), deployment.started_at.into());
    properties.insert("finished_at".into(), deployment.finished_at.into());
    properties.insert("outcome".into(), json!(deployment.outcome));
    if let Some(release_ref) = &deployment.release_ref {
        properties.insert("release_ref".into(), release_ref.clone().into());
    }
    stamp(&mut properties, target);
    let mut operations = vec![json!({"op": "upsert_node", "id": id, "properties": properties})];
    if let Some(release_ref) = &deployment.release_ref {
        let (source, destination) = deploys_endpoints(&id, release_ref);
        operations.push(edge(&source, "deploys", &destination, target));
    }
    bounded(operations)
}

/// The endpoints of a `deploys` edge: the deployment node to the release node it rolls out.
/// This is the direction of the ontology (`Deployment` to `Release`). `Release::id` is
/// `repo@version`; the release node id is `release:repo:version`.
fn deploys_endpoints(deployment_node: &str, release_ref: &str) -> (String, String) {
    let node = release_ref.replacen('@', ":", 1);
    (deployment_node.to_owned(), format!("release:{node}"))
}

fn bounded(operations: Vec<Value>) -> Result<Vec<Value>, String> {
    if operations.len() > MAX_BATCH_OPERATIONS {
        return Err("INVALID_ARGUMENT: the release exceeds one BatchUpdate".to_string());
    }
    Ok(operations)
}

/// Commit `operations` as ONE `BatchUpdate` through the ordinary graph gateway.
async fn commit(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    operations: Vec<Value>,
) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "lifecycle batch encoding failed".to_string())?;
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        Method::BatchUpdate { operations_msgpack },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::lifecycle::DeploymentOutcome;

    fn deployment(release_ref: Option<&str>) -> Deployment {
        Deployment {
            environment: "prod".into(),
            workload: "engine".into(),
            image_digest: "sha256:aa".into(),
            release_ref: release_ref.map(str::to_owned),
            started_at: 1,
            finished_at: 2,
            outcome: DeploymentOutcome::Succeeded,
        }
    }

    #[test]
    fn a_release_ref_maps_to_its_release_node_id() {
        let release = Release {
            repo: "medulla".into(),
            version: "1.0.0".into(),
            git_ref: "refs/tags/v1.0.0".into(),
            changeset_head: "c".into(),
            artifact_digests: vec![],
            published_at: 1,
        };
        assert_eq!(
            format!("release:{}", release.id().replacen('@', ":", 1)),
            release_node_id(&release)
        );
        assert!(deployment(None).release_ref.is_none());
    }

    // spec: MEDULLA-SDLC-R012.7
    #[test]
    fn the_deploys_edge_runs_from_the_deployment_to_the_release() {
        let release = Release {
            repo: "medulla".into(),
            version: "1.0.0".into(),
            git_ref: "refs/tags/v1.0.0".into(),
            changeset_head: "c".into(),
            artifact_digests: vec![],
            published_at: 1,
        };
        let deployment = deployment(Some(&release.id()));
        let id = deployment_node_id(&deployment);
        let (source, destination) = deploys_endpoints(&id, &release.id());
        assert_eq!(source, id);
        assert!(source.starts_with("deployment:"));
        assert_eq!(destination, release_node_id(&release));
    }
}
