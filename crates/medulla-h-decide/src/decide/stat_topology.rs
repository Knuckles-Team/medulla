//! The standalone `Topology` question (MEDULLA-DECISION-ENGINE-R111.1): the
//! request's one typed parameter, and the server-side read that fills it.

use medulla_decision_types::decision::statistical::topology::{
    TopologyQuestion, TOPOLOGY_PARAM, TOPOLOGY_SCHEMA_UNAVAILABLE,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, TypedValue,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::stat_support::invalid_parameter;
use super::SharedState;

/// The typed question of a `Topology` request, or why the request is not one.
pub(super) fn question_of(request: &DecideRequest) -> Result<&TopologyQuestion, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "topology needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    let TypedValue::Topology(question) = &param.value else {
        return Err(invalid_parameter(
            "topology requires a typed topology question",
        ));
    };
    if param.name != TOPOLOGY_PARAM {
        return Err(invalid_parameter(
            "the topology parameter must be named topology",
        ));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "topology requires the declared template id",
        ));
    };
    let [option] = options.as_slice() else {
        return Err(invalid_parameter(
            "topology declares exactly one candidate: the template id",
        ));
    };
    if option.option_id != question.template.graph_id
        || !option.classification.is_empty()
        || !option.numbers.is_empty()
        || !option.texts.is_empty()
    {
        return Err(invalid_parameter(
            "the declared candidate must be the template id without facts",
        ));
    }
    Ok(question)
}

/// `CODE: detail` text keeps its own code; anything else is a schema read
/// failure and carries `TOPOLOGY_SCHEMA_UNAVAILABLE`.
fn schema_unavailable(error: String) -> String {
    let coded = error.split_once(':').is_some_and(|(code, _)| {
        !code.is_empty() && code.chars().all(|c| c.is_ascii_uppercase() || c == '_')
    });
    match coded {
        true => error,
        false => format!("{TOPOLOGY_SCHEMA_UNAVAILABLE}: {error}"),
    }
}

/// Read the schema entailments and capacity headroom of the question and put
/// them in its parameter. A caller-supplied `read` is refused: observed
/// headroom is not a caller claim.
pub(super) async fn with_read(
    state: &SharedState,
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    let question = question_of(&request)?.clone();
    if question.read.is_some() {
        return Err(invalid_parameter(
            "the topology read is filled by the engine and cannot be supplied",
        ));
    }
    let template = store.assembly_template(&request.tenant_id, &question.template)?;
    let read = super::topology_read::read(
        state,
        &question.graph,
        verified,
        Some(&question.requirements),
        std::slice::from_ref(&template),
    )
    .await
    .map_err(schema_unavailable)?;
    let mut filled = question;
    filled.read = read;
    let mut params = request.params.as_slice().to_vec();
    params[0].value = TypedValue::Topology(Box::new(filled));
    request.params = medulla_types::contract::BoundedVec::new(params)
        .map_err(|detail| invalid_parameter(detail))?;
    Ok(request)
}
