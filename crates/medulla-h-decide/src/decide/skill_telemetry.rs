//! `Method::SkillTelemetry`: skill invocations and what they measure (MEDULLA-DECISION-ENGINE-R713.3
//! to R713.7).
//!
//! `Ingest` validates a whole batch before it plans anything: one bad event refuses the batch and
//! writes none of it. The reads (`Measures`, `Rank`, `Peers`, `TracesByEval`) see only the caller's
//! tenant and the caller's row-filtered view of the graph. The handler commits nothing itself: it
//! answers a [`PlannedWrite`] the router commits through the graph gateway.

use medulla_decision_types::skill_telemetry::SkillTelemetryOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::planned_write::PlannedWrite;
use super::SharedState;

/// Plan one `SkillTelemetry` operation for the verified caller.
pub async fn handle_skill_telemetry(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: SkillTelemetryOp,
) -> Result<PlannedWrite, String> {
    #[cfg(all(feature = "decide", feature = "redb"))]
    {
        served::plan(state, verified, op).await
    }
    #[cfg(not(all(feature = "decide", feature = "redb")))]
    {
        let _ = (state, verified, op);
        Err("SkillTelemetry requires the `decide` and `redb` features".to_string())
    }
}

#[cfg(all(feature = "decide", feature = "redb"))]
mod served {
    use std::collections::{BTreeMap, BTreeSet};

    use medulla_decision_stats::decision::skill_telemetry::{
        cohort_of, measures, peer_skills, rank_skills, RankInput,
    };
    use medulla_decision_types::skill_telemetry::{
        principal_pseudonym, ChosenBy, EvalOutcome, IngestReceipt, InvocationOutcome,
        PeersAnswer, RankAnswer, SkillInvocation, SkillMeasures, SkillTelemetryOp, TracesAnswer,
        Window, INGEST_BATCH_MAX, PEER_COHORT_TOO_SMALL, SCOPE_READ, SCOPE_WRITE,
        SKILL_INVOCATION_CLASS, TELEMETRY_BATCH_TOO_LARGE, TELEMETRY_CLOCK_INVALID,
        TELEMETRY_CODE_INVALID, TELEMETRY_FIELD_UNKNOWN, TELEMETRY_SCOPE_REQUIRED,
        TELEMETRY_SKILL_UNKNOWN, TRACES_PAGE_MAX,
    };
    use medulla_graph_core::graph::GraphView;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        SkillTelemetryIngest, SkillTelemetryMeasures, SkillTelemetryPeers, SkillTelemetryRank,
        SkillTelemetryTraces,
    };
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_types::contract::Digest256;
    use medulla_types::refusal_code::RefusalCode;
    use serde_json::{json, Map, Value};

    use crate::decide::knowledge_flag::served::{edge_operation, run_trace_of};
    use crate::decide::planned_write::PlannedWrite;
    use crate::decide::stat_skill_eval::read_report;
    use crate::decide::stat_support::{filtered_view_versioned, invalid_parameter};
    use crate::decide::SharedState;

    const OBSERVED_IN: &str = "observed_in";
    const SCORED_BY: &str = "scoredBy";
    const SCORES_RUN: &str = "scoresRun";
    const DEFAULT_TRACES_LIMIT: usize = 50;
    const DEFAULT_RANK_LIMIT: usize = 100;

    fn refuse(code: RefusalCode, detail: impl std::fmt::Display) -> String {
        format!("{}: {detail}", code.as_str())
    }

    fn read_only(graph: String, reply: ResultPayload) -> PlannedWrite {
        PlannedWrite {
            graph,
            operations: Vec::new(),
            reply,
        }
    }

    pub(super) async fn plan(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: SkillTelemetryOp,
    ) -> Result<PlannedWrite, String> {
        let scope = if op.is_mutation() { SCOPE_WRITE } else { SCOPE_READ };
        if !verified.allows_action(scope) {
            return Err(refuse(
                TELEMETRY_SCOPE_REQUIRED,
                format!("this op needs the scope {scope}"),
            ));
        }
        let graph = op.graph().to_string();
        let (view, _) = filtered_view_versioned(state, verified.agent_id(), &graph).await?;
        let tenant = verified.tenant().to_string();
        match op {
            SkillTelemetryOp::Ingest { events, .. } => ingest(graph, &view, verified, events),
            SkillTelemetryOp::Measures {
                skill_ref, window, ..
            } => {
                let store = state.write().await.ensure_agent_library()?;
                let rows: Vec<SkillInvocation> = invocations_of(&view, &tenant)
                    .into_iter()
                    .filter(|row| row.skill_ref == skill_ref)
                    .collect();
                let answer = measures_of(&store, &tenant, &skill_ref, &rows, window)?;
                Ok(read_only(
                    graph,
                    ResultPayload::of::<SkillTelemetryMeasures>(answer)?,
                ))
            }
            SkillTelemetryOp::Rank {
                skill_refs,
                window,
                limit,
                ..
            } => {
                let store = state.write().await.ensure_agent_library()?;
                let mut by_skill: BTreeMap<String, Vec<SkillInvocation>> = BTreeMap::new();
                for row in invocations_of(&view, &tenant) {
                    if skill_refs.is_empty() || skill_refs.contains(&row.skill_ref) {
                        by_skill.entry(row.skill_ref.clone()).or_default().push(row);
                    }
                }
                // A skill named by the caller is ranked even with no invocation in the window.
                for skill in &skill_refs {
                    by_skill.entry(skill.clone()).or_default();
                }
                let mut inputs = Vec::new();
                for (skill_ref, rows) in &by_skill {
                    inputs.push(RankInput {
                        skill_ref: skill_ref.clone(),
                        measures: measures_of(&store, &tenant, skill_ref, rows, window)?,
                    });
                }
                let prior = medulla_server_api::knob_config::knobs().skill_quality_prior_runs;
                let mut skills = rank_skills(&inputs, prior);
                skills.truncate(limit.map_or(DEFAULT_RANK_LIMIT, |n| n as usize));
                Ok(read_only(
                    graph,
                    ResultPayload::of::<SkillTelemetryRank>(RankAnswer { skills })?,
                ))
            }
            SkillTelemetryOp::Peers { window, .. } => {
                let knobs = medulla_server_api::knob_config::knobs();
                let caller = principal_pseudonym(
                    &tenant,
                    &verified.principal_persistence_id(),
                    knobs.skill_pseudonym_salt,
                );
                let uses: Vec<(String, String)> = invocations_of(&view, &tenant)
                    .into_iter()
                    .filter(|row| window.contains(row.started_ms))
                    .map(|row| (row.principal_pseudonym, row.skill_ref))
                    .collect();
                let cohort = cohort_of(&uses, &caller);
                let skills = peer_skills(&uses, &cohort, &caller, knobs.skill_peers_min_peers)
                    .map_err(|code| {
                        debug_assert_eq!(code, PEER_COHORT_TOO_SMALL);
                        refuse(
                            code,
                            format!(
                                "the cohort holds fewer than {} other principals",
                                knobs.skill_peers_min_peers
                            ),
                        )
                    })?;
                Ok(read_only(
                    graph,
                    ResultPayload::of::<SkillTelemetryPeers>(PeersAnswer { skills })?,
                ))
            }
            SkillTelemetryOp::TracesByEval {
                skill_ref,
                outcome,
                limit,
                cursor,
                ..
            } => {
                let answer = traces_by_eval(
                    &view,
                    &tenant,
                    &skill_ref,
                    outcome,
                    limit.map_or(DEFAULT_TRACES_LIMIT, |n| n as usize).min(TRACES_PAGE_MAX),
                    cursor.as_deref(),
                )?;
                Ok(read_only(
                    graph,
                    ResultPayload::of::<SkillTelemetryTraces>(answer)?,
                ))
            }
        }
    }

    fn measures_of(
        store: &medulla_persistence_records::agent_library::AgentLibraryStore,
        tenant: &str,
        skill_ref: &str,
        rows: &[SkillInvocation],
        window: Window,
    ) -> Result<SkillMeasures, String> {
        let eval = read_report(store, tenant, skill_ref)?
            .map(|report| (report.pass_rate_ppm, report.case_count));
        Ok(measures(rows, eval, window))
    }

    fn class_of(row: &Map<String, Value>) -> Option<&str> {
        row.get("type").and_then(Value::as_str)
    }

    /// The row an invocation is stored as: the typed record and nothing else.
    fn to_row(row: &SkillInvocation) -> Map<String, Value> {
        let mut map = Map::new();
        let mut put = |key: &str, value: Value| {
            map.insert(key.to_string(), value);
        };
        put("type", SKILL_INVOCATION_CLASS.into());
        put("node_type", SKILL_INVOCATION_CLASS.into());
        put("invocation_id", row.invocation_id.clone().into());
        put("tenant_id", row.tenant_id.clone().into());
        put("skill_ref", row.skill_ref.clone().into());
        put("skill_version", row.skill_version.clone().into());
        put("chosen_by", row.chosen_by.stored().into());
        put("principal_pseudonym", row.principal_pseudonym.clone().into());
        put("run_id", row.run_id.clone().into());
        put("started_ms", row.started_ms.into());
        put("ended_ms", row.ended_ms.into());
        match &row.outcome {
            InvocationOutcome::Ok => put("outcome", "ok".into()),
            InvocationOutcome::Error { code } => {
                put("outcome", "error".into());
                put("outcome_code", code.clone().into());
            }
        }
        if let Some(cost) = row.cost_delta_micro_usd {
            put("cost_delta_micro_usd", cost.into());
        }
        put("tokens_in", row.tokens_in.into());
        put("tokens_out", row.tokens_out.into());
        map
    }

    fn from_row(row: &Map<String, Value>) -> Option<SkillInvocation> {
        let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |key: &str| row.get(key).and_then(Value::as_u64);
        let outcome = match row.get("outcome").and_then(Value::as_str)? {
            "ok" => InvocationOutcome::Ok,
            "error" => InvocationOutcome::Error {
                code: text("outcome_code")?,
            },
            _ => return None,
        };
        Some(SkillInvocation {
            invocation_id: text("invocation_id")?,
            tenant_id: text("tenant_id")?,
            skill_ref: text("skill_ref")?,
            skill_version: text("skill_version")?,
            chosen_by: match row.get("chosen_by").and_then(Value::as_str)? {
                "agent" => ChosenBy::Agent,
                "user" => ChosenBy::User,
                _ => return None,
            },
            principal_pseudonym: text("principal_pseudonym")?,
            run_id: text("run_id")?,
            started_ms: number("started_ms")?,
            ended_ms: number("ended_ms")?,
            outcome,
            cost_delta_micro_usd: row.get("cost_delta_micro_usd").and_then(Value::as_i64),
            tokens_in: number("tokens_in")?,
            tokens_out: number("tokens_out")?,
        })
    }

    /// The invocations of `tenant` in the caller's view, in id order.
    fn invocations_of(view: &GraphView, tenant: &str) -> Vec<SkillInvocation> {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| view.node_row_object(id))
            .filter(|row| class_of(row) == Some(SKILL_INVOCATION_CLASS))
            .filter_map(|row| from_row(&row))
            .filter(|row| row.tenant_id == tenant)
            .collect()
    }

    fn ingest(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        events: Vec<Value>,
    ) -> Result<PlannedWrite, String> {
        let tenant = verified.tenant();
        if events.len() > INGEST_BATCH_MAX {
            return Err(refuse(
                TELEMETRY_BATCH_TOO_LARGE,
                format!("{} events are more than the {INGEST_BATCH_MAX} one batch holds", events.len()),
            ));
        }
        // The whole batch is checked before any event is planned.
        let mut accepted: Vec<SkillInvocation> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut repeated = 0u64;
        for event in events {
            let mut row: SkillInvocation = serde_json::from_value(event).map_err(|error| {
                let text = error.to_string();
                if text.contains("unknown field") {
                    refuse(TELEMETRY_FIELD_UNKNOWN, text)
                } else {
                    invalid_parameter(text)
                }
            })?;
            if row.ended_ms < row.started_ms {
                return Err(refuse(
                    TELEMETRY_CLOCK_INVALID,
                    format!("{} ends before it starts", row.invocation_id),
                ));
            }
            if !row.outcome.code_valid() {
                return Err(refuse(
                    TELEMETRY_CODE_INVALID,
                    format!("{} carries an error code outside the closed form", row.invocation_id),
                ));
            }
            if row.invocation_id.is_empty()
                || row.run_id.is_empty()
                || row.principal_pseudonym.len() != 64
                || !row.principal_pseudonym.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(invalid_parameter(
                    "an invocation names its id, its run and a 64-hex principal pseudonym",
                ));
            }
            let skill_known = view.node_row_object(&row.skill_ref).is_some_and(|node| {
                class_of(&node) == Some("Skill")
                    && node
                        .get("tenant_id")
                        .and_then(Value::as_str)
                        .is_none_or(|owner| owner == tenant)
            });
            if !skill_known {
                return Err(refuse(
                    TELEMETRY_SKILL_UNKNOWN,
                    format!("{} is not a skill revision of the graph", row.skill_ref),
                ));
            }
            row.tenant_id = tenant.to_string();
            let node = node_id(&row.invocation_id);
            if view.node_properties.contains_key(&node) || !seen.insert(row.invocation_id.clone()) {
                repeated += 1;
                continue;
            }
            accepted.push(row);
        }
        let ids: Vec<&[u8]> = accepted.iter().map(|row| row.invocation_id.as_bytes()).collect();
        let receipt = Digest256::framed(b"medulla.skill_telemetry.receipt.v1", &ids)
            .map_err(invalid_parameter)?
            .to_hex();
        let mut operations = Vec::new();
        for row in &accepted {
            let node = node_id(&row.invocation_id);
            operations.push(json!({"op": "upsert_node", "id": node, "properties": to_row(row)}));
            if let Some(trace) = run_trace_of(view, tenant, &row.run_id) {
                operations.push(edge_operation(&node, &trace, OBSERVED_IN, &receipt));
            }
        }
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<SkillTelemetryIngest>(IngestReceipt {
                stored: accepted.len() as u64,
                repeated,
            })?,
        })
    }

    fn node_id(invocation_id: &str) -> String {
        format!("skill-invocation:{invocation_id}")
    }

    /// The `(source, target)` pairs of the view's edges named `relationship`.
    fn edges_named(view: &GraphView, relationship: &str) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = view
            .edge_properties
            .iter()
            .filter(|(_, blobs)| {
                blobs.iter().any(|blob| {
                    medulla_types::msgpack::decode_property_value(blob.as_slice())
                        .ok()
                        .and_then(|edge| {
                            edge.get("relationship")
                                .and_then(Value::as_str)
                                .map(|name| name == relationship)
                        })
                        .unwrap_or(false)
                })
            })
            .map(|((source, target), _)| (source.clone(), target.clone()))
            .collect();
        pairs.sort();
        pairs
    }

    /// Whether a `:Score` node passed: its `passed` flag, else a normalised value at or above
    /// the middle of its range.
    fn score_passed(row: &Map<String, Value>) -> Option<bool> {
        if let Some(flag) = row.get("passed").and_then(Value::as_bool) {
            return Some(flag);
        }
        let number = |key: &str| row.get(key).and_then(Value::as_f64);
        let (value, min, max) = (number("value")?, number("range_min")?, number("range_max")?);
        (max > min).then(|| (value - min) / (max - min) >= 0.5)
    }

    /// The run traces of `skill_ref` by eval outcome, newest end first, one page at a time. The
    /// `scoredBy` edge of a trace and the `scoresRun` edge of a score both link a score to its
    /// trace; a trace with no score is `unscored`, one with a failing score is `fail`.
    fn traces_by_eval(
        view: &GraphView,
        tenant: &str,
        skill_ref: &str,
        wanted: EvalOutcome,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<TracesAnswer, String> {
        let mut ended: BTreeMap<String, u64> = BTreeMap::new();
        for row in invocations_of(view, tenant)
            .into_iter()
            .filter(|row| row.skill_ref == skill_ref)
        {
            if let Some(trace) = run_trace_of(view, tenant, &row.run_id) {
                let slot = ended.entry(trace).or_insert(0);
                *slot = (*slot).max(row.ended_ms);
            }
        }
        let mut scores: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (trace, score) in edges_named(view, SCORED_BY) {
            scores.entry(trace).or_default().push(score);
        }
        for (score, trace) in edges_named(view, SCORES_RUN) {
            scores.entry(trace).or_default().push(score);
        }
        let outcome_of = |trace: &str| -> EvalOutcome {
            let verdicts: Vec<bool> = scores
                .get(trace)
                .into_iter()
                .flatten()
                .filter_map(|score| view.node_row_object(score))
                .filter_map(|row| score_passed(&row))
                .collect();
            if verdicts.is_empty() {
                EvalOutcome::Unscored
            } else if verdicts.iter().all(|passed| *passed) {
                EvalOutcome::Pass
            } else {
                EvalOutcome::Fail
            }
        };
        let mut rows: Vec<(u64, String)> = ended
            .into_iter()
            .filter(|(trace, _)| outcome_of(trace) == wanted)
            .map(|(trace, end)| (end, trace))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        if let Some(cursor) = cursor {
            let (end, id) = cursor
                .split_once(':')
                .and_then(|(end, id)| Some((end.parse::<u64>().ok()?, id.to_string())))
                .ok_or_else(|| invalid_parameter("the cursor is not one this read returned"))?;
            rows.retain(|(row_end, row_id)| *row_end < end || (*row_end == end && *row_id > id));
        }
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = more.then(|| rows.last().map(|(end, id)| format!("{end}:{id}"))).flatten();
        Ok(TracesAnswer {
            trace_ids: rows.into_iter().map(|(_, id)| id).collect(),
            next_cursor,
        })
    }
}
