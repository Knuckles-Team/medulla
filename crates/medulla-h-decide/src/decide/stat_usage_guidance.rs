//! The `UsageGuidance` question (MEDULLA-DECISION-ENGINE-R177.3): the request's
//! typed parameters, and the server-side read of the connector pack facts and
//! the tool outcome counts the family orders the steps with.
//!
//! The pack is read from the question's graph as RDF triples: the connector
//! node's features and the tools, prompts and skills they are realised by
//! (`cp:providesFeature`, `cp:realizedBy`), each tool's and skill's
//! `cp:providesCapability` and `cp:operatesOn`, and the job class's `:hasStep`
//! chain with `:precedes` and `:needsSkill`. Outcome counts come from the
//! decision log of the asking tenant only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_catalog_types::connector_pack::{pack_component_id, PackEntryKind};
use medulla_decision_stats::decision::usage_guidance::ToolRate;
use medulla_decision_types::decision::jobs::RecordWindow;
use medulla_decision_types::decision::statistical::log::OutcomeAggregateRequest;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, TypedValue,
};
use medulla_decision_types::decision::usage_guidance::{
    DEFAULT_USAGE_GUIDANCE_GRAPH, USAGE_GUIDANCE_CONNECTOR_PARAM, USAGE_GUIDANCE_GRAPH_PARAM,
    USAGE_GUIDANCE_JOB_CLASS_PARAM,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::stat_designation::{shared_capability_index, DesignationRuntime};
use super::stat_log::{aggregate_log, LogReader};
use super::stat_support::invalid_parameter;
use super::SharedState;

const CP: &str = medulla_types::ontology_iri!("connector-pack#");
const CORE: &str = medulla_types::ontology_iri!("core#");
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";

/// The typed question of a usage guidance request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UsageQuestion {
    pub(super) connector: String,
    pub(super) job_class: String,
    pub(super) graph: String,
}

/// One object of a triple the pack is read from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Object {
    Iri(String),
    Literal(String),
}

impl Object {
    fn text(&self) -> &str {
        match self {
            Self::Iri(text) | Self::Literal(text) => text,
        }
    }
}

/// One triple of the question's graph, with its terms rendered as text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Fact {
    pub(super) subject: String,
    pub(super) predicate: String,
    pub(super) object: Object,
}

impl Fact {
    /// The citation form `<subject>/<predicate>/<object>`.
    pub(super) fn cite(&self) -> String {
        format!("{}/{}/{}", self.subject, self.predicate, self.object.text())
    }
}

/// A tool of the connector with what the guidance reads of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolFact {
    pub(super) iri: String,
    pub(super) component_id: String,
    pub(super) capabilities: BTreeSet<String>,
    pub(super) operates_on: BTreeSet<String>,
    pub(super) facts: Vec<Fact>,
}

/// A skill the steps need.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct SkillFact {
    pub(super) capabilities: BTreeSet<String>,
    pub(super) text: Option<String>,
    pub(super) facts: Vec<Fact>,
}

/// One `:TaskStep` of the job class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StepFact {
    pub(super) step_id: String,
    pub(super) skill: Option<String>,
    pub(super) precedes: Vec<String>,
    pub(super) facts: Vec<Fact>,
}

/// A prompt of the connector and the classes it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PromptFact {
    pub(super) iri: String,
    pub(super) applies_to: BTreeSet<String>,
}

/// What the pack states about the connector and the job class.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PackFacts {
    pub(super) connector_known: bool,
    pub(super) tools: BTreeMap<String, ToolFact>,
    pub(super) skills: BTreeMap<String, SkillFact>,
    pub(super) prompts: Vec<PromptFact>,
    pub(super) steps: Vec<StepFact>,
}

/// What a usage guidance question reads beside the capability index.
#[derive(Clone)]
pub(super) struct UsageGuidanceRead {
    pub(super) facts: PackFacts,
    /// Pooled outcomes per tool component id, from the asking tenant's log.
    pub(super) rates: BTreeMap<String, ToolRate>,
    pub(super) graph_revision: u64,
}

/// The typed question of a usage guidance request, or why it is not one.
pub(super) fn question_of(request: &DecideRequest) -> Result<UsageQuestion, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
    {
        return Err(invalid_parameter(
            "usage_guidance needs ordinary safety and no head or belief slices",
        ));
    }
    let mut connector = None;
    let mut job_class = None;
    let mut graph = None;
    for param in request.params.as_slice() {
        match (param.name.as_str(), &param.value) {
            (USAGE_GUIDANCE_CONNECTOR_PARAM, TypedValue::Iri(iri)) if connector.is_none() => {
                connector = Some(iri.clone())
            }
            (USAGE_GUIDANCE_JOB_CLASS_PARAM, TypedValue::Iri(iri)) if job_class.is_none() => {
                job_class = Some(iri.clone())
            }
            (USAGE_GUIDANCE_GRAPH_PARAM, TypedValue::Text(name)) if graph.is_none() => {
                graph = Some(name.clone())
            }
            _ => {
                return Err(invalid_parameter(format!(
                    "usage_guidance takes `{USAGE_GUIDANCE_CONNECTOR_PARAM}` and \
                     `{USAGE_GUIDANCE_JOB_CLASS_PARAM}` as IRIs and an optional \
                     `{USAGE_GUIDANCE_GRAPH_PARAM}` as text; got `{}`",
                    param.name
                )))
            }
        }
    }
    let (Some(connector), Some(job_class)) = (connector, job_class) else {
        return Err(invalid_parameter(format!(
            "usage_guidance needs the `{USAGE_GUIDANCE_CONNECTOR_PARAM}` and \
             `{USAGE_GUIDANCE_JOB_CLASS_PARAM}` parameters"
        )));
    };
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "usage_guidance declares exactly one candidate: the question id",
        ));
    };
    let [option] = options.as_slice() else {
        return Err(invalid_parameter(
            "usage_guidance declares exactly one candidate: the question id",
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
    Ok(UsageQuestion {
        connector,
        job_class,
        graph: graph.unwrap_or_else(|| DEFAULT_USAGE_GUIDANCE_GRAPH.to_string()),
    })
}

fn is_predicate(fact: &Fact, namespace: &str, name: &str) -> bool {
    fact.predicate.strip_prefix(namespace) == Some(name)
}

/// The tool name of a `tool://<connector>/<name>` IRI.
fn tool_name(iri: &str) -> Option<(&str, &str)> {
    iri.strip_prefix("tool://")?.split_once('/')
}

/// The pack facts the connector and job class name, read from `triples`.
///
/// A connector is known when its node declares a feature. A step whose skill
/// shares no capability with any tool of the connector has no tool and is left
/// out; `:precedes` edges to a step that is not of the job class are ignored by
/// the ordering.
pub(super) fn facts_of(connector: &str, job_class: &str, triples: &[Fact]) -> PackFacts {
    let objects = |subject: &str, namespace: &str, name: &str| -> Vec<&Fact> {
        triples
            .iter()
            .filter(|fact| fact.subject == subject && is_predicate(fact, namespace, name))
            .collect()
    };
    let iris = |facts: &[&Fact]| -> Vec<String> {
        facts
            .iter()
            .filter_map(|fact| match &fact.object {
                Object::Iri(iri) => Some(iri.clone()),
                Object::Literal(_) => None,
            })
            .collect()
    };
    let feature_links = objects(connector, CP, "providesFeature");
    let mut out = PackFacts {
        connector_known: !feature_links.is_empty(),
        ..PackFacts::default()
    };
    if !out.connector_known {
        return out;
    }
    let mut members: BTreeSet<String> = BTreeSet::new();
    for feature in iris(&feature_links) {
        members.extend(iris(&objects(&feature, CP, "realizedBy")));
    }
    for member in &members {
        let provides = objects(member, CP, "providesCapability");
        let operates = objects(member, CP, "operatesOn");
        let applies = objects(member, CP, "appliesTo");
        if let Some((owner, name)) = tool_name(member) {
            let mut facts: Vec<Fact> = provides
                .iter()
                .chain(operates.iter())
                .map(|fact| (*fact).clone())
                .collect();
            facts.sort();
            out.tools.insert(
                member.clone(),
                ToolFact {
                    iri: member.clone(),
                    component_id: pack_component_id(owner, PackEntryKind::Tool, name),
                    capabilities: iris(&provides).into_iter().collect(),
                    operates_on: iris(&operates).into_iter().collect(),
                    facts,
                },
            );
        } else if member.starts_with("prompt://") {
            out.prompts.push(PromptFact {
                iri: member.clone(),
                applies_to: iris(&applies).into_iter().collect(),
            });
        }
    }
    let steps = iris(&objects(job_class, CORE, "hasStep"));
    let has_step = objects(job_class, CORE, "hasStep");
    for step_id in steps {
        let precedes = objects(&step_id, CORE, "precedes");
        let skill = objects(&step_id, CORE, "needsSkill");
        let mut facts: Vec<Fact> = has_step
            .iter()
            .filter(|fact| fact.object == Object::Iri(step_id.clone()))
            .chain(precedes.iter())
            .chain(skill.iter())
            .map(|fact| (*fact).clone())
            .collect();
        facts.sort();
        let skill_iri = iris(&skill).into_iter().next();
        if let Some(skill_iri) = &skill_iri {
            out.skills.entry(skill_iri.clone()).or_insert_with(|| {
                let provides = objects(skill_iri, CP, "providesCapability");
                let comment = triples.iter().find_map(|fact| match &fact.object {
                    Object::Literal(text)
                        if fact.subject == *skill_iri && fact.predicate == RDFS_COMMENT =>
                    {
                        Some(text.clone())
                    }
                    _ => None,
                });
                SkillFact {
                    capabilities: iris(&provides).into_iter().collect(),
                    text: comment,
                    facts: provides.iter().map(|fact| (*fact).clone()).collect(),
                }
            });
        }
        out.steps.push(StepFact {
            step_id,
            skill: skill_iri,
            precedes: iris(&precedes),
            facts,
        });
    }
    out
}

fn unkey(node: &str) -> String {
    node.trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// The tool of a step: the connector's tool sharing the most capabilities with
/// the step's skill; ties go to the lower component id. A step with no skill or
/// no shared capability has no tool.
pub(super) fn step_tool<'a>(facts: &'a PackFacts, step: &StepFact) -> Option<&'a ToolFact> {
    let skill = facts.skills.get(step.skill.as_ref()?)?;
    facts
        .tools
        .values()
        .map(|tool| {
            (
                tool.capabilities.intersection(&skill.capabilities).count(),
                tool,
            )
        })
        .filter(|(shared, _)| *shared > 0)
        .max_by(|(a, left), (b, right)| {
            a.cmp(b)
                .then_with(|| right.component_id.cmp(&left.component_id))
        })
        .map(|(_, tool)| tool)
}

/// The pooled outcomes per tool component id of the asking tenant.
fn tool_rates(
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
) -> Result<BTreeMap<String, ToolRate>, String> {
    let reader = LogReader::of(verified);
    let aggregate = aggregate_log(
        store,
        &reader,
        &OutcomeAggregateRequest {
            tenant_id: verified.tenant().to_string(),
            question_id: None,
            window: RecordWindow {
                from_ms: 0,
                to_ms: u64::MAX,
            },
            attribution: None,
            transfer: None,
        },
    )?;
    let mut rates: BTreeMap<String, ToolRate> = BTreeMap::new();
    for row in aggregate
        .rows
        .iter()
        .filter(|row| row.question_id.is_none())
    {
        let rate = rates.entry(row.option_id.clone()).or_insert(ToolRate {
            successes: 0,
            trials: 0,
        });
        rate.successes += row.successes;
        rate.trials += row.trials;
    }
    Ok(rates)
}

/// Read the pack facts of the question's graph behind the ordinary read ACL
/// and the tenant's tool outcomes, and load the capability index handle.
pub(super) async fn with_usage_guidance_runtime(
    state: &SharedState,
    store: &Arc<AgentLibraryStore>,
    verified: &VerifiedRequestContext,
    request: &DecideRequest,
) -> Result<DesignationRuntime, String> {
    let question = question_of(request)?;
    let index = shared_capability_index(&*state.read().await);
    let core = super::topology_read::graph_core(state, &question.graph, verified).await?;
    let graph = question.graph.clone();
    let (connector, job_class) = (question.connector.clone(), question.job_class.clone());
    let (facts, graph_revision) = tokio::task::spawn_blocking(move || {
        // The revision is read before the triples: a write between the two
        // makes the answer older than its stamp, never newer.
        let graph_revision = core.version();
        let triples = medulla_rdf::mapping::export_triples(&core, &graph)?;
        let facts: Vec<Fact> = triples
            .iter()
            .filter_map(|triple| {
                let medulla_rdf::oxrdf::NamedOrBlankNode::NamedNode(subject) = &triple.subject
                else {
                    return None;
                };
                let object = match &triple.object {
                    medulla_rdf::oxrdf::Term::NamedNode(node) => Object::Iri(unkey(node.as_str())),
                    medulla_rdf::oxrdf::Term::Literal(literal) => {
                        Object::Literal(literal.value().to_string())
                    }
                    _ => return None,
                };
                Some(Fact {
                    subject: unkey(subject.as_str()),
                    predicate: unkey(triple.predicate.as_str()),
                    object,
                })
            })
            .collect();
        Ok::<_, String>((facts_of(&connector, &job_class, &facts), graph_revision))
    })
    .await
    .map_err(|error| format!("usage guidance pack read task failed: {error}"))??;
    let store = Arc::clone(store);
    let verified_owned = verified.clone();
    let rates = tokio::task::spawn_blocking(move || tool_rates(store.as_ref(), &verified_owned))
        .await
        .map_err(|error| format!("usage guidance outcome read task failed: {error}"))??;
    Ok(DesignationRuntime {
        index,
        projection: Default::default(),
        schema_digest: String::new(),
        schema_error: None,
        load_set: None,
        usage_guidance: Some(UsageGuidanceRead {
            facts,
            rates,
            graph_revision,
        }),
    })
}
