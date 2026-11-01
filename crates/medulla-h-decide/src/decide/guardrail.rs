//! Advisory guardrail entailment from one tenant-bound Agent Library snapshot.
//! The owning policy gate remains the authority for enforcement.

use super::SharedState;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::auth::VerifiedRequestContext;
#[cfg(feature = "redb")]
use medulla_persistence::agent_library::AgentLibraryStateExt;
use medulla_agent_types::agent_guardrail::GuardrailEntailmentRequest;

#[cfg(feature = "decide")]
pub async fn handle_guardrail_entailment(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: GuardrailEntailmentRequest,
) -> Response {
    match entailment_for(state, verified, &request).await {
        Ok(result) => Response::ok(
            req_id,
            medulla_protocol_types::protocol::ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::GuardrailEntailment,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "decide")]
async fn entailment_for(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    request: &GuardrailEntailmentRequest,
) -> Result<medulla_agent_types::agent_guardrail::GuardrailEntailmentResult, String> {
    use medulla_agent_types::agent_guardrail::guardrail_entailment_query;
    use medulla_agent_types::agent_guardrail::GuardrailEntailmentResult;
    use medulla_agent_types::agent_guardrail::GUARDRAIL_ENTAILMENT_RESULT_SCHEMA_VERSION;

    if request.tenant_id != verified.tenant() {
        return Err(
            "ACCESS_DENIED: guardrail tenant must match the verified request tenant".into(),
        );
    }
    request.validate()?;
    let store = state.write().await.ensure_agent_library()?;
    let rules = store.published_guardrail_rules(verified.tenant())?;
    let rules = guardrail_entailment_query(&request.task_iri, &rules)?;
    Ok(GuardrailEntailmentResult {
        schema_version: GUARDRAIL_ENTAILMENT_RESULT_SCHEMA_VERSION,
        ontology_digest: medulla_agent_types::agent_ontology::ontology_digest(),
        task_iri: request.task_iri.clone(),
        rules,
    })
}

#[cfg(not(feature = "decide"))]
pub async fn handle_guardrail_entailment(
    _state: &SharedState,
    req_id: u64,
    _verified: &VerifiedRequestContext,
    _request: GuardrailEntailmentRequest,
) -> Response {
    Response::err(req_id, "GuardrailEntailment requires the `decide` feature")
}
