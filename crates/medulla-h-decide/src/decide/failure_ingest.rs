//! `Method::FailureIngest`: failure-pattern mining over the failed runs of a
//! caller graph (MEDULLA-DECISION-ENGINE-R701.3).
//!
//! The handler reads the caller's row-filtered view of `graph`, turns every
//! failed `RunTrace` node into one `FailureRecord`, clusters the records by
//! signature and plans one `FailureGap` node per recurring pattern that has no
//! gap yet. It is the single writer of gap nodes, but it commits nothing
//! itself: it answers a [`PlannedWrite`] that the router commits through the
//! graph gateway under the caller's verified context. A repeat `Ingest` plans
//! nothing and an addressed gap is never reopened.

use medulla_decision_types::failure_patterns::FailureOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::planned_write::PlannedWrite;
use super::SharedState;

/// Plan one `FailureIngest` operation for the verified caller.
pub async fn handle_failure_ingest(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: FailureOp,
) -> Result<PlannedWrite, String> {
    #[cfg(feature = "decide")]
    {
        served::plan(state, verified, op).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, op);
        Err("FailureIngest requires the `decide` feature".to_string())
    }
}

#[cfg(feature = "decide")]
mod served {
    use std::collections::{BTreeMap, BTreeSet};

    use medulla_decision_stats::decision::usage_guidance::{
        order_steps, OrderError, PlanStep, UsageKnobs,
    };
    use medulla_decision_types::failure_patterns::{
        FailureIngestResult, FailureOp, FailurePattern, FailureRecord, GapStatus, GapTopic,
        Signature, MAX_FAILURE_STEPS,
    };
    use medulla_epistemic::failure_patterns::{cluster, is_recurring, ClusterParams};
    use medulla_graph_core::graph::GraphView;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        FailureGet, FailureIngestRun, FailureMarkAddressed,
    };
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_types::contract::BoundedVec;
    use medulla_types::gap_topic_machine::{GapStatusKind, GapTopicEvent};
    use medulla_types::transition_refused::MachineState;
    use serde_json::{json, Map, Value};

    use crate::decide::planned_write::PlannedWrite;
    use crate::decide::stat_support::{filtered_view_versioned, invalid_parameter};
    use crate::decide::SharedState;

    /// The `node_type` of a run's trace node.
    const RUN_TRACE: &str = "RunTrace";
    /// The termination reason of a run that answered.
    const FINAL_ANSWER: &str = "final_answer";
    /// The `type` of a gap node this handler writes.
    const FAILURE_GAP: &str = "FailureGap";
    const HAS_STEP: &str = "hasStep";
    const PRECEDES: &str = "precedes";
    const USES_TOOL: &str = "usesTool";

    fn gap_node_id(topic_id: &str) -> String {
        format!("gap:{topic_id}")
    }

    pub(super) async fn plan(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: FailureOp,
    ) -> Result<PlannedWrite, String> {
        let graph = op.graph().to_string();
        let (view, _) = filtered_view_versioned(state, verified.agent_id(), &graph).await?;
        match op {
            FailureOp::Ingest { since_ms, .. } => ingest(graph, &view, verified, since_ms),
            FailureOp::MarkAddressed { topic_id, by, .. } => {
                mark_addressed(graph, &view, &topic_id, &by)
            }
            FailureOp::Get { topic_id, .. } => get(graph, &view, &topic_id),
        }
    }

    fn ingest(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        since_ms: Option<u64>,
    ) -> Result<PlannedWrite, String> {
        let mut records = failed_runs(view)?;
        if let Some(since) = since_ms {
            records.retain(|record| record.at_ms >= since);
        }
        let knobs = medulla_server_api::knob_config::knobs();
        let params = ClusterParams {
            context_steps: knobs.failure_context_steps as usize,
            min_count: knobs.failure_min_count,
        };
        let patterns = cluster(&records, &params);
        let mut operations = Vec::new();
        let mut filed = Vec::new();
        for pattern in patterns.iter().filter(|p| is_recurring(p, &params)) {
            let topic_id = pattern.signature.topic_id();
            // An existing gap is never rewritten: a repeat ingest plans nothing
            // and an addressed gap is never reopened.
            if view.node_properties.contains_key(&gap_node_id(&topic_id)) {
                continue;
            }
            operations.push(gap_operation(&topic_id, pattern, verified)?);
            filed.push(topic_id);
        }
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<FailureIngestRun>(FailureIngestResult { patterns, filed })?,
        })
    }

    fn gap_operation(
        topic_id: &str,
        pattern: &FailurePattern,
        verified: &VerifiedRequestContext,
    ) -> Result<Value, String> {
        let minimal = serde_json::to_string(pattern.minimal_steps.as_slice())
            .map_err(|error| format!("failure gap encoding failed: {error}"))?;
        let mut properties = Map::new();
        properties.insert("type".into(), FAILURE_GAP.into());
        properties.insert("signature".into(), pattern.signature.0.clone().into());
        properties.insert("status".into(), next_text(GapStatusKind::initial()).into());
        properties.insert("count".into(), pattern.count.into());
        properties.insert("first_seen_ms".into(), pattern.first_seen.into());
        properties.insert("last_seen_ms".into(), pattern.last_seen.into());
        properties.insert("minimal_steps".into(), minimal.into());
        properties.insert("tenant_id".into(), verified.tenant().into());
        properties.insert("owner".into(), verified.agent_id().into());
        Ok(json!({
            "op": "upsert_node",
            "id": gap_node_id(topic_id),
            "properties": properties,
        }))
    }

    fn mark_addressed(
        graph: String,
        view: &GraphView,
        topic_id: &str,
        by: &str,
    ) -> Result<PlannedWrite, String> {
        let node_id = gap_node_id(topic_id);
        let mut row = gap_row(view, topic_id)?;
        let stored = row
            .get("status")
            .and_then(Value::as_str)
            .and_then(GapStatusKind::from_stored)
            .ok_or_else(|| invalid_parameter(format!("gap {topic_id} has no stored status")))?;
        let next = stored
            .apply(GapTopicEvent::Address)
            .map_err(|refused| refused.to_string())?;
        row.insert("status".into(), next_text(next).into());
        row.insert("addressed_by".into(), by.into());
        let topic = topic_of(topic_id, &row)?;
        Ok(PlannedWrite {
            graph,
            operations: vec![json!({
                "op": "upsert_node",
                "id": node_id,
                "properties": row,
            })],
            reply: ResultPayload::of::<FailureMarkAddressed>(topic)?,
        })
    }

    fn get(graph: String, view: &GraphView, topic_id: &str) -> Result<PlannedWrite, String> {
        let row = gap_row(view, topic_id)?;
        Ok(PlannedWrite {
            graph,
            operations: Vec::new(),
            reply: ResultPayload::of::<FailureGet>(topic_of(topic_id, &row)?)?,
        })
    }

    fn next_text(kind: GapStatusKind) -> &'static str {
        kind.stored()
    }

    /// The stored properties of the gap node of `topic_id`, as the caller sees them.
    fn gap_row(view: &GraphView, topic_id: &str) -> Result<Map<String, Value>, String> {
        view.node_row_object(&gap_node_id(topic_id))
            .filter(|row| row.get("type").and_then(Value::as_str) == Some(FAILURE_GAP))
            .ok_or_else(|| invalid_parameter(format!("no failure gap for topic {topic_id}")))
    }

    fn topic_of(topic_id: &str, row: &Map<String, Value>) -> Result<GapTopic, String> {
        let signature = row
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_parameter(format!("gap {topic_id} has no signature")))?;
        let status = match row.get("status").and_then(Value::as_str) {
            Some("open") => GapStatus::Open,
            Some("addressed") => GapStatus::Addressed {
                by: row
                    .get("addressed_by")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            _ => {
                return Err(invalid_parameter(format!(
                    "gap {topic_id} has no stored status"
                )))
            }
        };
        Ok(GapTopic {
            id: topic_id.to_string(),
            signature: Signature(signature.to_string()),
            status,
        })
    }

    /// The typed edges of the view: for each relationship the caller reads, the
    /// targets of each source in sorted order.
    fn edges_by_relationship(view: &GraphView) -> BTreeMap<&'static str, BTreeMap<String, BTreeSet<String>>> {
        let mut out: BTreeMap<&'static str, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
        for ((source, target), blobs) in &view.edge_properties {
            for blob in blobs {
                let Ok(edge) = medulla_types::msgpack::decode_property_value(blob.as_slice())
                else {
                    continue;
                };
                let Some(name) = edge.get("relationship").and_then(Value::as_str) else {
                    continue;
                };
                let Some(known) = [HAS_STEP, PRECEDES, USES_TOOL]
                    .into_iter()
                    .find(|candidate| *candidate == name)
                else {
                    continue;
                };
                out.entry(known)
                    .or_default()
                    .entry(source.clone())
                    .or_default()
                    .insert(target.clone());
            }
        }
        out
    }

    /// One `FailureRecord` per failed run: a `RunTrace` node whose
    /// `terminationReason` is present and is not `final_answer`.
    fn failed_runs(view: &GraphView) -> Result<Vec<FailureRecord>, String> {
        let edges = edges_by_relationship(view);
        let none = BTreeMap::new();
        let has_step = edges.get(HAS_STEP).unwrap_or(&none);
        let precedes = edges.get(PRECEDES).unwrap_or(&none);
        let uses_tool = edges.get(USES_TOOL).unwrap_or(&none);
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        let mut records = Vec::new();
        for id in ids {
            let Some(row) = view.node_row_object(id) else {
                continue;
            };
            if row.get("node_type").and_then(Value::as_str) != Some(RUN_TRACE) {
                continue;
            }
            let Some(reason) = row.get("terminationReason").and_then(Value::as_str) else {
                continue;
            };
            if reason == FINAL_ANSWER {
                continue;
            }
            let ordered = ordered_steps(id, has_step, precedes)?;
            let kinds: Vec<String> = ordered.iter().map(|step| step_kind(view, step)).collect();
            let last = ordered.last();
            let tool = last
                .and_then(|step| uses_tool.get(step))
                .and_then(|targets| targets.iter().next())
                .cloned()
                .unwrap_or_default();
            let failing_index = kinds.len().saturating_sub(1) as u32;
            let step_kind = kinds.last().cloned().unwrap_or_default();
            let mut kinds = kinds;
            kinds.truncate(MAX_FAILURE_STEPS);
            records.push(FailureRecord {
                record_id: id.clone(),
                run_trace_id: row
                    .get("run_trace_id")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                error_class: reason.to_string(),
                tool,
                step_kind,
                steps: BoundedVec::new(kinds)
                    .map_err(|error| invalid_parameter(format!("run {id}: {error}")))?,
                failing_index,
                at_ms: row.get("endMs").and_then(Value::as_u64).unwrap_or(0),
            });
        }
        Ok(records)
    }

    fn step_kind(view: &GraphView, step: &str) -> String {
        view.node_row_object(step)
            .and_then(|row| row.get("kind").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default()
    }

    /// The run's `hasStep` objects in the order of the one step ordering
    /// routine, which reads their `precedes` edges.
    fn ordered_steps(
        run: &str,
        has_step: &BTreeMap<String, BTreeSet<String>>,
        precedes: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Vec<String>, String> {
        let Some(steps) = has_step.get(run) else {
            return Ok(Vec::new());
        };
        let plan: Vec<PlanStep> = steps
            .iter()
            .map(|step| PlanStep {
                step_id: step.clone(),
                tool: String::new(),
                precedes: precedes
                    .get(step)
                    .map(|targets| targets.iter().cloned().collect())
                    .unwrap_or_default(),
            })
            .collect();
        order_steps(&plan, &BTreeMap::new(), &UsageKnobs { min_trials: 0 })
            .map(|order| order.steps)
            .map_err(|error| match error {
                OrderError::Cyclic(ids) => format!("STEP_ORDER_CYCLE: {}", ids.join(", ")),
                OrderError::DuplicateStep(id) => {
                    invalid_parameter(format!("run {run}: step {id} is declared twice"))
                }
            })
    }
}
