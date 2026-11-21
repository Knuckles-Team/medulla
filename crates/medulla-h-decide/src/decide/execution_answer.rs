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
    use medulla_decision_outcome_types::decision::coverage::CapabilityCoverageRequest;
    use medulla_decision_outcome_types::decision::execution_answer::{
        ExecutionAnswer, ExecutionAnswerParts, EXECUTION_ANSWER_SCHEMA_VERSION,
    };
    use medulla_decision_types::decision::record::AbstainReason;
    use medulla_decision_types::decision::DecisionOutcome;

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

    // (2) One assembly.
    let assembly = super::assemble::assemble_for(state, graph, verified, request.assembly.clone())
        .await?;
    let record = &assembly.record;
    let evidence_class = record.evidence_class;
    let premises = || -> Result<_, String> {
        medulla_types::contract::BoundedVec::new(
            record.premises.iter().take(PART_PREMISES).cloned().collect(),
        )
    };
    let abstain = |reasons: &[AbstainReason]| -> ExecutionAnswerPart {
        ExecutionAnswerPart::Abstain {
            reason: reasons
                .first()
                .cloned()
                .unwrap_or(AbstainReason::InsufficientConfidence),
            provenance: Default::default(),
            evidence_class,
        }
    };

    // (3) Coverage over the same requirements, candidates and tenant.
    let coverage = super::coverage::coverage_for(
        state,
        verified,
        &CapabilityCoverageRequest {
            tenant_id: request.assembly.tenant_id.clone(),
            requirements: request.assembly.requirements.clone(),
            candidates: request.assembly.candidates.clone(),
        },
    )
    .await?;
    let coverage_part = match &coverage.coverage {
        medulla_decision_outcome_types::decision::coverage::CapabilityCoverageOutcome::Covered {
            capabilities,
        } => ExecutionAnswerPart::Answer {
            value: serde_json::to_value(&coverage).map_err(|error| error.to_string())?,
            provenance: medulla_types::contract::BoundedVec::new(
                capabilities
                    .iter()
                    .take(PART_PREMISES)
                    .map(|capability| capability.because.clone())
                    .collect(),
            )?,
            evidence_class,
        },
        medulla_decision_outcome_types::decision::coverage::CapabilityCoverageOutcome::Abstained {
            reasons,
        } => abstain(reasons),
    };

    // (4) Guardrails.
    let guardrails =
        super::guardrail::entailment_for(state, verified, &guardrail_request).await?;
    let guardrails_part = ExecutionAnswerPart::Answer {
        value: serde_json::to_value(&guardrails).map_err(|error| error.to_string())?,
        provenance: Default::default(),
        evidence_class,
    };

    // (5) Steps and workflow, read as the caller.
    let (view, graph_version) =
        super::stat_support::filtered_view_versioned(state, verified.agent_id(), graph).await?;
    let max_rows = usize::try_from(
        medulla_server_api::knob_config::knobs().execution_answer_max_rows_per_part,
    )
    .unwrap_or(usize::MAX);
    let steps_part = query_part(
        &view,
        STEPS_QUERY,
        &request.task_iri,
        (graph, graph_version),
        max_rows,
    )?;
    let workflow_part = query_part(
        &view,
        WORKFLOW_QUERY,
        &request.task_iri,
        (graph, graph_version),
        max_rows,
    )?;

    // (6) Staffing, bindings and DAG come from the solved assembly.
    let (staffing_part, bindings_part, dag_part, certificate_digest) = match &record.outcome {
        DecisionOutcome::Abstained { reasons } => (
            abstain(reasons.as_slice()),
            abstain(reasons.as_slice()),
            abstain(reasons.as_slice()),
            String::new(),
        ),
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
            (
                answer(serde_json::Value::Array(staffing))?,
                answer(serde_json::Value::Array(bindings))?,
                answer(shape)?,
                String::from(certificate.digest()),
            )
        }
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

#[cfg(feature = "decide")]
use medulla_decision_outcome_types::decision::execution_answer::ExecutionAnswerPart;

/// Run one of the two SELECTs for `task_iri` and shape its rows as a part: the
/// rows (first `max_rows`, with `truncated`) and one `Claim` premise per
/// distinct source; no row is the typed abstention `UnmappedTask`.
#[cfg(feature = "decide")]
fn query_part(
    view: &medulla_graph_core::graph::GraphView,
    query: &str,
    task_iri: &str,
    (graph, graph_version): (&str, u64),
    max_rows: usize,
) -> Result<ExecutionAnswerPart, String> {
    use medulla_decision_types::decision::record::{
        AbstainReason, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef,
    };
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
    let (vars, mut rows) = result.to_rows();
    // The evaluator gives no order the caller can rely on: sort so the same
    // graph answers the same bytes.
    rows.sort();
    if rows.is_empty() {
        use sha2::{Digest, Sha256};
        return Ok(ExecutionAnswerPart::Abstain {
            reason: AbstainReason::UnmappedTask {
                text_digest: hex::encode(Sha256::digest(task_iri.as_bytes())),
            },
            provenance: Default::default(),
            evidence_class: EvidenceClass::Claim,
        });
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
            });
        }
    }
    let truncated = rows.len() > max_rows;
    rows.truncate(max_rows);
    Ok(ExecutionAnswerPart::Answer {
        value: serde_json::json!({ "vars": vars, "rows": rows, "truncated": truncated }),
        provenance: medulla_types::contract::BoundedVec::new(provenance)?,
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
