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

    if request.tenant_id != verified.tenant() {
        return Err("ACCESS_DENIED: assembly tenant must match the verified request tenant".into());
    }
    let store = state.write().await.ensure_agent_library()?;
    let policy = resolve_policy(&store, &request)?;
    let entries = store.assembly_candidates(&request.tenant_id, &request.candidates)?;
    let candidates = medulla_persistence_records::decision_record::candidate_facts(&entries)?;
    let templates = request
        .templates
        .iter()
        .map(|reference| store.assembly_template(&request.tenant_id, reference))
        .collect::<Result<Vec<_>, String>>()?;
    let required_media = request.requirements.required_input_media.clone();
    let topology = super::topology_read::read(state, graph, verified, &request, &templates).await?;
    let mut inputs =
        inputs(request, candidates, templates, policy).map_err(|error| error.to_string())?;
    inputs.topology = topology;
    let identity = RecordIdentity {
        tenant_id: verified.tenant().to_string(),
        caller_principal: verified.principal_persistence_id(),
        created_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    };
    // Bounded by the node budget, but CPU work all the same.
    let assembly = tokio::task::spawn_blocking(move || assemble(inputs, identity))
        .await
        .map_err(|error| format!("assembly task failed: {error}"))?
        .map_err(|error| error.to_string())?;
    let (model_revision, model_input) =
        selected_model(&assembly.record, &entries, required_media.as_slice())?;
    Ok(medulla_decision_types::decision::AssemblyResult {
        schema_version: medulla_decision_types::decision::ASSEMBLY_RESULT_SCHEMA_VERSION,
        record: assembly.record,
        graph: assembly.graph,
        agents: medulla_types::contract::BoundedVec::new(assembly.agents)
            .map_err(|error| error.to_string())?,
        model: assembly.model,
        model_revision,
        model_input,
    })
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
        .strip_prefix(medulla_types::solve_scalar::DIGEST_TEXT_PREFIX)
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
