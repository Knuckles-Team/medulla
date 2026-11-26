//! The `tool_necessity` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R165.3).
//!
//! Params: `tool_id` (iri), `model_id` (iri), `model_call_id` (text) and
//! `model_called` (bool); the optional `graph` names the graph whose
//! `RunTrace` receipt nodes are read, under the caller's access check and
//! row-level filter, so another tenant's traces are never seen. The one
//! declared option is `tool`. The family measures the tool's necessity for the
//! model over the traces inside `tool_necessity.window`, compares it with what
//! the model did and answers an advisory with the options `necessity:<q32>`,
//! `agreement:<bool>` and, on a mismatch, `mismatch:<reason>`. Too few runs in
//! an arm abstain `INSUFFICIENT_SAMPLES`. `Decide` writes nothing.

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::run_adjustment::INSUFFICIENT_SAMPLES;
use medulla_decision_types::decision::run_governance::RunTraceRecord;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, QuestionKind, TypedValue};
use medulla_decision_types::decision::tool_necessity::{
    compare_tool_decision, measure_tool_necessity, MismatchReason, NecessityRefusal,
    ToolNecessityKnobs, MODEL_UNKNOWN, STALE_MEASUREMENT, TOOL_UNKNOWN,
};
pub use medulla_decision_types::decision::tool_necessity::REFUSALS;
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource};
use crate::decide::SharedState;

use super::support::{abstained, advisory, invalid, param, single_option, text_param};
use super::DecisionFamily;

/// Whether `component_id` is a published component of `kind` for the tenant.
fn published(
    ctx: &ExecutionContext,
    component_id: &str,
    kind: AgentComponentKind,
) -> Result<bool, String> {
    Ok(ctx
        .store
        .current_component(ctx.tenant_id, component_id)?
        .is_some_and(|entry| entry.kind == kind))
}

/// The graph property that holds the typed run record of a `RunTrace` node.
const RUN_TRACE_FIELD: &str = "run_trace";
const PARAM_GRAPH: &str = "graph";

/// The graph whose run traces a necessity is measured over, when the request names one.
pub(in crate::decide) async fn evidence_view(
    state: &SharedState,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    match text_param(request, PARAM_GRAPH)? {
        Some(graph) => crate::decide::stat_support::filtered_view(state, agent_id, graph)
            .await
            .map(Some),
        None => Ok(None),
    }
}

/// The family of the `tool_necessity` question kind.
pub struct ToolNecessityFamily;

struct Abstention(RefusalCode, String);

impl DecisionRefusal for Abstention {
    fn code(&self) -> RefusalCode {
        self.0
    }
    fn detail(&self) -> String {
        self.1.clone()
    }
}

fn iri<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    match param(request, name)? {
        Some(TypedValue::Iri(value)) | Some(TypedValue::Text(value)) => Ok(value),
        _ => Err(invalid(format!("parameter {name} is required and is an iri"))),
    }
}

fn model_called(request: &DecideRequest) -> Result<bool, String> {
    match param(request, "model_called")? {
        Some(TypedValue::Bool(called)) => Ok(*called),
        _ => Err(invalid("parameter model_called is required and is a bool")),
    }
}

/// The run traces of `model_id` the caller can see inside the window.
fn traces(view: Option<&GraphView>, since_ms: u64) -> Vec<RunTraceRecord> {
    let Some(view) = view else {
        return Vec::new();
    };
    let mut runs: Vec<RunTraceRecord> = view
        .node_properties
        .keys()
        .filter_map(|id| view.node_row_object(id))
        .filter(|row| row.get("kind").and_then(serde_json::Value::as_str) == Some("run_trace"))
        .filter_map(|row| serde_json::from_value(row.get(RUN_TRACE_FIELD)?.clone()).ok())
        .filter(|run: &RunTraceRecord| run.started_at_ms >= since_ms)
        .collect();
    runs.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    runs
}

fn conclude_necessity(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
    graph_view: Option<&GraphView>,
) -> Result<Executed, String> {
    let tool_id = iri(request, "tool_id")?;
    let model_id = iri(request, "model_id")?;
    let model_call_id = text_param(request, "model_call_id")?
        .ok_or_else(|| invalid("parameter model_call_id is required"))?;
    let called = model_called(request)?;
    if !published(ctx, tool_id, AgentComponentKind::Tool)? {
        return Err(invalid(format!(
            "{}: `{tool_id}` is not a published Tool component",
            TOOL_UNKNOWN.as_str()
        )));
    }
    if !published(ctx, model_id, AgentComponentKind::ModelProfile)? {
        return Err(invalid(format!(
            "{}: `{model_id}` is not a published ModelProfile component",
            MODEL_UNKNOWN.as_str()
        )));
    }
    let knobs = ToolNecessityKnobs::from_policy(policy);
    let runs = traces(graph_view, ctx.now_ms.saturating_sub(knobs.window_ms));
    let necessity = match measure_tool_necessity(&runs, tool_id, model_id, knobs.min_samples) {
        Ok(necessity) => necessity,
        Err(refusal @ NecessityRefusal::InsufficientSamples { .. }) => {
            return Ok(abstained(
                QuestionKind::ToolNecessity,
                &Abstention(RefusalCode::new(INSUFFICIENT_SAMPLES), refusal.to_string()),
            ));
        }
        Err(refusal) => return Err(invalid(refusal)),
    };
    // A measurement whose newest counted run is older than `tool_necessity.max_age` is
    // not estimated forward: it abstains.
    if ctx.now_ms.saturating_sub(necessity.newest_run_ms) > knobs.max_age_ms {
        return Ok(abstained(
            QuestionKind::ToolNecessity,
            &Abstention(
                STALE_MEASUREMENT,
                format!(
                    "the newest counted run is older than {} ms",
                    knobs.max_age_ms
                ),
            ),
        ));
    }
    let comparison = compare_tool_decision(&necessity, called, knobs.necessary_min_q32);
    let mut options = vec![
        format!("necessity:{}", comparison.necessity_q32),
        format!("agreement:{}", comparison.agreement),
    ];
    // The measurement is one recorded fact: its premises name it by its provenance digest.
    let measurement = format!("tool_necessity:{}", necessity.provenance_digest);
    let observed = |subject: &'static str, fact: String| FamilyPremise {
        subject: subject.into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(measurement.clone()),
    };
    let mut premises = vec![
        observed("tool_necessity.score", comparison.necessity_q32.to_string()),
        observed(
            "tool_necessity.samples_with",
            necessity.samples_with.to_string(),
        ),
        observed(
            "tool_necessity.samples_without",
            necessity.samples_without.to_string(),
        ),
        observed(
            "tool_necessity.provenance_digest",
            necessity.provenance_digest.to_string(),
        ),
        observed("tool_necessity.comparison", comparison.agreement.to_string()),
    ];
    if let Some(mismatch) = comparison.mismatch(model_call_id) {
        let reason = match mismatch.reason {
            MismatchReason::MissedTool => "missed_tool",
            MismatchReason::UnneededTool => "unneeded_tool",
        };
        options.push(format!("mismatch:{reason}"));
        // The mismatch links to the model call that disagreed with the measurement.
        premises.push(FamilyPremise {
            subject: format!("tool_necessity.mismatch:{reason}").into(),
            fact: mismatch.model_call_id,
            class: PremiseClass::Observation,
            source: FamilyPremiseSource::Request("model_call_id"),
        });
    }
    let mut executed = advisory(options)?;
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for ToolNecessityFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, "tool")
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_necessity(ctx, request, policy, graph_view))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }
}
