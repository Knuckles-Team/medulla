//! `Method::AgentExecutionAnswer` (MEDULLA-CQ-ORCH-R008.2): the composed answer
//! to "How can I execute task XYZ?".
//!
//! One assembly (`assemble_for`) supplies the staffing, the bindings and the
//! DAG; the capability coverage and the guardrail entailment answer from the
//! same tenant-bound snapshots; the steps and the workflow are two SPARQL
//! SELECTs over the caller's filtered view of the request graph. Every part
//! keeps its own answer or typed abstention. Nothing is written.

use super::SharedState;
use medulla_decision_outcome_types::decision::execution_answer::AgentExecutionAnswerRequest;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::evidence_class::EvidenceClass;

/// The steps of the task, each with its successor and the source that states it.
#[cfg(feature = "decide")]
const STEPS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX dc: <http://purl.org/dc/terms/>
SELECT ?step ?next ?source WHERE {
  VALUES ?task { <TASK> }
  ?task :hasStep ?step . ?step dc:source ?source .
  OPTIONAL { ?step :precedes ?next }
}";

/// The tool-use contract steps an imported connector pack names for the task
/// (MEDULLA-CQ-ORCH-R012.1): each step's contract, tool, order and successor.
#[cfg(feature = "decide")]
const CONTRACT_STEPS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX sc: <https://knuckles-team.github.io/ontology/surface-contract#>
SELECT ?contract ?step ?tool ?order ?next WHERE {
  VALUES ?task { <TASK> }
  ?contract sc:forTask ?task ; :hasStep ?step .
  ?step sc:callsTool ?tool ; sc:stepOrder ?order .
  OPTIONAL { ?step :precedes ?next }
}";

/// The params, result fields and refusal codes of those steps. One value per row:
/// a multi-valued step never multiplies the rows of another list.
#[cfg(feature = "decide")]
const CONTRACT_PARAMS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX sc: <https://knuckles-team.github.io/ontology/surface-contract#>
SELECT ?step ?value WHERE {
  VALUES ?task { <TASK> }
  ?contract sc:forTask ?task ; :hasStep ?step . ?step sc:param ?value .
}";

/// The skills that serve the task and use a tool, by tool: `?tool ?skill`
/// (MEDULLA-CQ-ORCH-R012.2). One value per row.
#[cfg(feature = "decide")]
const CONTRACT_SKILLS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX sc: <https://knuckles-team.github.io/ontology/surface-contract#>
SELECT ?tool ?value WHERE {
  VALUES ?task { <TASK> }
  ?value a :Skill ; sc:servesTask ?task ; sc:usesTool ?tool .
}";

/// The agent prompts that serve the task, with the agent kind and business role they declare
/// (MEDULLA-CQ-ORCH-R012.2): `?prompt ?kind ?role`.
#[cfg(feature = "decide")]
const AGENT_PROMPTS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX sc: <https://knuckles-team.github.io/ontology/surface-contract#>
SELECT ?prompt ?kind ?role WHERE {
  VALUES ?task { <TASK> }
  ?prompt a :Prompt ; sc:servesTask ?task .
  OPTIONAL { ?prompt sc:agentKind ?kind }
  OPTIONAL { ?prompt sc:businessRole ?role }
}";

#[cfg(feature = "decide")]
const CONTRACT_RESULTS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX sc: <https://knuckles-team.github.io/ontology/surface-contract#>
SELECT ?step ?value WHERE {
  VALUES ?task { <TASK> }
  ?contract sc:forTask ?task ; :hasStep ?step . ?step sc:resultField ?value .
}";

#[cfg(feature = "decide")]
const CONTRACT_REFUSALS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX sc: <https://knuckles-team.github.io/ontology/surface-contract#>
SELECT ?step ?value WHERE {
  VALUES ?task { <TASK> }
  ?contract sc:forTask ?task ; :hasStep ?step .
  ?step sc:refusal ?refusal . ?refusal sc:refusalCode ?value .
}";

/// The workflow hand-offs between the task's steps, each with its source.
#[cfg(feature = "decide")]
const WORKFLOW_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX dc: <http://purl.org/dc/terms/>
SELECT ?from ?to ?source WHERE {
  VALUES ?task { <TASK> }
  ?task :hasStep ?from . ?from :handsOffTo ?to ; dc:source ?source .
}";

/// Who assembled the answer.
#[cfg(feature = "decide")]
const ASSEMBLED_BY: &str = "graph.assemble";

/// Most premises one part carries.
#[cfg(feature = "decide")]
const PART_PREMISES: usize = 64;

/// Answer the seven questions about one task from one assembly.
#[cfg(feature = "decide")]
pub async fn handle_agent_execution_answer(
    state: &SharedState,
    req_id: u64,
    (graph, verified): (&str, &VerifiedRequestContext),
    request: AgentExecutionAnswerRequest,
) -> Response {
    match answer_for(state, graph, verified, request).await {
        Ok(answer) => Response::ok(
            req_id,
            medulla_protocol_types::protocol::ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentExecutionAnswer,
            >(&answer),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "decide")]
async fn answer_for(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    request: AgentExecutionAnswerRequest,
) -> Result<medulla_decision_outcome_types::decision::execution_answer::ExecutionAnswer, String> {
    use medulla_agent_types::agent_guardrail::GuardrailEntailmentRequest;
    use medulla_decision_outcome_types::decision::execution_answer::{
        ExecutionAnswer, ExecutionAnswerParts, EXECUTION_ANSWER_SCHEMA_VERSION,
    };

    // (1) Admit: both checks run before any read.
    let guardrail_request = GuardrailEntailmentRequest {
        tenant_id: request.assembly.tenant_id.clone(),
        task_iri: request.task_iri.clone(),
    };
    guardrail_request.validate()?;
    if request.task_iri.contains(['<', '>', '"']) {
        return Err("INVALID_ARGUMENT: the task IRI must not contain `<`, `>` or `\"`".into());
    }
    if request.assembly.tenant_id != verified.tenant() {
        return Err(
            "ACCESS_DENIED: assembly tenant must match the verified request tenant".to_string(),
        );
    }

    // From here every part is computed on its own: a part that cannot be answered becomes a
    // typed abstention and never fails the response or drops a field (R008.3).
    let task_iri = request.task_iri.as_str();

    // (2) One assembly. Its failure or abstention is shared by the three parts that read it.
    let assembly =
        super::assemble::assemble_for(state, graph, verified, request.assembly.clone()).await;
    let (staffing_part, bindings_part, dag_part, certificate_digest) = match &assembly {
        Err(error) => {
            let fault = || PartFault::AssemblyAbstained(error.clone());
            (
                part_or_abstain("staffing", task_iri, Err(fault())),
                part_or_abstain("bindings", task_iri, Err(fault())),
                part_or_abstain("dag", task_iri, Err(fault())),
                String::new(),
            )
        }
        Ok(assembly) => solved_parts(assembly, task_iri)?,
    };

    // (3) Coverage over the same requirements, candidates and tenant.
    let coverage_part = part_or_abstain(
        "coverage",
        task_iri,
        coverage_answer(state, verified, &request, assembly.as_ref().ok()).await,
    );

    // (4) Guardrails.
    let guardrails_part = part_or_abstain(
        "guardrails",
        task_iri,
        guardrails_answer(state, verified, &guardrail_request).await,
    );

    // (5) Steps and workflow, read as the caller.
    let rows = steps_and_workflow(state, graph, verified, task_iri).await;
    let (steps_part, workflow_part) = match rows {
        Ok((steps, workflow)) => (
            part_or_abstain("steps", task_iri, steps),
            part_or_abstain("workflow", task_iri, workflow),
        ),
        Err(error) => (
            part_or_abstain("steps", task_iri, Err(PartFault::NoSource(error.clone()))),
            part_or_abstain("workflow", task_iri, Err(PartFault::NoSource(error))),
        ),
    };

    Ok(ExecutionAnswer {
        schema_version: EXECUTION_ANSWER_SCHEMA_VERSION,
        task_iri: request.task_iri,
        parts: ExecutionAnswerParts {
            steps: steps_part,
            staffing: staffing_part,
            coverage: coverage_part,
            guardrails: guardrails_part,
            bindings: bindings_part,
            workflow: workflow_part,
            dag: dag_part,
        },
        assembled_by: ASSEMBLED_BY.to_string(),
        certificate_digest,
    })
}

/// Why one part has no answer (MEDULLA-CQ-ORCH-R008.3).
#[cfg(feature = "decide")]
pub(super) enum PartFault {
    /// The part's source could not be read: the concept has no connected authority.
    NoSource(String),
    /// The part's query returned no row.
    Empty,
    /// The assembly the part reads abstained; the reason is copied into the detail.
    AssemblyAbstained(String),
    /// The part is already a typed abstention of its own (a coverage gap).
    Own(ExecutionAnswerPart),
}

/// The part's answer, or its typed abstention: `AbstainReason::Family` with one of the three
/// `ORCH_PART_*` codes, evidence class `Claim` and the premise `orch:part=<name>/<code>`.
#[cfg(feature = "decide")]
pub(super) fn part_or_abstain(
    name: &str,
    task_iri: &str,
    outcome: Result<ExecutionAnswerPart, PartFault>,
) -> ExecutionAnswerPart {
    use medulla_decision_types::decision::execution_part::{
        ORCH_PART_ASSEMBLY_ABSTAINED, ORCH_PART_EMPTY, ORCH_PART_NO_SOURCE,
    };
    use medulla_decision_types::decision::record::{
        AbstainReason, PremiseClass, PremiseProvenance, PremiseRef,
    };
    use medulla_types::evidence_class::EvidenceClass;

    let (code, detail) = match outcome {
        Ok(part) | Err(PartFault::Own(part)) => return part,
        Err(PartFault::NoSource(detail)) => (ORCH_PART_NO_SOURCE, detail),
        Err(PartFault::Empty) => (ORCH_PART_EMPTY, format!("no row for {task_iri}")),
        Err(PartFault::AssemblyAbstained(detail)) => (ORCH_PART_ASSEMBLY_ABSTAINED, detail),
    };
    let premise = PremiseRef {
        subject: task_iri.to_string(),
        fact: format!("orch:part={name}/{}", code.as_str()),
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: "task_iri".to_string(),
        },
        trust_tier: None,
    };
    ExecutionAnswerPart::Abstain {
        reason: AbstainReason::Family {
            family: medulla_decision_types::decision::statistical::QuestionKind::TaskPlan,
            code,
            detail,
        },
        provenance: medulla_types::contract::BoundedVec::new(vec![premise]).unwrap_or_default(),
        evidence_class: EvidenceClass::Claim,
    }
}

/// The staffing, bindings and DAG parts of one assembly, and the digest of the sealed assembly record.
#[cfg(feature = "decide")]
fn solved_parts(
    assembly: &medulla_decision_types::decision::AssemblyResult,
    task_iri: &str,
) -> Result<
    (
        ExecutionAnswerPart,
        ExecutionAnswerPart,
        ExecutionAnswerPart,
        String,
    ),
    String,
> {
    use medulla_decision_types::decision::DecisionOutcome;

    let record = &assembly.record;
    let evidence_class = record.evidence_class;
    let premises = || -> Result<_, String> {
        medulla_types::contract::BoundedVec::new(
            record
                .premises
                .iter()
                .take(PART_PREMISES)
                .cloned()
                .collect(),
        )
    };
    match &record.outcome {
        DecisionOutcome::Abstained { reasons } => {
            let copied = reasons
                .as_slice()
                .first()
                .map(|reason| reason.code().as_str().to_string())
                .unwrap_or_else(|| "INSUFFICIENT_CONFIDENCE".to_string());
            let part = |name: &str| {
                part_or_abstain(
                    name,
                    task_iri,
                    Err(PartFault::AssemblyAbstained(copied.clone())),
                )
            };
            Ok((
                part("staffing"),
                part("bindings"),
                part("dag"),
                String::new(),
            ))
        }
        DecisionOutcome::Solved { slots, .. } => {
            let staffing: Vec<serde_json::Value> = slots
                .iter()
                .zip(assembly.agents.iter())
                .map(|(slot, agent)| {
                    serde_json::json!({
                        "slot": slot.slot,
                        "agent_id": agent.agent_id,
                        "role": agent.role,
                        "model_identity": agent.model_identity,
                    })
                })
                .collect();
            let bindings: Vec<serde_json::Value> = assembly
                .agents
                .iter()
                .map(|agent| {
                    serde_json::json!({
                        "agent_id": agent.agent_id,
                        "system_prompt": agent.system_prompt.component_id,
                        "model_profile": agent.model_profile.component_id,
                        "tools": agent.tools.iter().map(|d| d.component_id.as_str()).collect::<Vec<_>>(),
                        "skills": agent.skills.iter().map(|d| d.component_id.as_str()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let shape = assembly
                .graph
                .as_ref()
                .map(|draft| serde_json::to_value(&draft.shape))
                .transpose()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "INTERNAL: a solved assembly carries no graph".to_string())?;
            let answer = |value: serde_json::Value| -> Result<ExecutionAnswerPart, String> {
                Ok(ExecutionAnswerPart::Answer {
                    value,
                    provenance: premises()?,
                    evidence_class,
                })
            };
            Ok((
                answer(serde_json::Value::Array(staffing))?,
                answer(serde_json::Value::Array(bindings))?,
                answer(shape)?,
                record.record_digest.clone(),
            ))
        }
    }
}

/// The coverage part: the coverage answer, a coverage gap as its own typed abstention, or no
/// source when the coverage read fails.
#[cfg(feature = "decide")]
async fn coverage_answer(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    request: &AgentExecutionAnswerRequest,
    assembly: Option<&medulla_decision_types::decision::AssemblyResult>,
) -> Result<ExecutionAnswerPart, PartFault> {
    use medulla_decision_outcome_types::decision::coverage::{
        CapabilityCoverageOutcome, CapabilityCoverageRequest,
    };

    let evidence_class = assembly
        .map(|assembly| assembly.record.evidence_class)
        .unwrap_or(medulla_types::evidence_class::EvidenceClass::Claim);
    let coverage = super::coverage::coverage_for(
        state,
        verified,
        &CapabilityCoverageRequest {
            tenant_id: request.assembly.tenant_id.clone(),
            requirements: request.assembly.requirements.clone(),
            candidates: request.assembly.candidates.clone(),
        },
    )
    .await
    .map_err(PartFault::NoSource)?;
    match &coverage.coverage {
        CapabilityCoverageOutcome::Covered { capabilities } => Ok(ExecutionAnswerPart::Answer {
            value: serde_json::to_value(&coverage)
                .map_err(|error| PartFault::NoSource(error.to_string()))?,
            provenance: medulla_types::contract::BoundedVec::new(
                capabilities
                    .iter()
                    .take(PART_PREMISES)
                    .map(|capability| capability.because.clone())
                    .collect(),
            )
            .map_err(PartFault::NoSource)?,
            evidence_class,
        }),
        CapabilityCoverageOutcome::Abstained { reasons } => {
            Err(PartFault::Own(ExecutionAnswerPart::Abstain {
                reason: reasons.first().cloned().unwrap_or(
                    medulla_decision_types::decision::record::AbstainReason::InsufficientConfidence,
                ),
                provenance: Default::default(),
                evidence_class,
            }))
        }
    }
}

/// The guardrails part: the entailment answer, or no source when the read fails.
#[cfg(feature = "decide")]
async fn guardrails_answer(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    request: &medulla_agent_types::agent_guardrail::GuardrailEntailmentRequest,
) -> Result<ExecutionAnswerPart, PartFault> {
    let guardrails = super::guardrail::entailment_for(state, verified, request)
        .await
        .map_err(PartFault::NoSource)?;
    Ok(ExecutionAnswerPart::Answer {
        value: serde_json::to_value(&guardrails)
            .map_err(|error| PartFault::NoSource(error.to_string()))?,
        provenance: Default::default(),
        evidence_class: medulla_types::evidence_class::EvidenceClass::Claim,
    })
}

/// The steps and workflow parts, each read from the caller's filtered view. The outer error is a
/// view that could not be read at all.
#[cfg(feature = "decide")]
async fn steps_and_workflow(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    task_iri: &str,
) -> Result<
    (
        Result<ExecutionAnswerPart, PartFault>,
        Result<ExecutionAnswerPart, PartFault>,
    ),
    String,
> {
    let (view, graph_version) =
        super::stat_support::filtered_view_versioned(state, verified.agent_id(), graph).await?;
    let max_rows = usize::try_from(
        medulla_server_api::knob_config::knobs().execution_answer_max_rows_per_part,
    )
    .unwrap_or(usize::MAX);
    let read = |query: &str| query_part(&view, query, task_iri, (graph, graph_version), max_rows);
    let contracts = contract_steps(&view, task_iri, max_rows);
    let prompts = agent_prompts(&view, task_iri, max_rows);
    Ok((
        merge_contract_steps(
            read(STEPS_QUERY),
            contracts,
            prompts,
            (graph, graph_version),
        ),
        read(WORKFLOW_QUERY),
    ))
}

/// One step of an imported tool-use contract (MEDULLA-CQ-ORCH-R012.1).
#[cfg(feature = "decide")]
struct ContractStep {
    contract: String,
    step: String,
    tool: String,
    order: u64,
    next: Option<String>,
    params: Vec<String>,
    result_fields: Vec<String>,
    refusal_codes: Vec<String>,
    /// The skills that serve the task and use this step's tool.
    skills: Vec<String>,
}

/// One agent prompt that serves the task: a staffing candidate (MEDULLA-CQ-ORCH-R012.2).
#[cfg(feature = "decide")]
struct AgentPrompt {
    prompt: String,
    agent_kind: Option<String>,
    business_role: Option<String>,
}

/// The agent prompts that serve `task_iri`, by prompt IRI. A read that fails is an error.
#[cfg(feature = "decide")]
fn agent_prompts(
    view: &medulla_graph_core::graph::GraphView,
    task_iri: &str,
    max_rows: usize,
) -> Result<Vec<AgentPrompt>, String> {
    let mut prompts: Vec<AgentPrompt> = Vec::new();
    for row in select_rows(view, AGENT_PROMPTS_QUERY, task_iri)?.1 {
        let cell = |index: usize| row.get(index).cloned().flatten().map(|v| lexical(&v));
        let Some(prompt) = cell(0) else { continue };
        if prompts.iter().any(|held| {
            held.prompt == prompt && held.agent_kind == cell(1) && held.business_role == cell(2)
        }) {
            continue;
        }
        prompts.push(AgentPrompt {
            prompt,
            agent_kind: cell(1),
            business_role: cell(2),
        });
    }
    prompts.sort_by(|a, b| a.prompt.cmp(&b.prompt));
    prompts.truncate(max_rows);
    Ok(prompts)
}

/// The lexical form of one SPARQL term: a quoted literal loses its quotes and its
/// datatype or language tag; an IRI stays as is.
#[cfg(feature = "decide")]
fn lexical(term: &str) -> String {
    let term = term.trim();
    match term.strip_prefix('"') {
        Some(rest) => rest
            .rsplit_once('"')
            .map(|(value, _)| value)
            .unwrap_or(rest)
            .to_string(),
        None => term.trim_matches(['<', '>']).to_string(),
    }
}

/// The last segment of an individual's IRI, the tool's name.
#[cfg(feature = "decide")]
fn local_name(iri: &str) -> &str {
    iri.rsplit([':', '#', '/']).next().unwrap_or(iri)
}

/// The connector of a contract individual `urn:scion:<connector>:<kind>:<name>`.
#[cfg(feature = "decide")]
fn connector_of(contract: &str) -> String {
    contract
        .strip_prefix("urn:scion:")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_default()
        .to_string()
}

/// Run one SELECT for `task_iri` over the caller's view: its variables and its rows,
/// sorted so the same graph answers the same bytes.
#[cfg(feature = "decide")]
fn select_rows(
    view: &medulla_graph_core::graph::GraphView,
    query: &str,
    task_iri: &str,
) -> Result<(Vec<String>, Vec<Vec<Option<String>>>), String> {
    use medulla_rdf::sparql::{Dataset, Projection, QueryOutcome};

    let text = query.replace("<TASK>", &format!("<{task_iri}>"));
    let outcome = medulla_rdf::sparql::execute(
        &Dataset::new(view, Vec::new()),
        &text,
        &Projection::default(),
        None,
    )?;
    let QueryOutcome::Solutions(result) = outcome else {
        return Err("INTERNAL: the execution answer query did not answer a table".to_string());
    };
    // The answer reads each term as its one-line text (`<iri>` or the lexical form).
    let (vars, mut rows) = result.to_text_rows();
    rows.sort();
    Ok((vars, rows))
}

/// The values one step carries in a `?step ?value` list query, by step.
#[cfg(feature = "decide")]
fn values_by_step(
    view: &medulla_graph_core::graph::GraphView,
    query: &str,
    task_iri: &str,
) -> Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let mut by_step: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for row in select_rows(view, query, task_iri)?.1 {
        if let (Some(Some(step)), Some(Some(value))) = (row.first(), row.get(1)) {
            by_step
                .entry(lexical(step))
                .or_default()
                .push(lexical(value));
        }
    }
    Ok(by_step)
}

/// The tool-use contract steps of `task_iri`, in contract then step order. An empty list
/// means no imported contract names the task. A read that fails is an error, never an
/// empty list.
#[cfg(feature = "decide")]
fn contract_steps(
    view: &medulla_graph_core::graph::GraphView,
    task_iri: &str,
    max_rows: usize,
) -> Result<Vec<ContractStep>, String> {
    let mut params = values_by_step(view, CONTRACT_PARAMS_QUERY, task_iri)?;
    let mut results = values_by_step(view, CONTRACT_RESULTS_QUERY, task_iri)?;
    let mut refusals = values_by_step(view, CONTRACT_REFUSALS_QUERY, task_iri)?;
    // Skills are keyed by the tool they use (the first column of the query).
    let skills_by_tool = values_by_step(view, CONTRACT_SKILLS_QUERY, task_iri)?;
    let (_, rows) = select_rows(view, CONTRACT_STEPS_QUERY, task_iri)?;
    let mut steps: Vec<ContractStep> = Vec::new();
    for row in rows {
        let cell = |index: usize| row.get(index).cloned().flatten().map(|v| lexical(&v));
        let (Some(contract), Some(step), Some(tool), Some(order)) =
            (cell(0), cell(1), cell(2), cell(3))
        else {
            continue;
        };
        let Ok(order) = order.parse::<u64>() else {
            continue;
        };
        let take = |lists: &mut std::collections::BTreeMap<String, Vec<String>>| {
            let mut values = lists.get(&step).cloned().unwrap_or_default();
            values.sort();
            values.dedup();
            values
        };
        let next = cell(4);
        if let Some(held) = steps
            .iter_mut()
            .find(|held| held.step == step && held.tool == tool && held.order == order)
        {
            // An optional successor repeats the row once per successor.
            if held.next.is_none() {
                held.next = next;
            }
            continue;
        }
        let skills = {
            let mut skills = skills_by_tool.get(&tool).cloned().unwrap_or_default();
            skills.sort();
            skills.dedup();
            skills
        };
        steps.push(ContractStep {
            contract,
            tool,
            order,
            next,
            params: take(&mut params),
            result_fields: take(&mut results),
            refusal_codes: take(&mut refusals),
            skills,
            step,
        });
    }
    steps.sort_by(|a, b| (&a.contract, a.order, &a.step).cmp(&(&b.contract, b.order, &b.step)));
    steps.truncate(max_rows);
    Ok(steps)
}

/// The steps part with the contract steps added (MEDULLA-CQ-ORCH-R012.1): the authored
/// steps keep their rows, and each contract step is one entry of `contract_steps` with
/// its connector, tool, order, params, result fields and refusal codes. A task that has
/// no authored row but has a contract is answered from the contract; a task with
/// neither stays `ORCH_PART_EMPTY`. A contract read that fails is no source for the part.
#[cfg(feature = "decide")]
fn merge_contract_steps(
    authored: Result<ExecutionAnswerPart, PartFault>,
    contracts: Result<Vec<ContractStep>, String>,
    prompts: Result<Vec<AgentPrompt>, String>,
    (graph, graph_version): (&str, u64),
) -> Result<ExecutionAnswerPart, PartFault> {
    use medulla_decision_types::decision::record::{PremiseClass, PremiseProvenance, PremiseRef};

    let contracts = match contracts {
        Ok(steps) => steps,
        Err(error) => return Err(PartFault::NoSource(error)),
    };
    let prompts = match prompts {
        Ok(prompts) => prompts,
        Err(error) => return Err(PartFault::NoSource(error)),
    };
    if contracts.is_empty() {
        return authored;
    }
    let entries: Vec<serde_json::Value> = contracts
        .iter()
        .map(|step| {
            serde_json::json!({
                "contract": step.contract,
                "connector": connector_of(&step.contract),
                "step": step.step,
                "tool": local_name(&step.tool),
                "tool_iri": step.tool,
                "order": step.order,
                "next": step.next.as_deref().map(lexical),
                "params": step.params,
                "result_fields": step.result_fields,
                "refusal_codes": step.refusal_codes,
                "skills": step.skills,
                // A step whose tool no skill of the task uses abstains in its own entry
                // (MEDULLA-CQ-ORCH-R012.2).
                "skill_abstain": step.skills.is_empty().then(|| serde_json::json!({
                    "family": "task_plan",
                    "code": medulla_decision_types::decision::execution_part::ORCH_PART_EMPTY.as_str(),
                    "detail": format!("no skill of {} uses tool {}", step.contract, step.tool),
                })),
            })
        })
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let contract_premises: Vec<PremiseRef> = contracts
        .iter()
        .filter(|step| seen.insert(step.contract.clone()))
        .take(PART_PREMISES)
        .map(|step| PremiseRef {
            subject: step.step.clone(),
            fact: format!("sc:ToolUseContract {}", step.contract),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::GraphRow {
                graph: graph.to_string(),
                graph_version,
            },
            trust_tier: None,
        })
        .collect();
    let (mut value, mut provenance) = match authored {
        Ok(ExecutionAnswerPart::Answer {
            value, provenance, ..
        }) => (value, provenance.as_slice().to_vec()),
        Err(PartFault::Empty) => (
            serde_json::json!({ "vars": [], "rows": [], "truncated": false }),
            Vec::new(),
        ),
        other => return other,
    };
    value["contract_steps"] = serde_json::Value::Array(entries);
    // The agent prompts that serve the task are staffing candidates.
    value["agent_prompts"] = serde_json::Value::Array(
        prompts
            .iter()
            .map(|prompt| {
                serde_json::json!({
                    "prompt": prompt.prompt,
                    "agent_kind": prompt.agent_kind,
                    "business_role": prompt.business_role,
                })
            })
            .collect(),
    );
    provenance.extend(contract_premises);
    provenance.extend(prompts.iter().take(PART_PREMISES).map(|prompt| PremiseRef {
        subject: prompt.prompt.clone(),
        fact: format!("sc:servesTask {}", prompt.prompt),
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::GraphRow {
            graph: graph.to_string(),
            graph_version,
        },
        trust_tier: None,
    }));
    provenance.truncate(PART_PREMISES);
    Ok(ExecutionAnswerPart::Answer {
        value,
        provenance: medulla_types::contract::BoundedVec::new(provenance)
            .map_err(PartFault::NoSource)?,
        evidence_class: EvidenceClass::Claim,
    })
}

#[cfg(feature = "decide")]
use medulla_decision_outcome_types::decision::execution_answer::ExecutionAnswerPart;

/// Run one of the two SELECTs for `task_iri` and shape its rows as a part: the
/// rows (first `max_rows`, with `truncated`) and one `Claim` premise per
/// distinct source; no row is `PartFault::Empty`.
#[cfg(feature = "decide")]
fn query_part(
    view: &medulla_graph_core::graph::GraphView,
    query: &str,
    task_iri: &str,
    (graph, graph_version): (&str, u64),
    max_rows: usize,
) -> Result<ExecutionAnswerPart, PartFault> {
    use medulla_decision_types::decision::record::{PremiseClass, PremiseProvenance, PremiseRef};
    use medulla_rdf::sparql::{Dataset, Projection, QueryOutcome};

    let text = query.replace("<TASK>", &format!("<{task_iri}>"));
    let outcome = medulla_rdf::sparql::execute(
        &Dataset::new(view, Vec::new()),
        &text,
        &Projection::default(),
        None,
    )
    .map_err(PartFault::NoSource)?;
    let QueryOutcome::Solutions(result) = outcome else {
        return Err(PartFault::NoSource(
            "INTERNAL: the execution answer query did not answer a table".to_string(),
        ));
    };
    let (vars, mut rows) = result.to_text_rows();
    // The evaluator gives no order the caller can rely on: sort so the same
    // graph answers the same bytes.
    rows.sort();
    if rows.is_empty() {
        return Err(PartFault::Empty);
    }
    let source_column = vars.iter().position(|var| var == "source");
    let subject_column = 0;
    let mut seen = std::collections::BTreeSet::new();
    let mut provenance = Vec::new();
    for row in &rows {
        let Some(Some(source)) = source_column.and_then(|column| row.get(column)) else {
            continue;
        };
        if provenance.len() < PART_PREMISES && seen.insert(source.clone()) {
            provenance.push(PremiseRef {
                subject: row
                    .get(subject_column)
                    .cloned()
                    .flatten()
                    .unwrap_or_default(),
                fact: format!("dc:source {source}"),
                class: PremiseClass::Claim,
                provenance: PremiseProvenance::GraphRow {
                    graph: graph.to_string(),
                    graph_version,
                },
                trust_tier: None,
            });
        }
    }
    let truncated = rows.len() > max_rows;
    rows.truncate(max_rows);
    Ok(ExecutionAnswerPart::Answer {
        value: serde_json::json!({ "vars": vars, "rows": rows, "truncated": truncated }),
        provenance: medulla_types::contract::BoundedVec::new(provenance)
            .map_err(PartFault::NoSource)?,
        evidence_class: EvidenceClass::Claim,
    })
}

/// The `(connector, tool name)` pairs the tool-use contracts of `task_iri` name in `graph`, read
/// through the caller's filtered view and sorted (MEDULLA-SDLC-R016.4). An empty list means no
/// imported contract names the task; a read that fails is an error, never an empty list.
#[cfg(feature = "decide")]
pub async fn read_contract_tools(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    task_iri: &str,
) -> Result<Vec<(String, String)>, String> {
    let (view, _) =
        super::stat_support::filtered_view_versioned(state, verified.agent_id(), graph).await?;
    let max_rows = usize::try_from(
        medulla_server_api::knob_config::knobs().execution_answer_max_rows_per_part,
    )
    .unwrap_or(usize::MAX);
    let mut tools: Vec<(String, String)> = contract_steps(&view, task_iri, max_rows)?
        .into_iter()
        .map(|step| {
            (
                connector_of(&step.contract),
                local_name(&step.tool).to_string(),
            )
        })
        .collect();
    tools.sort();
    tools.dedup();
    Ok(tools)
}

/// A build without the Decide layer reads no contract.
#[cfg(not(feature = "decide"))]
pub async fn read_contract_tools(
    _state: &SharedState,
    _graph: &str,
    _verified: &VerifiedRequestContext,
    _task_iri: &str,
) -> Result<Vec<(String, String)>, String> {
    Err("research tools require the `decide` feature".to_string())
}

/// A build without the Decide layer still serves the method: it refuses by
/// name.
#[cfg(not(feature = "decide"))]
pub async fn handle_agent_execution_answer(
    _state: &SharedState,
    req_id: u64,
    _request_context: (&str, &VerifiedRequestContext),
    _request: AgentExecutionAnswerRequest,
) -> Response {
    Response::err(req_id, "AgentExecutionAnswer requires the `decide` feature")
}
