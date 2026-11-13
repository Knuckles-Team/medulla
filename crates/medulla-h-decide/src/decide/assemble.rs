//! `Method::AgentAssemble`: prove an agent graph out of one tenant's library.
//!
//! Reads the tenant-bound candidate scope from one Agent Library snapshot
//! (step 1a), hands the complete inputs to the pure decision function
//! (`medulla_compute::assemble`), and answers the sealed record plus -- only when
//! it is `Solved` -- the one-agent graph, the agent it runs and the model the
//! certificate covers. Nothing here commits; the record becomes durable only
//! through `DecisionCommit`.

use super::SharedState;
use medulla_decision_types::decision::AssemblyRequest;
#[cfg(feature = "redb")]
#[cfg(feature = "decide")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// The audit target every Decide-layer decision line is written under.
#[cfg(feature = "decide")]
pub(crate) const DECIDE_AUDIT_TARGET: &str = "eg::decide::audit";

/// Read the candidate scope, solve, and answer a record plus -- only when the
/// outcome is `Solved` -- the graph draft it proves.
#[cfg(feature = "decide")]
pub async fn handle_agent_assemble(
    state: &SharedState,
    req_id: u64,
    (graph, verified): (&str, &VerifiedRequestContext),
    request: AssemblyRequest,
) -> Response {
    match assemble_for(state, graph, verified, request).await {
        Ok(result) => {
            audit_line("agent-assemble", &result.record, verified);
            Response::ok(
                req_id,
                medulla_protocol_types::protocol::ResultPayload::of_ref::<
                    medulla_protocol_types::result_contract::storage::AgentAssemble,
                >(&result),
            )
        }
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "decide")]
async fn assemble_for(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    request: AssemblyRequest,
) -> Result<medulla_decision_types::decision::AssemblyResult, String> {
    use medulla_compute::assemble::{assemble, inputs, RecordIdentity};

    use super::telemetry::{self, Stage};
    const FAMILY: &str = "assembly";
    let tenant = verified.tenant();

    let (store, policy) = telemetry::stage_async(Stage::Admit, FAMILY, tenant, async {
        if request.tenant_id != verified.tenant() {
            return Err(
                "ACCESS_DENIED: assembly tenant must match the verified request tenant".to_string(),
            );
        }
        super::family::assemble::admit_graph_size(
            &request,
            medulla_server_api::knob_config::knobs().primitive_max_edges,
        )?;
        let store = state.write().await.ensure_agent_library()?;
        let policy = resolve_policy(&store, &request)?;
        Ok::<_, String>((store, policy))
    })
    .await?;
    let entries = telemetry::stage(Stage::Candidates, FAMILY, tenant, || {
        store.assembly_candidates(&request.tenant_id, &request.candidates)
    })?;
    telemetry::stage_async(
        Stage::Visibility,
        FAMILY,
        tenant,
        admit_tool_set(state, graph, verified, &entries),
    )
    .await?;
    // The profile narrows the candidates only AFTER the tool-set admission
    // above ran over the whole scope: it never bypasses admission and never
    // adds a grant (MEDULLA-DECISION-ENGINE-R036.107).
    let profile = read_profile(state, graph, verified, &request).await?;
    let (entries, profile_constraint) = match &profile {
        Some(profile) => {
            let (kept, constraint) = restrict_to_allowlist(entries, profile)?;
            (kept, Some(constraint))
        }
        None => (entries, None),
    };
    let (candidates, templates) = telemetry::stage(Stage::Constraints, FAMILY, tenant, || {
        let candidates = medulla_persistence_records::decision_record::candidate_facts(&entries)?;
        let templates = request
            .templates
            .iter()
            .map(|reference| store.assembly_template(&request.tenant_id, reference))
            .collect::<Result<Vec<_>, String>>()?;
        Ok::<_, String>((candidates, templates))
    })?;
    let required_media = request.requirements.required_input_media.clone();
    let coding_agent = request.coding_agent.clone();
    let receipt_age_limit_s = coding_agent
        .as_ref()
        .map(|_| medulla_server_api::knob_config::knobs().coding_agent_max_receipt_age);
    let topology = telemetry::stage_async(
        Stage::Entailment,
        FAMILY,
        tenant,
        super::topology_read::read(
            state,
            graph,
            verified,
            request.requirements.topology.as_ref(),
            &templates,
        ),
    )
    .await?;
    let mut inputs =
        inputs(request, candidates, templates, policy).map_err(|error| assembly_refusal(&error))?;
    inputs.topology = topology;
    inputs.profile = profile_constraint;
    inputs.receipt_age_limit_s = receipt_age_limit_s;
    let identity = RecordIdentity {
        tenant_id: verified.tenant().to_string(),
        caller_principal: verified.principal_persistence_id(),
        created_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    };
    // Bounded by the node budget, but CPU work all the same.
    let assembly = telemetry::stage_async(Stage::Solve, FAMILY, tenant, async {
        tokio::task::spawn_blocking(move || assemble(inputs, identity))
            .await
            .map_err(|error| format!("assembly task failed: {error}"))?
            .map_err(|error| assembly_refusal(&error))
    })
    .await?;
    let (model_revision, model_input) = telemetry::stage(Stage::Rank, FAMILY, tenant, || {
        selected_model(&assembly.record, &entries, required_media.as_slice())
    })?;
    let coding_agent_plan =
        coding_agent_plan(&assembly.record, coding_agent.as_ref(), receipt_age_limit_s);
    let result = telemetry::stage(Stage::Record, FAMILY, tenant, || {
        Ok::<_, String>(medulla_decision_types::decision::AssemblyResult {
            schema_version: medulla_decision_types::decision::ASSEMBLY_RESULT_SCHEMA_VERSION,
            record: assembly.record,
            graph: assembly.graph,
            agents: medulla_types::contract::BoundedVec::new(assembly.agents)
                .map_err(|error| error.to_string())?,
            model: assembly.model,
            model_revision,
            model_input,
            coding_agent_plan,
        })
    })?;
    telemetry::stage(Stage::Conclude, FAMILY, tenant, || {
        telemetry::assembled(&result.record);
        Ok::<_, String>(())
    })?;
    Ok(result)
}

/// The profile the request names, read as the verified caller: the owner or a
/// role with a read share. A profile the caller may not read (or another
/// tenant's) is refused with `PROFILE_NOT_VISIBLE` before anything narrows.
#[cfg(feature = "decide")]
async fn read_profile(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    request: &AssemblyRequest,
) -> Result<Option<medulla_types::assistant_profile::AssistantProfile>, String> {
    let Some(profile_id) = request.assistant_profile_ref.as_deref() else {
        return Ok(None);
    };
    let core = super::topology_read::graph_core(state, graph, verified).await?;
    let s = state.read().await;
    medulla_h_graph::graph_ops::assistant_profile::read_visible_profile(
        &core,
        &s.isolation,
        verified.tenant(),
        verified.agent_id(),
        profile_id,
    )
    .map(Some)
}

/// Remove every tool candidate outside the profile's `tool_allowlist`
/// (MEDULLA-DECISION-ENGINE-R036.107). A tool is a candidate of the `Tool`
/// primitive class; prompts, models, ontologies and the rest are not tools and
/// stay. The constraint records what was removed and which capabilities those
/// tools declared, so the pure decision reports an uncoverable requirement
/// under `PROFILE_TOOL_NOT_ALLOWED`.
#[cfg(feature = "decide")]
pub(crate) fn restrict_to_allowlist(
    entries: Vec<medulla_agent_types::agent_component::AgentComponentEntry>,
    profile: &medulla_types::assistant_profile::AssistantProfile,
) -> Result<
    (
        Vec<medulla_agent_types::agent_component::AgentComponentEntry>,
        medulla_decision_types::decision::coding_agent::ProfileConstraint,
    ),
    String,
> {
    use medulla_decision_types::decision::primitive::PrimitiveKind;
    use std::collections::BTreeSet;
    let allowed: BTreeSet<&str> = profile.tool_allowlist.iter().map(String::as_str).collect();
    let mut removed = Vec::new();
    let mut blocked = BTreeSet::new();
    let kept = entries
        .into_iter()
        .filter(|entry| {
            let is_tool = PrimitiveKind::of(entry.kind) == Some(PrimitiveKind::Tool);
            if !is_tool || allowed.contains(entry.component_id.as_str()) {
                return true;
            }
            removed.push(entry.component_id.clone());
            blocked.extend(entry.declared_capabilities.iter().cloned());
            false
        })
        .collect();
    let bounded = |values: Vec<String>, what: &str| {
        medulla_types::contract::BoundedVec::new(values)
            .map_err(|error| format!("ASSEMBLY_INPUTS_INVALID: profile {what}: {error}"))
    };
    Ok((
        kept,
        medulla_decision_types::decision::coding_agent::ProfileConstraint {
            profile_id: profile.profile_id.clone(),
            revision: profile.revision,
            removed: bounded(removed, "removed tools")?,
            blocked_capabilities: bounded(blocked.into_iter().collect(), "blocked capabilities")?,
        },
    ))
}

/// The composed coding-agent plan of a `Solved` record whose request asked for
/// one; the pure decision already refused anything that does not compose.
#[cfg(feature = "decide")]
fn coding_agent_plan(
    record: &medulla_decision_types::decision::DecisionRecord,
    request: Option<&medulla_decision_types::decision::coding_agent::CodingAgentRequest>,
    limit_s: Option<u64>,
) -> Option<medulla_decision_types::decision::coding_agent::CodingAgentPlan> {
    if !matches!(
        record.outcome,
        medulla_decision_types::decision::DecisionOutcome::Solved { .. }
    ) {
        return None;
    }
    medulla_compute::assemble::coding_agent::compose_coding_agent_plan(request?, limit_s?).ok()
}

/// The wire text of an assembly refusal, folded under the `Assemble` origin
/// (MEDULLA-DECISION-ENGINE-R010.100.2): `CODE: detail`.
#[cfg(feature = "decide")]
pub(crate) fn assembly_refusal(error: &medulla_compute::assemble::AssembleError) -> String {
    use medulla_decision_types::decision::record::outcome::{AbstainReason, RefusalOrigin};
    let folded = AbstainReason::from_refusal(RefusalOrigin::ASSEMBLE, error);
    format!("{}: {}", folded.code().as_str(), error.detail)
}

/// The selected model slot's revision and declared input capabilities
/// (MEDULLA-DECISION-ENGINE-R815). Both are `None` unless the record is
/// `Solved`; a required media type the profile does not declare (or a profile
/// that declares nothing) is refused with `MODEL_MEDIA_CAPABILITY_UNKNOWN`.
#[cfg(feature = "decide")]
pub(crate) fn selected_model(
    record: &medulla_decision_types::decision::DecisionRecord,
    entries: &[medulla_agent_types::agent_component::AgentComponentEntry],
    required: &[medulla_agent_types::agent_component::ModelInputMediaType],
) -> Result<
    (
        Option<String>,
        Option<medulla_agent_types::agent_component::ModelInputCapabilities>,
    ),
    String,
> {
    use medulla_agent_types::agent_component::{AgentComponentFacts, AgentComponentKind};
    use medulla_decision_types::decision::{DecisionErrorCode, DecisionOutcome};

    let DecisionOutcome::Solved { slots, .. } = &record.outcome else {
        return Ok((None, None));
    };
    let Some(slot) = slots
        .iter()
        .find(|slot| slot.component.kind == AgentComponentKind::ModelProfile)
    else {
        return Ok((None, None));
    };
    let revision = slot
        .component
        .definition_digest
        .strip_prefix(medulla_query_types::solve_scalar::DIGEST_TEXT_PREFIX)
        .unwrap_or(&slot.component.definition_digest)
        .to_string();
    let declared = entries
        .iter()
        .find(|entry| entry.component_id == slot.component.component_id)
        .and_then(|entry| match &entry.facts {
            AgentComponentFacts::ModelProfile {
                input_capabilities, ..
            } => input_capabilities.clone(),
            _ => None,
        });
    if let Some(missing) = required.iter().find(|media| {
        declared
            .as_ref()
            .is_none_or(|caps| !caps.media_types.contains(media))
    }) {
        return Err(format!(
            "{}: model {} does not declare input media {}",
            DecisionErrorCode::ModelMediaCapabilityUnknown.as_str(),
            slot.component.component_id,
            missing.as_str()
        ));
    }
    Ok((Some(revision), declared))
}

/// The plan's tool set: the component identifier of every candidate entry.
#[cfg(feature = "decide")]
pub(crate) fn plan_tool_iris(
    entries: &[medulla_agent_types::agent_component::AgentComponentEntry],
) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry.component_id.clone())
        .collect()
}

/// Tool-set admission (MEDULLA-IDENTITY-R008.1): the candidate scope must lie
/// inside the principal's STORED `:RunScopePolicy` bound in the verified
/// tenant's graph. A refusal is audited before it returns (R008.5).
#[cfg(feature = "decide")]
pub(crate) async fn admit_tool_set(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    entries: &[medulla_agent_types::agent_component::AgentComponentEntry],
) -> Result<(), String> {
    let core = super::topology_read::graph_core(state, graph, verified).await?;
    let principal = verified.principal_persistence_id();
    match medulla_isolation::isolation::enforce_scope(&core, &principal, &plan_tool_iris(entries)) {
        Ok(_) => Ok(()),
        Err(refusal) => {
            audit_scope_refusal(state, graph, verified, &refusal).await?;
            Err(refusal.to_string())
        }
    }
}

/// Write the `scope_exceeds_policy` audit-chain entry of a refusal: actor,
/// tenant, and (digested in the canonical line) the tool and policy IRIs.
#[cfg(feature = "decide")]
async fn audit_scope_refusal(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    refusal: &medulla_isolation::isolation::ScopeRefusal,
) -> Result<(), String> {
    let line = medulla_isolation::isolation::scope_refusal_audit_line(refusal, verified.tenant());
    tracing::warn!(
        target: DECIDE_AUDIT_TARGET,
        action = "agent-assemble",
        principal = %verified.principal_persistence_id(),
        tenant = %verified.tenant(),
        refusal = %line,
        "scope exceeds policy"
    );
    #[cfg(feature = "security")]
    {
        use medulla_persistence::AsRedb;
        use sha2::Digest;
        let persistence = state.read().await.persistence.clone();
        if let Some(redb) = persistence.as_ref().and_then(|p| p.as_redb()) {
            let (subject_iri, policy_iri) = match refusal {
                medulla_isolation::isolation::ScopeRefusal::ExceedsPolicy {
                    tool_iri,
                    policy_iri,
                } => (Some(tool_iri.clone()), Some(policy_iri.clone())),
                medulla_isolation::isolation::ScopeRefusal::Unreadable(_) => (None, None),
            };
            let event = medulla_store::redb_store::OperationAuditEvent {
                tenant: verified.tenant().to_string(),
                principal: verified.principal_persistence_id(),
                op: "AgentAssemble".to_string(),
                surface: "engine".to_string(),
                params_sha256: hex::encode(sha2::Sha256::digest(line.as_bytes())),
                status: "denied".to_string(),
                request_id: hex::encode(sha2::Sha256::digest(
                    verified.idempotency_key().as_bytes(),
                )),
                audit_class: medulla_isolation::isolation::SCOPE_EXCEEDS_POLICY_AUDIT_CLASS
                    .to_string(),
                subject_iri,
                policy_iri,
            };
            redb.audit_append(&medulla_kernel::persist::sanitize(graph), event)
                .await?;
        }
    }
    #[cfg(not(feature = "security"))]
    let _ = (state, graph);
    Ok(())
}

/// The policy a request decides under: the engine default, or the verified
/// body of the published `DecisionPolicy` component revision it pins.
#[cfg(feature = "decide")]
fn resolve_policy(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    request: &AssemblyRequest,
) -> Result<medulla_decision_types::decision::DecisionPolicy, String> {
    use medulla_decision_types::{decision::DecisionPolicy, decision::DecisionPolicyRef};
    match &request.policy {
        DecisionPolicyRef::Default => Ok(DecisionPolicy::engine_default()),
        DecisionPolicyRef::Pinned { component } => {
            store.pinned_decision_policy(&request.tenant_id, component)
        }
    }
}

/// One structured audit line per decision: who asked, what was decided, and
/// how far it can be trusted. Record ids only -- never inputs or text.
#[cfg(feature = "decide")]
pub(crate) fn audit_line(
    action: &'static str,
    record: &medulla_decision_types::decision::DecisionRecord,
    verified: &VerifiedRequestContext,
) {
    let outcome = match &record.outcome {
        medulla_decision_types::decision::DecisionOutcome::Solved { .. } => "solved",
        medulla_decision_types::decision::DecisionOutcome::Abstained { .. } => "abstained",
    };
    tracing::info!(
        target: DECIDE_AUDIT_TARGET,
        action,
        record_id = %record.record_id,
        tenant = %verified.tenant(),
        caller = %verified.principal_persistence_id(),
        outcome,
        resolution_kind = ?record.resolution_kind,
        evidence_class = ?record.evidence_class,
        eliminated = record.eliminated.len(),
        catalog_digest = %record.inputs.catalog_digest,
        "decide-layer decision"
    );
}

/// A build without the Decide layer still serves the method: it refuses by
/// name.
#[cfg(not(feature = "decide"))]
pub async fn handle_agent_assemble(
    _state: &SharedState,
    req_id: u64,
    _caller: (&str, &VerifiedRequestContext),
    _request: AssemblyRequest,
) -> Response {
    Response::err(req_id, "AgentAssemble requires the `decide` feature")
}
