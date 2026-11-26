use super::*;

mod delivery;

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_work_types::orchestration::visual_observation::{
    ObservationRefusal, VisualObservation, VisualObservationReceipt,
    VISUAL_OBSERVATION_NODE_TYPE, VISUAL_OBSERVATION_RECORD_PROPERTY,
};
use medulla_protocol_types::result_contract::ingestion::{
    GoalPut as GoalPutResult, ProjectModeGet as ProjectModeGetResult,
    ProjectModeSet as ProjectModeSetResult,
    FileDefect as FileDefectResult, SpecStatus as SpecStatusResult, LifecycleTrace as LifecycleTraceResult, PutObservation as PutObservationResult, TransitionDefect as TransitionDefectResult, GetDefect as GetDefectResult, SpecQuestion as SpecQuestionResult, LandingPolicyPut as LandingPolicyPutResult, ProposalDecide as ProposalDecideResult, ProposalList as ProposalListResult,
    ProposalPut as ProposalPutResult, RecordDeployment as RecordDeploymentResult,
    RecordGateVerdict as RecordGateVerdictResult, RecordRelease as RecordReleaseResult,
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
    handle_project_mode_get, handle_project_mode_set, project_mode_record, handle_proposal_decide, handle_proposal_put, project_mode_node_id,
    check_pull_request_target, pull_request_arguments, pull_request_outcome,
    ProposalDecision, OPEN_PULL_REQUEST_TOOL, REPOSITORY_NODE_TYPE, resolve_source_connection, SourceConnection,
    handle_file_defect, handle_get_defect, handle_transition_defect, defect_node_id, defect_from_node,
    DefectFileOutcome, GetDefectReceipt, GetDefectRequest, TransitionDefectRequest, Defect, DefectWrite, FileDefectRequest,
    DEFECT_TYPE, LifecycleRefusal, Proposal, ProposalDecideReceipt, ProposalDecideRequest, ProposalEvidence,
    ProposalKind, ProposalListEntry, ProposalListRequest, ProposalReceipt, ProposalState,
    ProposalWrite, PullRequestError, PullRequestTarget, MAX_PROPOSAL_EVIDENCE, release_node_id, Deployment, DeploymentInput,
    DeploymentReceipt, Goal, GoalReceipt, GoalWrite, ProjectMode, ProjectModeReceipt,
    ProjectModeGetRequest, ProjectModeSet, ProjectModeWrite, Release, ReleaseReceipt,
};
use medulla_source_types::gate_verdict::{
    gate_verdict_node_id, handle_landing_policy_put, GateVerdictReceipt, GateVerdictRecord,
    LandingPolicy, LandingPolicyPutRequest, LandingPolicyReceipt, GATE_VERDICT_RECORD_PROPERTY,
    GATE_VERDICT_SEAL_PROPERTY, GATE_VERDICT_TYPE, LANDING_POLICY_RECORD_PROPERTY,
    LANDING_POLICY_TYPE,
};
use medulla_source_types::lifecycle::DefectIntake;
use medulla_work_types::step_verdict::VerificationVerdict;
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
const MAX_DEFECTS_READ: usize = 100_000;
const MAX_SPEC_VIEW_READ: usize = 200_000;
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
        Method::RecordGateVerdict {
            changeset,
            branch,
            gate_id,
            input_digest,
            verdict,
            evidence_digest,
            duration_ms,
        } => {
            let record = GateVerdictRecord {
                changeset,
                gate_id,
                input_digest,
                verdict,
                evidence_digest,
                duration_ms,
            };
            dispatch_boxed(record_gate_verdict(state, target, branch, record)).await
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
        Method::ProjectModeGet { request } => {
            dispatch_boxed(project_mode_get(state, target, request)).await
        }
        Method::SpecQuestion {
            question,
            subject,
            graph,
        } => {
            let target = SdlcTarget {
                graph: &graph,
                ..target
            };
            dispatch_boxed(spec_question(state, target, question, subject)).await
        }
        Method::DeliveryOpen { request } => {
            dispatch_boxed(delivery::open(state, target, request)).await
        }
        Method::DeliveryFeedback { event } => {
            dispatch_boxed(delivery::feedback(state, target, event)).await
        }
        Method::DeliveryReport { write } => {
            dispatch_boxed(delivery::report(state, target, write)).await
        }
        Method::DeliveryStatus { request } => {
            dispatch_boxed(delivery::status(state, target, request)).await
        }
        Method::DeliveryList { request } => {
            dispatch_boxed(delivery::list(state, target, request)).await
        }
        Method::DeliveryStop { request } => {
            dispatch_boxed(delivery::stop(state, target, request)).await
        }
        Method::DeliveryRetry { request } => {
            dispatch_boxed(delivery::retry(state, target, request)).await
        }
        Method::LifecycleTrace { request } => {
            dispatch_boxed(lifecycle_trace(state, target, request)).await
        }
        Method::SpecStatus {
            repo,
            rev,
            spec_id,
            graph,
        } => {
            let target = SdlcTarget {
                graph: &graph,
                ..target
            };
            dispatch_boxed(spec_status(state, target, repo, rev, spec_id)).await
        }
        Method::FileDefect { request } => {
            dispatch_boxed(file_defect(state, target, request)).await
        }
        Method::TransitionDefect { request } => {
            dispatch_boxed(transition_defect(state, target, request)).await
        }
        Method::GetDefect { request } => dispatch_boxed(get_defect(state, target, request)).await,
        Method::LandingPolicyPut { request } => {
            dispatch_boxed(landing_policy_put(state, target, request)).await
        }
        Method::PutObservation { observation } => {
            dispatch_boxed(put_observation(state, target, observation)).await
        }
        other => return ControlFlow::Continue(other),
    };
    ControlFlow::Break(response)
}

/// Recover the deliveries of `graph` whose assignment lease lapsed (MEDULLA-SDLC-R019.13), once
/// at engine start, under the verified context of the engine's service actor. Answers how many
/// deliveries changed.
pub async fn recover_deliveries_at_boot(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    verified: &VerifiedRequestContext,
) -> Result<u32, String> {
    let target = SdlcTarget {
        graph,
        req_id: 0,
        caller: None,
        verified,
    };
    delivery::recover_at_boot(state, target).await
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

/// `RecordGateVerdict` (MEDULLA-SCM-R008): decide in `medulla_h_ingest::gate_verdict::record`,
/// then write the sealed verdict node and its one `judges` edge in ONE gateway-routed batch.
async fn record_gate_verdict(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    branch: String,
    request: GateVerdictRecord,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        request.validate().map_err(|e| e.to_string())?;
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
        let read = |id: &str| -> Option<Value> {
            let blob = core.get_node_properties(id)?;
            if !authority.can_see_node(&blob, core.is_schema_node(id)) {
                return None;
            }
            medulla_types::msgpack::decode_property_value(&blob).ok()
        };
        let changeset_node = read(&commit_node_id(&request.changeset)).filter(|node| {
            node.get("type").and_then(Value::as_str) == Some(COMMIT_TYPE)
                && node
                    .get("tenant_id")
                    .and_then(Value::as_str)
                    .is_none_or(|owner| owner == tenant)
        });
        let changeset_known = changeset_node.is_some();
        // The repository a failed gate files its defect under: the changeset's own.
        let repo = changeset_node
            .as_ref()
            .and_then(|node| node.get("repo").and_then(Value::as_str))
            .unwrap_or(target.graph)
            .to_string();
        let stored: Option<GateVerdictRecord> = read(&gate_verdict_node_id(
            &request.changeset,
            &request.gate_id,
        ))
        .and_then(|node| {
            serde_json::from_str(node.get(GATE_VERDICT_RECORD_PROPERTY)?.as_str()?).ok()
        });
        let tree_digest = if changeset_known {
            changeset_tree_digest(state, &target, &branch, &request.changeset).await?
        } else {
            None
        };
        let write = medulla_h_ingest::gate_verdict::record(
            request,
            changeset_known,
            tree_digest.as_deref(),
            stored.as_ref(),
        )
        .map_err(|e| e.to_string())?;
        let body = serde_json::to_string(&write.record)
            .map_err(|_| "verdict record encoding failed".to_string())?;
        let mut properties = Map::new();
        properties.insert("type".into(), GATE_VERDICT_TYPE.into());
        properties.insert(GATE_VERDICT_RECORD_PROPERTY.into(), body.into());
        properties.insert(GATE_VERDICT_SEAL_PROPERTY.into(), write.seal.clone().into());
        properties.insert("changeset".into(), write.record.changeset.clone().into());
        properties.insert("gate_id".into(), write.record.gate_id.clone().into());
        properties.insert("verdict".into(), write.record.verdict.name().into());
        let recorded_at = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        properties.insert("recorded_at_ms".into(), recorded_at.into());
        stamp(&mut properties, &target);
        let (from, to) = write.judges();
        let mut operations = vec![
            json!({"op": "upsert_node", "id": write.node_id, "properties": properties}),
            edge(&from, "judges", &to, &target),
        ];
        // MEDULLA-SDLC-R003.3: a failed gate files a defect in the verdict's own batch. A
        // replayed verdict files nothing, so one verdict is one symptom.
        if write.record.verdict == VerificationVerdict::Fail && !write.replayed {
            let filing = FileDefectRequest {
                repo: repo.clone(),
                intake: DefectIntake::GateFailure,
                title: format!("gate {} failed", write.record.gate_id),
                severity: None,
                source_record_id: write.node_id.clone(),
                observed_at: recorded_at,
                text: format!("gate {} failed", write.record.gate_id),
            };
            let filed = file_defect_write(state, &target, filing).await?;
            operations.extend(defect_operations(&filed, &target)?);
        }
        commit(state, &target, operations).await?;
        Ok(GateVerdictReceipt {
            node_id: write.node_id,
            seal: write.seal,
            replayed: write.replayed,
        })
    }
    .await;
    typed_response::<RecordGateVerdictResult>(target.req_id, result)
}

/// The node types a competency question reads.
const SPEC_VIEW_TYPES: [&str; 13] = [
    "PlanUnknown",
    "Requirement",
    "SYMBOL",
    "TaskLine",
    "Contract",
    "ConstitutionPrinciple",
    "Goal",
    COMMIT_TYPE,
    "PlanSection",
    RELEASE_TYPE,
    DEPLOYMENT_TYPE,
    DEFECT_TYPE,
    "ReleaseBundle",
];
/// The relationships a competency question reads.
const SPEC_VIEW_RELATIONSHIPS: [&str; 15] = [
    "implementedByCommit",
    "verifiedBy",
    "implementedBy",
    "boundTest",
    "citesRequirement",
    "crossesSeam",
    "dependsOnRow",
    "covers",
    "targetsGoal",
    "changes",
    "implements",
    "contains",
    "deploys",
    "affects",
    "inBundle",
];
const SPEC_VIEW_EXTRA_RELATIONSHIPS: [&str; 2] = ["forRow", "governedBy"];

/// `SpecQuestion` (MEDULLA-SDLC-R014.6): read the records and edges the caller can see once,
/// then answer in `medulla_sdlc::spec_artifact_questions::answer`. UQL has no negation across
/// rows, so no UQL runs here.
async fn spec_question(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    question: medulla_source_types::spec_question::SpecQuestion,
    subject: Option<String>,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        // MEDULLA-SDLC-R016.4: the loop's tools are the tools its contracts name, read from
        // the graph. A graph with no contract for the research task answers no row.
        if question == medulla_source_types::spec_question::SpecQuestion::ResearchTools {
            let tools = research_tools(state, &target).await?;
            return Ok(medulla_source_types::spec_question::SpecQuestionAnswer {
                rows: tools
                    .into_iter()
                    .map(|held| {
                        std::collections::BTreeMap::from([
                            ("connector".to_string(), held.connector),
                            ("tool".to_string(), held.tool),
                        ])
                    })
                    .collect(),
                digests: Vec::new(),
            });
        }
        // MEDULLA-SDLC-R016.1: the unknowns are read only for a codebase that takes proposals.
        // The repository is the graph; a project with no mode record reads as `reference`.
        if question == medulla_source_types::spec_question::SpecQuestion::PlanUnknowns {
            let mode = stored_project_mode(state, &target, target.graph).await?;
            if mode == ProjectMode::Reference {
                return Err(LifecycleRefusal::ModeReferenceNoProposals(target.graph.to_owned())
                    .to_string());
            }
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
        let view = spec_graph_view(&core, &authority, tenant);
        medulla_sdlc::spec_artifact_questions::answer(question, subject.as_deref(), &view)
            .map_err(|e| e.to_string())
    }
    .await;
    typed_response::<SpecQuestionResult>(target.req_id, result)
}

/// The tools the research task's contracts name in the request graph, read under the caller's
/// view (MEDULLA-SDLC-R016.4).
async fn research_tools(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
) -> Result<Vec<medulla_sdlc::lifecycle::research::ResearchTool>, String> {
    let read = medulla_h_decide::decide::read_contract_tools(
        state,
        target.graph,
        target.verified,
        medulla_sdlc::lifecycle::research::RESEARCH_TASK_IRI,
    )
    .await?;
    Ok(read
        .into_iter()
        .map(|(connector, tool)| medulla_sdlc::lifecycle::research::ResearchTool {
            connector,
            tool,
        })
        .collect())
}

/// An agent library with no revisions: the engine default policy needs no stored revision.
struct NoPolicyLibrary;

impl medulla_decision_policy::policy::PolicyLibrary for NoPolicyLibrary {
    fn policy_revisions(
        &self,
        _tenant_id: &str,
        _component_id: &str,
    ) -> Result<Vec<medulla_agent_types::agent_component::AgentComponentEntry>, String> {
        Ok(Vec::new())
    }

    fn champion_policy_target(
        &self,
        _tenant_id: &str,
        _policy_id: &str,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// The agent library as the decision-policy seam reads it.
struct LibraryHandle(Arc<medulla_persistence_records::agent_library::AgentLibraryStore>);

impl medulla_decision_policy::policy::PolicyLibrary for LibraryHandle {
    fn policy_revisions(
        &self,
        tenant_id: &str,
        component_id: &str,
    ) -> Result<Vec<medulla_agent_types::agent_component::AgentComponentEntry>, String> {
        self.0
            .component_revisions(tenant_id, component_id)
            .map_err(|error| error.to_string())
    }

    fn champion_policy_target(
        &self,
        tenant_id: &str,
        policy_id: &str,
    ) -> Result<Option<String>, String> {
        self.0.champion_policy_target(tenant_id, policy_id)
    }
}

/// The trace limits and the policy digest of one request (MEDULLA-SDLC-R006.7): the
/// `lifecycle.*` knobs of the policy the request names.
async fn trace_limits(
    state: &Arc<RwLock<ServerState>>,
    tenant: &str,
    reference: &medulla_agent_types::agent_component::DecisionPolicyRef,
) -> Result<(medulla_source_types::lifecycle::TraceLimits, String), String> {
    use medulla_decision_types::decision::knobs::KnobValue;
    use medulla_decision_types::decision::lifecycle::KNOBS;
    let resolved = match reference {
        medulla_agent_types::agent_component::DecisionPolicyRef::Default => {
            medulla_decision_policy::policy::resolve_policy(&NoPolicyLibrary, tenant, reference)?
        }
        pinned => {
            let store = medulla_persistence_records::agent_library::shared_agent_library(state)
                .await?;
            medulla_decision_policy::policy::resolve_policy(&LibraryHandle(store), tenant, pinned)?
        }
    };
    let integer = |index: usize| match resolved.policy.knob(&KNOBS[index]) {
        KnobValue::Integer(value) | KnobValue::DecimalMicros(value) => {
            u64::try_from(value).unwrap_or(0)
        }
        KnobValue::DurationMs(value) => value,
    };
    let limits = medulla_source_types::lifecycle::TraceLimits {
        max_depth: u8::try_from(integer(0)).unwrap_or(u8::MAX),
        node_budget: usize::try_from(integer(1)).unwrap_or(usize::MAX),
    };
    Ok((limits, resolved.digest))
}

/// `LifecycleTrace` (MEDULLA-SDLC-R006.3 to R006.5): resolve the limits from the request's
/// policy, check the request against them, read the lifecycle records around the anchor that
/// the caller can see, and walk them through `lifecycle_trace`. Writes nothing.
async fn lifecycle_trace(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: medulla_source_types::lifecycle::LifecycleTraceRequest,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let (limits, policy_digest) = trace_limits(state, tenant, &request.policy).await?;
        let request = medulla_source_types::lifecycle::handle_lifecycle_trace(request, limits)
            .map_err(|e| e.to_string())?;
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
        let store = medulla_h_ingest::lifecycle_status::lifecycle_store_around(
            &core,
            tenant,
            &request.anchor,
            request.depth,
            limits.node_budget,
            &|blob: &[u8], schema: bool| authority.can_see_node(blob, schema),
        );
        let mut answer = medulla_core::lifecycle::trace::lifecycle_trace(&store, &request, limits);
        answer.policy_digest = policy_digest;
        Ok(answer)
    }
    .await;
    typed_response::<LifecycleTraceResult>(target.req_id, result)
}

/// `SpecStatus` (MEDULLA-SDLC-R005.3, R005.4): read the requirement rows, changesets and test
/// bindings of one repository that the caller can see, then derive each row's state through
/// `derive_spec_status`. A refusal returns no row and writes nothing.
async fn spec_status(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    repo: String,
    rev: String,
    spec_id: Option<String>,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let request = medulla_source_types::lifecycle::handle_spec_status(
            medulla_source_types::lifecycle::SpecStatusRequest { repo, rev, spec_id },
        )
        .map_err(|e| e.to_string())?;
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
        let view = medulla_h_ingest::lifecycle_status::spec_status_view(
            &core,
            tenant,
            &request.repo,
            &|blob: &[u8], schema: bool| authority.can_see_node(blob, schema),
        );
        medulla_core::lifecycle::status::answer_spec_status(
            &request.repo,
            &request.rev,
            request.spec_id.as_deref(),
            &view,
        )
        .map_err(|e| e.to_string())
    }
    .await;
    typed_response::<SpecStatusResult>(target.req_id, result)
}

/// The visible records of the spec artifact types and the typed edges among them.
fn spec_graph_view(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
) -> medulla_sdlc::spec_artifact_questions::SpecGraphView {
    use medulla_sdlc::spec_artifact_questions::{SpecGraphView, ViewNode};
    let mut view = SpecGraphView::default();
    for node_type in SPEC_VIEW_TYPES {
        for (id, blob) in core.get_nodes_by_label(node_type, MAX_SPEC_VIEW_READ) {
            if !authority.can_see_node(&blob, core.is_schema_node(&id)) {
                continue;
            }
            let Ok(Value::Object(node)) = medulla_types::msgpack::decode_property_value(&blob)
            else {
                continue;
            };
            if node.get("tenant_id").and_then(Value::as_str).is_some_and(|owner| owner != tenant) {
                continue;
            }
            let props = node
                .iter()
                .map(|(key, value)| {
                    let text = match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (key.clone(), text)
                })
                .collect();
            view.nodes.insert(
                id,
                ViewNode {
                    node_type: node_type.to_string(),
                    props,
                    digest: medulla_source_types::lifecycle::sha256_hex(&blob),
                },
            );
        }
    }
    let ids: Vec<String> = view.nodes.keys().cloned().collect();
    for source in &ids {
        for destination in core.get_successors(source).unwrap_or_default() {
            if !view.nodes.contains_key(&destination) {
                continue;
            }
            for (relationship, _) in core.typed_edges_between(source, &destination) {
                if SPEC_VIEW_RELATIONSHIPS.contains(&relationship.as_str())
                    || SPEC_VIEW_EXTRA_RELATIONSHIPS.contains(&relationship.as_str())
                {
                    view.edges
                        .insert((source.clone(), relationship, destination.clone()));
                }
            }
        }
    }
    view
}

/// `FileDefect` (MEDULLA-SDLC-R003.3): file one symptom through the one defect book and write
/// the touched `Defect` node in one gateway-routed batch.
async fn file_defect(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: FileDefectRequest,
) -> Response {
    let result = async {
        if target.verified.tenant().trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let request = handle_file_defect(request).map_err(|e| e.to_string())?;
        let write = file_defect_write(state, &target, request).await?;
        commit(state, &target, defect_operations(&write, &target)?).await?;
        Ok(write.receipt())
    }
    .await;
    typed_response::<FileDefectResult>(target.req_id, result)
}

/// The stored defect and its fix changeset, when the caller can see the node.
async fn stored_defect(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    repo: &str,
    defect_id: &str,
) -> Result<(String, Defect, Option<String>), String> {
    let node_id = defect_node_id(repo, defect_id);
    let unknown = || LifecycleRefusal::EndpointUnknown(node_id.clone()).to_string();
    let node = visible_node(state, target, &node_id).await?.ok_or_else(unknown)?;
    let defect = defect_from_node(&node, target.verified.tenant()).ok_or_else(unknown)?;
    let fix = node.get("fix_changeset").and_then(Value::as_str).map(str::to_owned);
    Ok((node_id, defect, fix))
}

/// `TransitionDefect` (MEDULLA-SDLC-R003.4): decide the move through the defect machine against
/// the stored defect, then write the changed `Defect` node in one gateway-routed batch. A move
/// to `verified` reads the sealed gate verdict of the stored fix changeset first.
async fn transition_defect(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: TransitionDefectRequest,
) -> Response {
    let result = async {
        if target.verified.tenant().trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let (node_id, defect, stored_fix) =
            stored_defect(state, &target, &request.repo, &request.defect_id).await?;
        let mut verdict_passed = false;
        if let (Some(fix), Some(gate)) = (stored_fix.as_deref(), request.gate_id.as_deref()) {
            if let Some(node) = visible_node(state, &target, &gate_verdict_node_id(fix, gate)).await? {
                verdict_passed = node
                    .get(GATE_VERDICT_RECORD_PROPERTY)
                    .and_then(Value::as_str)
                    .and_then(|body| serde_json::from_str::<GateVerdictRecord>(body).ok())
                    .is_some_and(|record| record.verdict == VerificationVerdict::Pass);
            }
        }
        let moved = handle_transition_defect(&request, &defect, stored_fix.as_deref(), verdict_passed)
            .map_err(|e| e.to_string())?;
        let write = DefectWrite {
            defect: moved.defect.clone(),
            outcome: DefectFileOutcome::Absorbed,
        };
        let mut properties = defect_properties(&write, &target)?;
        if let Some(fix) = &moved.fix_changeset {
            properties.insert("fix_changeset".into(), fix.clone().into());
        }
        commit(
            state,
            &target,
            vec![json!({"op": "upsert_node", "id": node_id, "properties": properties})],
        )
        .await?;
        Ok(moved.receipt())
    }
    .await;
    typed_response::<TransitionDefectResult>(target.req_id, result)
}

/// `GetDefect` (MEDULLA-SDLC-R003.5): the stored defect with its symptom list, read through the
/// caller's visible store; nothing is written.
async fn get_defect(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: GetDefectRequest,
) -> Response {
    let result = async {
        if target.verified.tenant().trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let request = handle_get_defect(request).map_err(|e| e.to_string())?;
        let (node_id, defect, fix_changeset) =
            stored_defect(state, &target, &request.repo, &request.defect_id).await?;
        Ok(GetDefectReceipt {
            node_id,
            fix_changeset,
            defect,
        })
    }
    .await;
    typed_response::<GetDefectResult>(target.req_id, result)
}

/// File `request` against the stored defects of its repository: the decision, not the write.
async fn file_defect_write(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    request: FileDefectRequest,
) -> Result<DefectWrite, String> {
    let existing = load_defects(state, target, &request.repo).await?;
    Ok(medulla_h_ingest::defect_intake::file_defect(existing, request))
}

/// The stored defects of `repo` the caller can see, owned by the verified tenant.
async fn load_defects(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    repo: &str,
) -> Result<Vec<Defect>, String> {
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
    let tenant = target.verified.tenant();
    let mut defects: Vec<Defect> = core
        .get_nodes_by_label(DEFECT_TYPE, MAX_DEFECTS_READ)
        .into_iter()
        .filter(|(node_id, blob)| authority.can_see_node(blob, core.is_schema_node(node_id)))
        .filter_map(|(_, blob)| {
            let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            medulla_source_types::lifecycle::defect_from_node(&node, tenant)
        })
        .filter(|defect| defect.repo == repo)
        .collect();
    // The defect book numbers a new defect by position, so the stored order is the id order.
    defects.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(defects)
}

/// The one `Defect` node of a filing, stamped with the verified tenant and owner.
fn defect_properties(write: &DefectWrite, target: &SdlcTarget<'_>) -> Result<Map<String, Value>, String> {
    let mut properties = write.node_properties()?;
    stamp(&mut properties, target);
    Ok(properties)
}

fn defect_operations(write: &DefectWrite, target: &SdlcTarget<'_>) -> Result<Vec<Value>, String> {
    bounded(vec![
        json!({"op": "upsert_node", "id": write.node_id(), "properties": defect_properties(write, target)?}),
    ])
}

/// `LandingPolicyPut` (MEDULLA-SCM-R008): the landing policy of one branch is a governed record.
/// Decide in `handle_landing_policy_put` against the stored policy (a lowering needs an operator
/// approval), then write the one `LandingPolicy` node in one gateway-routed batch.
async fn landing_policy_put(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: LandingPolicyPutRequest,
) -> Response {
    let result = async {
        if target.verified.tenant().trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let stored: Option<LandingPolicy> = visible_node(
            state,
            &target,
            &medulla_source_types::gate_verdict::landing_policy_node_id(
                &request.policy.repo,
                &request.policy.branch,
            ),
        )
        .await?
        .and_then(|node| {
            serde_json::from_str(node.get(LANDING_POLICY_RECORD_PROPERTY)?.as_str()?).ok()
        });
        let write = handle_landing_policy_put(request, stored.as_ref()).map_err(|e| e.to_string())?;
        let body = serde_json::to_string(&write.request.policy)
            .map_err(|_| "landing policy encoding failed".to_string())?;
        let mut properties = Map::new();
        properties.insert("type".into(), LANDING_POLICY_TYPE.into());
        properties.insert(LANDING_POLICY_RECORD_PROPERTY.into(), body.into());
        properties.insert("repo".into(), write.request.policy.repo.clone().into());
        properties.insert("branch".into(), write.request.policy.branch.clone().into());
        if let Some(approval) = &write.request.approval {
            properties.insert("approver".into(), approval.approver.clone().into());
            properties.insert("reason".into(), approval.reason.clone().into());
            properties.insert("approved_at_ms".into(), approval.approved_at_ms.into());
        }
        stamp(&mut properties, &target);
        if !write.replayed {
            commit(
                state,
                &target,
                vec![json!({"op": "upsert_node", "id": write.node_id, "properties": properties})],
            )
            .await?;
        }
        Ok(LandingPolicyReceipt {
            node_id: write.node_id,
            lowered: write.lowered,
            replayed: write.replayed,
        })
    }
    .await;
    typed_response::<LandingPolicyPutResult>(target.req_id, result)
}

/// `PutObservation` (MEDULLA-ORCH-VOCAB-R810): store one run-bound visual observation. The
/// envelope is checked against its bounds, its tenant must be the verified caller's and its run a
/// `RunTrace` the caller's tenant holds (a foreign run and a missing one read the same). An id
/// stored with the same record is a replay; with another content digest it refuses
/// `OBSERVATION_CONFLICT`. The one `VisualObservation` node is written in one gateway-routed batch.
async fn put_observation(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    observation: VisualObservation,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        observation.validate().map_err(|refusal| refusal.to_string())?;
        if observation.tenant_id != tenant {
            return Err(ObservationRefusal::ScopeUnknown.to_string());
        }
        let run = visible_node(state, &target, &observation.run_id).await?;
        if run
            .as_ref()
            .and_then(|node| node.get("type"))
            .and_then(Value::as_str)
            != Some("RunTrace")
        {
            return Err(ObservationRefusal::ScopeUnknown.to_string());
        }
        let node_id = observation.node_id();
        let stored: Option<VisualObservation> = visible_node(state, &target, &node_id)
            .await?
            .and_then(|node| {
                serde_json::from_str(node.get(VISUAL_OBSERVATION_RECORD_PROPERTY)?.as_str()?).ok()
            });
        let replayed = match &stored {
            Some(existing) if *existing == observation => true,
            Some(existing) if existing.content_digest != observation.content_digest => {
                return Err(ObservationRefusal::Conflict.to_string());
            }
            _ => false,
        };
        if !replayed {
            let body = serde_json::to_string(&observation)
                .map_err(|_| "observation encoding failed".to_string())?;
            let mut properties = Map::new();
            properties.insert("type".into(), VISUAL_OBSERVATION_NODE_TYPE.into());
            properties.insert(VISUAL_OBSERVATION_RECORD_PROPERTY.into(), body.into());
            properties.insert("run_id".into(), observation.run_id.clone().into());
            properties.insert(
                "observation_id".into(),
                observation.observation_id.clone().into(),
            );
            properties.insert(
                "content_digest".into(),
                observation.content_digest.clone().into(),
            );
            stamp(&mut properties, &target);
            commit(
                state,
                &target,
                vec![json!({"op": "upsert_node", "id": node_id, "properties": properties})],
            )
            .await?;
        }
        Ok(VisualObservationReceipt { node_id, replayed })
    }
    .await;
    typed_response::<PutObservationResult>(target.req_id, result)
}

/// The stored node `id` of the request graph, decoded, when the caller can see it and it
/// belongs to the verified tenant.
async fn visible_node(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    id: &str,
) -> Result<Option<Value>, String> {
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
    let Some(blob) = core.get_node_properties(id) else {
        return Ok(None);
    };
    if !authority.can_see_node(&blob, core.is_schema_node(id)) {
        return Ok(None);
    }
    Ok(medulla_types::msgpack::decode_property_value(&blob)
        .ok()
        .filter(|node| {
            node.get("tenant_id")
                .and_then(Value::as_str)
                .is_none_or(|owner| owner == target.verified.tenant())
        }))
}

/// The digest of the canonical tree listing of `changeset` on the chain of `branch`; `None`
/// when the branch is empty or the changeset is not on it. The chain is read from the registry
/// directly: the default branch has a chain but no branch row, so `resolve_target` (which
/// refuses it) is not the read.
#[cfg(all(feature = "redb", feature = "blob"))]
async fn changeset_tree_digest(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    branch: &str,
    changeset: &str,
) -> Result<Option<String>, String> {
    use medulla_mutation_types::branch::BranchRef;
    let registry = {
        let guard = state.read().await;
        medulla_persistence::branch_registry_store::shared_branch_registry(&guard)?
    };
    let reference = BranchRef {
        tenant: target.verified.tenant().to_string(),
        graph: target.graph.to_string(),
        name: branch.to_string(),
    };
    let changeset = changeset.to_string();
    tokio::task::spawn_blocking(move || {
        let mut found = None;
        for commit in registry.chain(&reference)? {
            let Some(bytes) = registry.commit_payload(&reference, &commit.id)? else {
                continue;
            };
            if medulla_source_types::source_changeset::decode_changeset(&bytes)
                .is_ok_and(|candidate| candidate.id == changeset)
            {
                found = Some(commit.id.to_string());
                break;
            }
        }
        let Some(commit_id) = found else {
            return Ok(None);
        };
        let tree = medulla_compute::source_tree::tree_at(
            &super::read_tree::RegistryTree {
                registry: &registry,
                branch: &reference,
            },
            &commit_id,
            "",
            0,
        )
        .map_err(|error| format!("{error:?}"))?;
        Ok(Some(medulla_source_types::gate_verdict::tree_listing_digest(
            tree.iter()
                .map(|entry| (entry.path.as_str(), entry.blob_digest.as_str(), entry.mode)),
        )))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(not(all(feature = "redb", feature = "blob")))]
async fn changeset_tree_digest(
    _state: &Arc<RwLock<ServerState>>,
    _target: &SdlcTarget<'_>,
    _branch: &str,
    _changeset: &str,
) -> Result<Option<String>, String> {
    Err("RecordGateVerdict requires the `redb` and `blob` features".to_string())
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
        // MEDULLA-SDLC-R016.2: a research entry is cited, before its evidence is resolved.
        if medulla_sdlc::lifecycle::research::is_research_entry(&proposal) {
            medulla_sdlc::lifecycle::research::check_research_entry(&proposal)
                .map_err(|e| e.to_string())?;
        }
        let resolved = resolve_evidence(state, &target, &proposal).await?;
        // MEDULLA-SDLC-R016.4: a cited source that names the research tool that read it names a
        // tool the graph's contracts hold.
        if medulla_sdlc::lifecycle::research::is_research_entry(&proposal) {
            check_cited_tools(state, &target, &proposal).await?;
        }
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

/// Refuse `RESEARCH_TOOL_UNKNOWN` for a cited record whose `research_tool` property names a
/// tool the research contracts of the graph do not hold. A record that names no tool is not read
/// against the contracts.
async fn check_cited_tools(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    proposal: &Proposal,
) -> Result<(), String> {
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
    let named: BTreeSet<String> = proposal
        .evidence
        .iter()
        .filter(|cited| cited.kind == medulla_source_types::lifecycle::EvidenceKind::ResearchCitation)
        .filter_map(|cited| {
            let blob = core.get_node_properties(&cited.record_id)?;
            if !authority.can_see_node(&blob, core.is_schema_node(&cited.record_id)) {
                return None;
            }
            let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            node.get("research_tool")?.as_str().map(str::to_owned)
        })
        .collect();
    if named.is_empty() {
        return Ok(());
    }
    let known = research_tools(state, target).await?;
    for tool in named {
        medulla_sdlc::lifecycle::research::check_research_tool(&known, &tool)
            .map_err(|refusal| refusal.to_string())?;
    }
    Ok(())
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
    statement: String,
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
        statement: text("statement").unwrap_or_default().to_owned(),
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

/// The provenance guard of `SubmitChangeset` (MEDULLA-SDLC-R017.4). A changeset the verified
/// principal makes as a service or an agent that edits the spec tree must name, in its
/// `Proposal:` trailers, approved proposals of its repository that the caller can see in the
/// repository graph. Whether the submitter is the loop comes from the verified claims only.
/// A person's changeset, and one that touches no spec file, are not read against the graph.
pub(super) async fn check_spec_edit_provenance(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    req_id: u64,
    repo: &str,
    touched: &[&str],
    message: &str,
) -> Result<(), String> {
    let loop_authored = verified.is_loop_principal();
    if !loop_authored || !medulla_sdlc::lifecycle::proposals::touches_spec_tree(touched.iter().copied())
    {
        return Ok(());
    }
    let named = medulla_source_types::lifecycle::proposal_trailers(message);
    let target = SdlcTarget {
        graph: repo,
        req_id,
        caller: None,
        verified,
    };
    // A named proposal is read in the repository graph; an unnamed one needs no read.
    let stored = if named.is_empty() {
        Vec::new()
    } else {
        visible_proposals(state, &target).await?
    };
    let bindings: Vec<medulla_sdlc::lifecycle::proposals::StoredBinding<'_>> = stored
        .iter()
        .map(|held| medulla_sdlc::lifecycle::proposals::StoredBinding {
            id: held.entry.id.as_str(),
            project: held.project.as_str(),
            state: held.entry.state,
        })
        .collect();
    medulla_sdlc::lifecycle::proposals::check_changeset_provenance(
        loop_authored,
        repo,
        touched,
        &named,
        &bindings,
    )
    .map_err(|refusal| refusal.to_string())
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
///
/// An approval then opens the pull request (MEDULLA-SDLC-R017.8): the accept is already
/// committed, the `open_pull_request` tool of the named SCM connector runs through the connector
/// gateway under the caller's identity, and the outcome is recorded on the proposal and answered
/// on the receipt. A failure never undoes the accept; it is the receipt's typed
/// `pull_request_error`.
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
        if let Some(pull_request) = &request.pull_request {
            check_pull_request_target(pull_request).map_err(|e| e.to_string())?;
        }
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
        let mut receipt = ProposalDecideReceipt {
            id: request.id.clone(),
            state: next,
            pull_request: None,
            pull_request_error: None,
        };
        if let (ProposalDecision::Approve, Some(pull_request)) =
            (request.decision, &request.pull_request)
        {
            let opened = match repository_connection(state, &target, &stored.project).await {
                Ok(connection) => {
                    open_pull_request(&target, stored, &request.id, &connection, pull_request)
                        .await
                }
                Err(error) => Err(error),
            };
            match opened {
                Ok(address) => receipt.pull_request = Some(address),
                Err(error) => receipt.pull_request_error = Some(error),
            }
            if let Err(why) = record_pull_request(state, &target, &receipt).await {
                receipt.pull_request_error = Some(PullRequestError {
                    code: PULL_REQUEST_RECORD_FAILED.to_owned(),
                    message: why,
                });
            }
        }
        Ok(receipt)
    }
    .await;
    typed_response::<ProposalDecideResult>(target.req_id, result)
}

/// Read the registered source connection of the proposal's repository from its `Repository`
/// node (MEDULLA-SDLC-R017.8). The engine asks the graph, never the caller. No visible node of
/// the tenant for the repository, or a node without a server and repo, is the typed
/// `SOURCE_CONNECTION_UNKNOWN`; a graph that cannot be read is `SOURCE_CONNECTION_READ_FAILED`.
async fn repository_connection(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    repository: &str,
) -> Result<SourceConnection, PullRequestError> {
    let tenant = target.verified.tenant();
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )
        .map_err(|why| PullRequestError {
            code: "SOURCE_CONNECTION_READ_FAILED".to_owned(),
            message: why,
        })?
    };
    let node = core
        .get_nodes_by_label(REPOSITORY_NODE_TYPE, MAX_PROPOSALS_READ)
        .into_iter()
        .filter(|(node_id, properties)| {
            authority.can_see_node(properties, core.is_schema_node(node_id))
        })
        .filter_map(|(_, properties)| {
            medulla_types::msgpack::decode_property_value(&properties).ok()
        })
        .find(|node| {
            node.get("name").and_then(Value::as_str) == Some(repository)
                && node.get("tenant_id").and_then(Value::as_str) == Some(tenant)
        });
    resolve_source_connection(repository, node.as_ref())
}

/// The code of an outcome that could not be written back to the proposal.
const PULL_REQUEST_RECORD_FAILED: &str = "PULL_REQUEST_RECORD_FAILED";

/// Open the pull request of one approved proposal through the connector gateway. The call
/// carries the proposal id as its idempotency key, so a repeated approval never opens two.
async fn open_pull_request(
    target: &SdlcTarget<'_>,
    stored: &StoredProposal,
    proposal_id: &str,
    connection: &SourceConnection,
    pull_request: &PullRequestTarget,
) -> Result<String, PullRequestError> {
    let kind = serde_json::to_value(stored.entry.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let arguments = pull_request_arguments(
        connection,
        pull_request,
        proposal_id,
        &kind,
        &stored.entry.target,
        &stored.statement,
    );
    let reply = run_connector_tool(
        connection.server.clone(),
        OPEN_PULL_REQUEST_TOOL.to_owned(),
        arguments,
        target.verified.agent_id().to_owned(),
        proposal_id.to_owned(),
    )
    .await;
    pull_request_outcome(reply)
}

/// Run one connector tool call on a blocking thread; a failure is `(code, message)`.
#[cfg(feature = "query")]
async fn run_connector_tool(
    server: String,
    tool: String,
    arguments: std::collections::BTreeMap<String, Value>,
    principal: String,
    idempotency_key: String,
) -> Result<Value, (String, String)> {
    use medulla_h_query::query::connector_tool::{call_connector_tool, ConnectorToolRequest};
    let call = ConnectorToolRequest {
        server,
        tool,
        arguments,
        principal,
        idempotency_key,
    };
    match tokio::task::spawn_blocking(move || call_connector_tool(&call)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(failure)) => Err((failure.code, failure.message)),
        Err(_) => Err((
            "CONNECTOR_UNREACHABLE".to_owned(),
            "the connector call did not finish".to_owned(),
        )),
    }
}

/// Run one connector tool call: this build has no connector gateway.
#[cfg(not(feature = "query"))]
async fn run_connector_tool(
    _server: String,
    _tool: String,
    _arguments: std::collections::BTreeMap<String, Value>,
    _principal: String,
    _idempotency_key: String,
) -> Result<Value, (String, String)> {
    Err((
        "CONNECTOR_PORT_UNAVAILABLE".to_owned(),
        "this build has no connector gateway".to_owned(),
    ))
}

/// Write the pull request outcome of an approval onto its proposal: the address, or the typed
/// error, and null for the other, so a later outcome replaces an earlier one.
async fn record_pull_request(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    receipt: &ProposalDecideReceipt,
) -> Result<(), String> {
    let mut properties = Map::new();
    properties.insert("type".into(), PROPOSAL_TYPE.into());
    properties.insert("tenant_id".into(), target.verified.tenant().into());
    let text = |value: Option<&String>| value.map_or(Value::Null, |text| text.clone().into());
    properties.insert("pull_request".into(), text(receipt.pull_request.as_ref()));
    let error = receipt.pull_request_error.as_ref();
    properties.insert("pull_request_error_code".into(), text(error.map(|e| &e.code)));
    properties.insert(
        "pull_request_error_message".into(),
        text(error.map(|e| &e.message)),
    );
    let operations = bounded(vec![
        json!({"op": "upsert_node", "id": receipt.id, "properties": properties}),
    ])?;
    commit(state, target, operations).await
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

/// `ProjectModeGet` (MEDULLA-SDLC-R012.9): the typed read of one project's mode
/// record. A project with no visible record of the verified tenant answers
/// `reference` with `recorded: false`. Reads only.
async fn project_mode_get(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: ProjectModeGetRequest,
) -> Response {
    let result = async {
        let tenant = target.verified.tenant();
        if tenant.trim().is_empty() {
            return Err("TENANT_SCOPE_REQUIRED".to_string());
        }
        let request = handle_project_mode_get(request).map_err(|e| e.to_string())?;
        let stored = stored_project_mode_node(state, &target, &request.project).await?;
        Ok(project_mode_record(&request.project, stored.as_ref()))
    }
    .await;
    typed_response::<ProjectModeGetResult>(target.req_id, result)
}

/// The stored mode of `project` in the request graph, `reference` when no record
/// of the verified tenant exists.
async fn stored_project_mode(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    project: &str,
) -> Result<ProjectMode, String> {
    let stored = stored_project_mode_node(state, target, project).await?;
    Ok(project_mode_record(project, stored.as_ref()).mode)
}

/// The decoded `ProjectMode` node of `project` that the verified tenant owns and
/// the caller may see; `None` for an absent, foreign or hidden record, so a read
/// is no existence oracle.
async fn stored_project_mode_node(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    project: &str,
) -> Result<Option<Value>, String> {
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
        return Ok(None);
    };
    if !authority.can_see_node(&properties, core.is_schema_node(&node_id)) {
        return Ok(None);
    }
    let Ok(node) = medulla_types::msgpack::decode_property_value(&properties) else {
        return Ok(None);
    };
    if node.get("tenant_id").and_then(Value::as_str) != Some(target.verified.tenant()) {
        return Ok(None);
    }
    Ok(Some(node))
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
