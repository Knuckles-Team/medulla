//! `Method::AgentAssemble`: prove an agent graph out of one tenant's library.
//!
//! Reads the tenant-bound candidate scope from one Agent Library snapshot
//! (step 1a), hands the complete inputs to the pure decision function
//! (`medulla_compute::assemble`), and answers the sealed record plus -- only when
//! it is `Solved` -- the one-agent graph, the agent it runs and the model the
//! certificate covers. Nothing here commits; the record becomes durable only
//! through `DecisionCommit`.

use super::SharedState;
use medulla_decision_types::decision::request::AssemblyRequest;
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

/// Turn `template_key` (MEDULLA-PLACEMENT-R001.12) into the exact pinned reference of the
/// newest published revision of that graph template in the request's tenant, so the solve and
/// the sealed record name one revision. The key is cleared once resolved. A key beside a
/// non-empty `templates`, or an empty key, refuses `ASSEMBLY_INPUTS_INVALID`.
#[cfg(feature = "decide")]
fn resolve_template_key(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    request: &mut AssemblyRequest,
) -> Result<(), String> {
    let Some(key) = request.template_key.take() else {
        return Ok(());
    };
    if key.is_empty() {
        return Err("ASSEMBLY_INPUTS_INVALID: template_key must not be empty".to_string());
    }
    if !request.templates.is_empty() {
        return Err(
            "ASSEMBLY_INPUTS_INVALID: template_key and templates are mutually exclusive"
                .to_string(),
        );
    }
    let reference = store.assembly_template_ref(&request.tenant_id, &key)?;
    request.templates = medulla_types::contract::BoundedVec::new(vec![reference])
        .map_err(|message| format!("ASSEMBLY_INPUTS_INVALID: template_key: {message}"))?;
    Ok(())
}

#[cfg(feature = "decide")]
pub(super) async fn assemble_for(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    request: AssemblyRequest,
) -> Result<medulla_decision_types::decision::request::AssemblyResult, String> {
    use {
        medulla_compute_assemble::assemble::assemble, medulla_compute_assemble::assemble::inputs,
        medulla_compute_assemble::assemble::RecordIdentity,
    };

    use super::telemetry::{self, Stage};
    const FAMILY: &str = "assembly";
    let tenant = verified.tenant();

    let (store, policy, shields) = telemetry::stage_async(Stage::Admit, FAMILY, tenant, async {
        if request.tenant_id != verified.tenant() {
            return Err(
                "ACCESS_DENIED: assembly tenant must match the verified request tenant".to_string(),
            );
        }
        super::family::assemble::admit_graph_size(
            &request,
            medulla_server_api::knob_config::knobs().primitive_max_edges,
        )?;
        medulla_decision_policy_types::decision::evidence_stream::validate(
            request.evidence_streams.as_slice(),
        )
        .map_err(|refused| refused.message())?;
        // The step shield is declared before anything is read: a stale schema sends nothing
        // (MEDULLA-ORCH-VOCAB-R005.100).
        let shields =
            super::assemble_shield::shield_step_schema(request.step_contracts.as_slice())?;
        let store = state.write().await.ensure_agent_library()?;
        let policy = resolve_policy(&store, &request)?;
        Ok::<_, String>((store, policy, shields))
    })
    .await?;
    let mut request = request;
    telemetry::stage(Stage::Candidates, FAMILY, tenant, || {
        resolve_template_key(&store, &mut request)
    })?;
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
    // The one system-prompt slot narrows to the connector prompt that covers the required
    // capabilities (MEDULLA-DECISION-ENGINE-R814.3); the selection is evidence of the record.
    let (entries, prompt_selection) = {
        use medulla_decision_policy_types::decision::knob_families::prompt_candidate::KNOBS;
        use medulla_decision_statistical_types::decision::knobs::KnobValue;
        let min_match_micros = match policy.knob(&KNOBS[0]) {
            KnobValue::DecimalMicros(micros) => micros,
            // The knob is a decimal; a pinned policy of another shape keeps the default 1.0.
            _ => 1_000_000,
        };
        let required: std::collections::BTreeSet<String> = request
            .requirements
            .capabilities
            .as_slice()
            .iter()
            .cloned()
            .collect();
        // A connector whose projection failed ontology admission hides its prompts first
        // (MEDULLA-DECISION-ENGINE-R814.4).
        let (entries, hidden) =
            super::prompt_candidates::exclude_failed_connectors(entries, &store, tenant);
        let (entries, selected) =
            super::prompt_candidates::narrow_system_prompts(entries, &required, min_match_micros);
        let selection = super::prompt_candidates::prompt_selection(
            selected.as_ref(),
            hidden,
            &required,
            &entries,
        );
        (entries, selection)
    };
    let templates = telemetry::stage(Stage::Constraints, FAMILY, tenant, || {
        request
            .templates
            .iter()
            .map(|reference| store.assembly_template(&request.tenant_id, reference))
            .collect::<Result<Vec<_>, String>>()
    })?;
    // Slot agent kinds narrow the candidates before selection (MEDULLA-ORCH-VOCAB-R007.4); the
    // slot source is the topology facts of the request's templates.
    let (entries, candidates) = telemetry::stage(Stage::Constraints, FAMILY, tenant, || {
        let slots: Vec<_> = templates
            .iter()
            .filter_map(|template| template.topology.as_ref())
            .flat_map(|topology| topology.slots.as_slice().iter().cloned())
            .collect();
        let views: Vec<_> = entries
            .iter()
            .map(medulla_decision_stats_records::decision::candidate::CandidateView::from_component)
            .collect();
        let kept = super::candidates::filter_by_slot_kind(&slots, &entries, &views)
            .map_err(|refusal| refusal.to_string())?;
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|entry| kept.iter().any(|view| view.id == entry.component_id))
            .collect();
        let candidates = medulla_persistence_records::decision_record::candidate_facts(&entries)?;
        Ok::<_, String>((entries, candidates))
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
    let steps = telemetry::stage_async(
        Stage::Entailment,
        FAMILY,
        tenant,
        super::steps_read::read(
            state,
            graph,
            verified,
            request.requirements.tasks.as_slice(),
        ),
    )
    .await?;
    super::assemble_shield::check_steps_exist(request.step_contracts.as_slice(), steps.as_slice())?;
    // MEDULLA-ORCH-VOCAB-R800.3: the definitions the request names are read and validated here;
    // the prompts are rendered once the solve is done.
    let prompts = request.prompts.clone();
    let definitions = telemetry::stage_async(
        Stage::Entailment,
        FAMILY,
        tenant,
        super::definitions_read::read_definitions(
            state,
            graph,
            verified,
            request.patterns.as_slice(),
            request.prompts.as_slice(),
        ),
    )
    .await?;
    let mut inputs =
        inputs(request, candidates, templates, policy).map_err(|error| assembly_refusal(&error))?;
    inputs.topology = topology;
    inputs.steps = steps;
    inputs.profile = profile_constraint;
    inputs.prompt_selection = prompt_selection;
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
    let rendered_prompts = render_prompts(prompts.as_slice(), &definitions, assembly.agents.len())?;
    let result = telemetry::stage(Stage::Record, FAMILY, tenant, || {
        Ok::<_, String>(medulla_decision_types::decision::request::AssemblyResult {
            schema_version:
                medulla_decision_types::decision::request::ASSEMBLY_RESULT_SCHEMA_VERSION,
            record: assembly.record,
            graph: assembly.graph,
            agents: medulla_types::contract::BoundedVec::new(assembly.agents)
                .map_err(|error| error.to_string())?,
            model: assembly.model,
            model_revision,
            model_input,
            coding_agent_plan,
            rendered_prompts: medulla_types::contract::BoundedVec::new(rendered_prompts)
                .map_err(|error| error.to_string())?,
            step_shields: medulla_types::contract::BoundedVec::new(shields)
                .map_err(|error| error.to_string())?,
        })
    })?;
    telemetry::stage(Stage::Conclude, FAMILY, tenant, || {
        telemetry::assembled(&result.record);
        Ok::<_, String>(())
    })?;
    Ok(result)
}

/// Render every assigned prompt against the template read for it (MEDULLA-ORCH-VOCAB-R800.3).
/// An `agent_slot` outside the assembled agents refuses `ASSEMBLY_INPUTS_INVALID`; a render
/// refusal returns its `TEMPLATE_SLOT_*` code as `CODE: detail`.
#[cfg(feature = "decide")]
fn render_prompts(
    prompts: &[medulla_decision_types::decision::request::PromptAssignment],
    definitions: &super::definitions_read::ReadDefinitions,
    agents: usize,
) -> Result<Vec<medulla_decision_types::decision::request::RenderedPrompt>, String> {
    use medulla_work_types::orchestration::definitions::{render_digest, render_template};
    prompts
        .iter()
        .zip(&definitions.templates)
        .map(|(assignment, (template, body))| {
            if usize::try_from(assignment.agent_slot).map_or(true, |slot| slot >= agents) {
                return Err(format!(
                    "ASSEMBLY_INPUTS_INVALID: prompts: slot {} is not assembled",
                    assignment.agent_slot
                ));
            }
            let values: Vec<(String, _)> = assignment
                .values
                .as_slice()
                .iter()
                .map(|slot| (slot.name.clone(), slot.value.clone()))
                .collect();
            let text = render_template(template, body, &values).map_err(|refusal| {
                format!(
                    "{}: template {} renders no text",
                    refusal.as_str(),
                    template.id
                )
            })?;
            Ok(medulla_decision_types::decision::request::RenderedPrompt {
                agent_slot: assignment.agent_slot,
                template_id: template.id.clone(),
                template_digest: template.digest.clone(),
                text,
                render_digest: render_digest(&template.digest, &values),
            })
        })
        .collect()
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
) -> Result<Option<medulla_identity_types::assistant_profile::AssistantProfile>, String> {
    let Some(profile_id) = request.assistant_profile_ref.as_deref() else {
        return Ok(None);
    };
    let core = super::topology_read::graph_core(state, graph, verified).await?;
    let s = state.read().await;
    medulla_h_graph_gateway::graph_ops::assistant_profile::read_visible_profile(
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
    profile: &medulla_identity_types::assistant_profile::AssistantProfile,
) -> Result<
    (
        Vec<medulla_agent_types::agent_component::AgentComponentEntry>,
        medulla_decision_program_types::decision::coding_agent::ProfileConstraint,
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
        medulla_decision_program_types::decision::coding_agent::ProfileConstraint {
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
    record: &medulla_decision_types::decision::record::DecisionRecord,
    request: Option<&medulla_decision_program_types::decision::coding_agent::CodingAgentRequest>,
    limit_s: Option<u64>,
) -> Option<medulla_decision_program_types::decision::coding_agent::CodingAgentPlan> {
    if !matches!(
        record.outcome,
        medulla_decision_types::decision::record::DecisionOutcome::Solved { .. }
    ) {
        return None;
    }
    medulla_compute_assemble::assemble::coding_agent::compose_coding_agent_plan(request?, limit_s?)
        .ok()
}

/// The wire text of an assembly refusal, folded under the `Assemble` origin
/// (MEDULLA-DECISION-ENGINE-R010.100.2): `CODE: detail`.
#[cfg(feature = "decide")]
pub(crate) fn assembly_refusal(
    error: &medulla_compute_assemble::assemble::AssembleError,
) -> String {
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
    record: &medulla_decision_types::decision::record::DecisionRecord,
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
    use {
        medulla_decision_statistical_types::decision::DecisionErrorCode,
        medulla_decision_types::decision::record::DecisionOutcome,
    };

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
                status: medulla_store::redb_store::audit_status::AuditStatus::settled(
                    medulla_store::redb_store::audit_machine::AuditEvent::Deny,
                ),
                request_id: hex::encode(sha2::Sha256::digest(
                    verified.idempotency_key().as_bytes(),
                )),
                audit_class: medulla_isolation::isolation::SCOPE_EXCEEDS_POLICY_AUDIT_CLASS
                    .to_string(),
                subject_iri,
                policy_iri,
                evidence: Default::default(),
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
) -> Result<medulla_decision_statistical_types::decision::DecisionPolicy, String> {
    use medulla_agent_types::agent_component::DecisionPolicyRef;
    use medulla_decision_statistical_types::decision::DecisionPolicy;
    match &request.policy {
        DecisionPolicyRef::Default => Ok(DecisionPolicy::engine_default()),
        DecisionPolicyRef::Pinned { component } => {
            store.pinned_decision_policy(&request.tenant_id, component)
        }
        DecisionPolicyRef::Champion { policy_id } => {
            store.champion_decision_policy(&request.tenant_id, policy_id)
        }
    }
}

/// One structured audit line per decision: who asked, what was decided, and
/// how far it can be trusted. Record ids only -- never inputs or text.
#[cfg(feature = "decide")]
pub(crate) fn audit_line(
    action: &'static str,
    record: &medulla_decision_types::decision::record::DecisionRecord,
    verified: &VerifiedRequestContext,
) {
    let outcome = match &record.outcome {
        medulla_decision_types::decision::record::DecisionOutcome::Solved { .. } => "solved",
        medulla_decision_types::decision::record::DecisionOutcome::Abstained { .. } => "abstained",
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
