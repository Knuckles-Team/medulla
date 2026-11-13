//! The `LoadSet` question (MEDULLA-DECISION-ENGINE-R176.3): the request's one
//! typed parameter, and the server-side read of the local agents of the
//! question's graph the family merges with the capability designations.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_outcome_types::decision::load_set::AgentCandidate;
use medulla_decision_types::decision::capability_designation::SchemaProjection;
use medulla_decision_types::decision::load_set::{LoadSetQuestion, LOAD_SET_PARAM};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, TypedValue,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::stat_designation::{schema_closure, shared_capability_index, DesignationRuntime};
use super::stat_support::invalid_parameter;
use super::SharedState;

/// `:hasPrompt`: the prompt an agent starts with.
const HAS_PROMPT: &str = medulla_types::ontology_iri!("core#hasPrompt");
/// `:usesModel`: the model profile an agent runs on.
const USES_MODEL: &str = medulla_types::ontology_iri!("core#usesModel");
/// `:hasSkill`: a skill an agent holds.
const HAS_SKILL: &str = medulla_types::ontology_iri!("core#hasSkill");
/// `:orchestrationUsesTool`: a tool an agent uses.
const USES_TOOL: &str = medulla_types::ontology_iri!("core#orchestrationUsesTool");

/// What a load-set question reads beside the capability index.
#[derive(Clone)]
pub(super) struct LoadSetRead {
    /// Agents of the question's graph that name a prompt and a model.
    pub(super) agents: Vec<AgentCandidate>,
    /// The graph revision the agents were read at.
    pub(super) graph_revision: u64,
}

/// The typed question of a load-set request, or why the request is not one.
pub(super) fn question_of(request: &DecideRequest) -> Result<&LoadSetQuestion, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "load_set needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    let TypedValue::LoadSet(question) = &param.value else {
        return Err(invalid_parameter(
            "load_set requires a typed load-set question",
        ));
    };
    if param.name != LOAD_SET_PARAM {
        return Err(invalid_parameter(format!(
            "the load-set parameter must be named {LOAD_SET_PARAM}"
        )));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "load_set declares exactly one candidate: the question id",
        ));
    };
    let [option] = options.as_slice() else {
        return Err(invalid_parameter(
            "load_set declares exactly one candidate: the question id",
        ));
    };
    if option.option_id != request.question.question_id
        || !option.classification.is_empty()
        || !option.numbers.is_empty()
        || !option.texts.is_empty()
    {
        return Err(invalid_parameter(
            "the declared candidate must be the question id without facts",
        ));
    }
    Ok(question)
}

fn unkey(node: &str) -> String {
    node.trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// The agents of one graph: every subject that names a prompt and a model,
/// with the tools and skills it holds, and the graph revision they were read
/// at. The graph is read behind the ordinary read ACL.
async fn read_agents(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    graph: &str,
) -> Result<LoadSetRead, String> {
    let core = super::topology_read::graph_core(state, graph, verified).await?;
    let name = graph.to_string();
    tokio::task::spawn_blocking(move || {
        // The revision is read before the triples: a write between the two
        // makes the answer older than its stamp, never newer.
        let graph_revision = core.version();
        let triples = medulla_rdf::mapping::export_triples(&core, &name)?;
        let mut held: BTreeMap<String, [BTreeSet<String>; 4]> = BTreeMap::new();
        for triple in &triples {
            let slot = match triple.predicate.as_str() {
                HAS_PROMPT => 0,
                USES_MODEL => 1,
                HAS_SKILL => 2,
                USES_TOOL => 3,
                _ => continue,
            };
            let medulla_rdf::oxrdf::NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                continue;
            };
            let medulla_rdf::oxrdf::Term::NamedNode(object) = &triple.object else {
                continue;
            };
            held.entry(unkey(subject.as_str())).or_default()[slot].insert(unkey(object.as_str()));
        }
        let agents = held
            .into_iter()
            .filter_map(|(agent_iri, [prompts, models, skills, tools])| {
                // An agent that names no prompt or no model cannot be started.
                Some(AgentCandidate {
                    agent_iri,
                    prompt_iri: prompts.into_iter().next()?,
                    model_profile: models.into_iter().next()?,
                    tools,
                    skills,
                })
            })
            .collect();
        Ok::<_, String>(LoadSetRead {
            agents,
            graph_revision,
        })
    })
    .await
    .map_err(|error| format!("load set agent read task failed: {error}"))?
}

/// Load the index handle, the schema closure and the agents of the question.
/// A schema that cannot be read is recorded in the runtime: the family then
/// refuses a question with required capabilities.
pub(super) async fn with_load_set_runtime(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    request: &DecideRequest,
) -> Result<DesignationRuntime, String> {
    let question = question_of(request)?;
    let index = shared_capability_index(&*state.read().await);
    let load_set = read_agents(state, verified, &question.graph).await?;
    let (projection, schema_digest, schema_error) =
        match schema_closure(state, verified, &question.graph).await {
            Ok((projection, digest)) => (projection, digest, None),
            Err(detail) => (
                SchemaProjection::default(),
                String::new(),
                Some(format!(
                    "the schema closure of graph {} is unavailable: {detail}",
                    question.graph
                )),
            ),
        };
    Ok(DesignationRuntime {
        index,
        projection,
        schema_digest,
        schema_error,
        load_set: Some(load_set),
        usage_guidance: None,
    })
}
