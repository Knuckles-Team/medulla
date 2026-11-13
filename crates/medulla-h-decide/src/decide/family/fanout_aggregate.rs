//! The `fanout_aggregate` question family (MEDULLA-DECISION-ENGINE-R134.100).
//!
//! Applies the plan's already-selected aggregation operator to the complete
//! receipt set of a fan-out. It adds no width, no concurrency policy and no
//! second aggregation authority. `Decide` is evaluate-only and writes nothing;
//! a caller commits the record through the decision log for readback.

use medulla_decision_types::decision::knob_families::fanout::knob_values;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::task_plan::fanout::{
    aggregate_fanout, FanoutRefusal, ARTIFACT_DIGEST_MISMATCH, FANOUT_CHILD_UNKNOWN,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::filtered_view;

use super::support::{abstained, advisory, invalid, param, single_option, text_param};
use super::DecisionFamily;

/// The one option a fan-out aggregation question declares.
pub const AGGREGATE_OPTION: &str = "aggregate";
const PARAM_FANOUT: &str = "fanout";
const PARAM_GRAPH: &str = "graph";

/// The family of the `fanout_aggregate` question kind.
pub struct FanoutAggregateFamily;

fn fanout(
    request: &DecideRequest,
) -> Result<&medulla_decision_types::decision::task_plan::fanout::FanoutAggregateRequest, String> {
    match param(request, PARAM_FANOUT)? {
        Some(TypedValue::FanoutResults(body)) => Ok(body),
        Some(_) => Err(invalid("parameter fanout is not a fan-out result set")),
        None => Err(invalid("parameter fanout is required")),
    }
}

/// Open the graph the receipts live in through the caller's row-level filter
/// and refuse a receipt the caller cannot see. A receipt in another tenant, a
/// receipt hidden by the filter and a receipt that does not exist refuse with
/// the same code, so the refusal never confirms a row exists.
pub(in crate::decide) async fn fanout_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    tenant: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    let body = fanout(request)?;
    let graph = text_param(request, PARAM_GRAPH)?
        .filter(|graph| !graph.trim().is_empty())
        .ok_or_else(|| invalid("a fanout_aggregate question requires a graph"))?;
    let view = filtered_view(state, agent_id, graph).await?;
    if let Some(receipt) = &body.final_artifact_receipt {
        // A readback that is not there, not visible or in another tenant is
        // one refusal: it never confirms the node exists.
        let node = view.node_row_object(&receipt.readback_id).filter(|props| {
            props
                .get("tenant_id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|owner| owner == tenant)
        });
        if node.is_none() {
            return Err(invalid(format!(
                "{}: readback={}",
                ARTIFACT_DIGEST_MISMATCH.as_str(),
                receipt.readback_id
            )));
        }
    }
    for result in body.results.iter() {
        let visible = view
            .node_row_object(&result.receipt_id)
            .filter(|props| {
                props
                    .get("tenant_id")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|owner| owner == tenant)
            })
            .is_some();
        if !visible {
            return Err(invalid(format!(
                "{}: task={}",
                FANOUT_CHILD_UNKNOWN.as_str(),
                result.task_id
            )));
        }
    }
    Ok(view)
}

fn premises(
    body: &medulla_decision_types::decision::task_plan::fanout::FanoutAggregateRequest,
    aggregate: &medulla_decision_types::decision::task_plan::fanout::FanoutAggregate,
    knobs: &medulla_decision_types::decision::knob_families::fanout::FanoutKnobs,
) -> Vec<FamilyPremise> {
    let mut out = Vec::new();
    for result in body.results.iter() {
        out.push(FamilyPremise {
            subject: "fanout",
            fact: format!(
                "fanout_result:task={};status={};receipt={}",
                result.task_id,
                result.status.name(),
                result.receipt_id
            ),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request("params.fanout"),
        });
    }
    for entry in &aggregate.entries {
        out.push(FamilyPremise {
            subject: "fanout",
            fact: format!(
                "fanout_entry:task={};status={};value={}",
                entry.task_id,
                entry.status.name(),
                entry.value
            ),
            class: PremiseClass::Definition,
            source: FamilyPremiseSource::Policy,
        });
    }
    if let Some(account) = &aggregate.summaries {
        for (task, digest, tokens) in &account.children {
            out.push(FamilyPremise {
                subject: "fanout",
                fact: format!("child_summary:child={task};digest={digest};tokens={tokens}"),
                class: PremiseClass::Claim,
                source: FamilyPremiseSource::Request("params.fanout"),
            });
        }
        out.push(FamilyPremise {
            subject: "fanout",
            fact: format!(
                "summary_budget:plan={};total={};limit={};remaining={}",
                body.plan_digest,
                account.total_tokens,
                account.limit_tokens,
                account.remaining_tokens
            ),
            class: PremiseClass::Definition,
            source: FamilyPremiseSource::Policy,
        });
        out.push(FamilyPremise {
            subject: "fanout",
            fact: format!(
                "fanout.knobs:max_child_summary_tokens={};max_summary_tokens={}",
                knobs.max_child_summary_tokens, knobs.max_summary_tokens
            ),
            class: PremiseClass::Definition,
            source: FamilyPremiseSource::Policy,
        });
    }
    if let Some(delivery) = &aggregate.delivery {
        out.push(FamilyPremise {
            subject: "fanout",
            fact: format!(
                "final_artifact:schema={}@{};kind={};digest={}",
                delivery.schema_id,
                delivery.schema_version,
                body.final_artifact_receipt
                    .as_ref()
                    .map(|receipt| receipt.kind.as_str())
                    .unwrap_or_default(),
                delivery.artifact_digest
            ),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request("params.fanout"),
        });
        out.push(FamilyPremise {
            subject: "fanout",
            fact: format!(
                "fanout_delivery:plan={};coordinator={};receipts={};readback={};digest={}",
                delivery.plan_digest,
                delivery.coordinator,
                delivery.child_receipt_ids.join(","),
                delivery.readback_id,
                delivery.delivery_digest
            ),
            class: PremiseClass::Definition,
            source: FamilyPremiseSource::Policy,
        });
    }
    out.push(FamilyPremise {
        subject: "fanout",
        fact: format!(
            "fanout_aggregate:op={};plan={};aggregate_digest={}",
            serde_json::to_value(aggregate.op)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            body.plan_digest,
            aggregate.aggregate_digest
        ),
        class: PremiseClass::Definition,
        source: FamilyPremiseSource::Policy,
    });
    out
}

/// The delivered artifact's digest must be the `digest` of the stored readback
/// node. A delivery with no readback node in the view does not match.
fn readback_matches(
    aggregate: &medulla_decision_types::decision::task_plan::fanout::FanoutAggregate,
    graph_view: Option<&GraphView>,
) -> bool {
    let Some(delivery) = &aggregate.delivery else {
        return true;
    };
    graph_view
        .and_then(|view| view.node_row_object(&delivery.readback_id))
        .and_then(|props| {
            props
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .map(|digest| digest == delivery.artifact_digest)
        })
        .unwrap_or(false)
}

impl DecisionFamily for FanoutAggregateFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, AGGREGATE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        fanout(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let body = match fanout(request) {
            Ok(body) => body,
            Err(detail) => return Some(Err(detail)),
        };
        let knobs = knob_values(policy);
        Some(match aggregate_fanout(body, &knobs) {
            Err(refusal) => Ok(abstained(
                medulla_decision_types::decision::statistical::QuestionKind::FanoutAggregate,
                &refusal,
            )),
            Ok(aggregate) if !readback_matches(&aggregate, graph_view) => Ok(abstained(
                medulla_decision_types::decision::statistical::QuestionKind::FanoutAggregate,
                &FanoutRefusal::ArtifactDigestMismatch,
            )),
            Ok(aggregate) => {
                let ids = aggregate
                    .outputs
                    .iter()
                    .map(|digest| format!("result:{digest}"))
                    .collect();
                advisory(ids).map(|mut executed| {
                    executed.family_premises = premises(body, &aggregate, &knobs);
                    executed
                })
            }
        })
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
