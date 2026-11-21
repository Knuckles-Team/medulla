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

/// The three refusal codes of an abstained part (MEDULLA-CQ-ORCH-R008.3), registered with the
/// decision codes in `medulla-decision-types` so an abstention round-trips.
pub use medulla_decision_types::decision::execution_part::REFUSALS;

/// The steps of the task, each with its successor and the source that states it.
#[cfg(feature = "decide")]
const STEPS_QUERY: &str = "PREFIX : <https://knuckles-team.github.io/ontology/core#>
PREFIX dc: <http://purl.org/dc/terms/>
SELECT ?step ?next ?source WHERE {
  VALUES ?task { <TASK> }
  ?task :hasStep ?step . ?step dc:source ?source .
  OPTIONAL { ?step :precedes ?next }
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
    let assembly = super::assemble::assemble_for(state, graph, verified, request.assembly.clone())
        .await;
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
enum PartFault {
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
fn part_or_abstain(
    name: &str,
    task_iri: &str,
    outcome: Result<ExecutionAnswerPart, PartFault>,
) -> ExecutionAnswerPart {
    use medulla_decision_types::decision::execution_part::{
        ORCH_PART_ASSEMBLY_ABSTAINED, ORCH_PART_EMPTY, ORCH_PART_NO_SOURCE,
    };
    use medulla_decision_types::decision::record::{
        AbstainReason, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef,
    };

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
    };
    ExecutionAnswerPart::Abstain {
        reason: AbstainReason::Family {
            family: medulla_decision_types::decision::statistical::QuestionKind::TaskPlan,
            code,
            detail,
        },
        provenance: medulla_types::contract::BoundedVec::new(vec![premise])
            .unwrap_or_default(),
        evidence_class: EvidenceClass::Claim,
    }
}

/// The staffing, bindings and DAG parts of one assembly, and the certificate digest.
#[cfg(feature = "decide")]
fn solved_parts(
    assembly: &medulla_decision_types::decision::AssemblyResult,
    task_iri: &str,
) -> Result<(ExecutionAnswerPart, ExecutionAnswerPart, ExecutionAnswerPart, String), String> {
    use medulla_decision_types::decision::DecisionOutcome;

    let record = &assembly.record;
    let evidence_class = record.evidence_class;
    let premises = || -> Result<_, String> {
        medulla_types::contract::BoundedVec::new(
            record.premises.iter().take(PART_PREMISES).cloned().collect(),
        )
    };
    match &record.outcome {
        DecisionOutcome::Abstained { reasons } => {
            let copied = reasons
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
            Ok((part("staffing"), part("bindings"), part("dag"), String::new()))
        }
        DecisionOutcome::Solved {
            slots, certificate, ..
        } => {
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
                String::from(certificate.digest()),
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
        .unwrap_or(medulla_decision_types::decision::record::EvidenceClass::Claim);
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
                reason: reasons
                    .first()
                    .cloned()
                    .unwrap_or(medulla_decision_types::decision::record::AbstainReason::InsufficientConfidence),
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
        evidence_class: medulla_decision_types::decision::record::EvidenceClass::Claim,
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
    Ok((read(STEPS_QUERY), read(WORKFLOW_QUERY)))
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
    use medulla_decision_types::decision::record::{
        EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef,
    };
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
    let (vars, mut rows) = result.to_rows();
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
