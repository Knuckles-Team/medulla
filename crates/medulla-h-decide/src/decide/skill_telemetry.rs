//! `Method::SkillTelemetry`: skill invocations and what they measure (MEDULLA-DECISION-ENGINE-R713.3
//! to R713.7).
//!
//! `Ingest` validates a whole batch before it plans anything: one bad event refuses the batch and
//! writes none of it. The reads (`Measures`, `Rank`, `Peers`, `TracesByEval`) see only the caller's
//! tenant and the caller's row-filtered view of the graph. The handler commits nothing itself: it
//! answers a [`PlannedWrite`] the router commits through the graph gateway.

use medulla_decision_statistical_types::skill_telemetry::SkillTelemetryOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::planned_write::PlannedWrite;
use super::SharedState;

/// The graph node id of one invocation (MEDULLA-DECISION-ENGINE-R713.3). The id is namespaced by
/// the tenant, with the tenant length in front so no tenant and id pair spells another's, so the
/// repeat check of an ingest looks only at the caller's own tenant: a foreign tenant's invocation
/// id never collides with the caller's and never answers differently (no existence oracle).
#[cfg(all(feature = "decide", feature = "redb"))]
pub(crate) fn invocation_node_id(tenant: &str, invocation_id: &str) -> String {
    format!("skill-invocation:{}:{tenant}:{invocation_id}", tenant.len())
}

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

    use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
    use medulla_decision_stats::decision::skill_telemetry::{
        cohort_of, measures, peer_skills, rank_skills, RankInput,
    };
    use medulla_graph_core::graph::{GraphCore, GraphView};
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
    use {
        medulla_decision_statistical_types::skill_telemetry::principal_pseudonym,
        medulla_decision_statistical_types::skill_telemetry::started_day,
        medulla_decision_statistical_types::skill_telemetry::window_days,
        medulla_decision_statistical_types::skill_telemetry::ChosenBy,
        medulla_decision_statistical_types::skill_telemetry::EvalOutcome,
        medulla_decision_statistical_types::skill_telemetry::IngestReceipt,
        medulla_decision_statistical_types::skill_telemetry::InvocationOutcome,
        medulla_decision_statistical_types::skill_telemetry::PeersAnswer,
        medulla_decision_statistical_types::skill_telemetry::RankAnswer,
        medulla_decision_statistical_types::skill_telemetry::SkillInvocation,
        medulla_decision_statistical_types::skill_telemetry::SkillMeasures,
        medulla_decision_statistical_types::skill_telemetry::SkillTelemetryOp,
        medulla_decision_statistical_types::skill_telemetry::TracesAnswer,
        medulla_decision_statistical_types::skill_telemetry::Window,
        medulla_decision_statistical_types::skill_telemetry::INDEX_KEY_DAY,
        medulla_decision_statistical_types::skill_telemetry::INDEX_KEY_PRINCIPAL,
        medulla_decision_statistical_types::skill_telemetry::INDEX_KEY_SKILL,
        medulla_decision_statistical_types::skill_telemetry::INGEST_BATCH_MAX,
        medulla_decision_statistical_types::skill_telemetry::PEER_COHORT_TOO_SMALL,
        medulla_decision_statistical_types::skill_telemetry::SCOPE_READ,
        medulla_decision_statistical_types::skill_telemetry::SCOPE_WRITE,
        medulla_decision_statistical_types::skill_telemetry::SKILL_INVOCATION_CLASS,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_BATCH_TOO_LARGE,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_CLOCK_INVALID,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_CODE_INVALID,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_FIELD_UNKNOWN,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_SCOPE_REQUIRED,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_SESSION_UNKNOWN,
        medulla_decision_statistical_types::skill_telemetry::TELEMETRY_SKILL_UNKNOWN,
        medulla_decision_statistical_types::skill_telemetry::TRACES_PAGE_MAX,
    };

    use crate::decide::knowledge_flag::served::edge_operation;
    use crate::decide::planned_write::PlannedWrite;
    use crate::decide::stat_skill_eval::read_report;
    use crate::decide::stat_support::{
        edges_named, filtered_view_versioned, invalid_parameter, readable_graph,
    };
    use crate::decide::SharedState;

    const RUN_TRACE: &str = "RunTrace";
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
        let scope = if op.is_mutation() {
            SCOPE_WRITE
        } else {
            SCOPE_READ
        };
        if !verified.allows_action(scope) {
            return Err(refuse(
                TELEMETRY_SCOPE_REQUIRED,
                format!("this op needs the scope {scope}"),
            ));
        }
        let graph = op.graph().to_string();
        // Every read goes through the invocation read index: the rows it needs are looked up by
        // skill, principal and day, never found by scanning a snapshot of the whole graph.
        let (core, isolation) = readable_graph(
            state,
            verified.agent_id(),
            &graph,
            StatisticalErrorCode::CandidatePlanRefused,
        )
        .await?;
        let index = InvocationIndex {
            core,
            isolation,
            agent: verified.agent_id().to_string(),
        };
        let tenant = verified.tenant().to_string();
        match op {
            SkillTelemetryOp::Ingest { events, .. } => ingest(graph, &index, verified, events),
            SkillTelemetryOp::Measures {
                skill_ref, window, ..
            } => {
                let store = state.write().await.ensure_agent_library()?;
                let rows = index.invocations(&tenant, Some(&skill_ref), None, Some(window));
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
                if skill_refs.is_empty() {
                    for row in index.invocations(&tenant, None, None, Some(window)) {
                        by_skill.entry(row.skill_ref.clone()).or_default().push(row);
                    }
                } else {
                    for skill in &skill_refs {
                        let rows = index.invocations(&tenant, Some(skill), None, Some(window));
                        by_skill.entry(skill.clone()).or_default().extend(rows);
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
                let uses: Vec<(String, String)> = index
                    .invocations(&tenant, None, None, Some(window))
                    .into_iter()
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
                // Only this read needs the graph's edges, so only this read takes a snapshot.
                let (view, _) = filtered_view_versioned(state, verified.agent_id(), &graph).await?;
                let answer = traces_by_eval(
                    &index,
                    &view,
                    &tenant,
                    &skill_ref,
                    outcome,
                    limit
                        .map_or(DEFAULT_TRACES_LIMIT, |n| n as usize)
                        .min(TRACES_PAGE_MAX),
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
        put(
            "principal_pseudonym",
            row.principal_pseudonym.clone().into(),
        );
        put("run_id", row.run_id.clone().into());
        put("started_ms", row.started_ms.into());
        put(INDEX_KEY_DAY, started_day(row.started_ms).into());
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

    /// The read index over `SkillInvocation` rows: lookups by skill revision, principal
    /// pseudonym and day of `started_ms`, over the graph's own property index (the three keys are
    /// stored on every invocation row by the ingest batch, so they are maintained with it). A row
    /// is visible only to a caller its row-level rules admit, as in a filtered view.
    struct InvocationIndex {
        core: std::sync::Arc<GraphCore>,
        isolation: medulla_isolation::isolation::IsolationLayer,
        agent: String,
    }

    impl InvocationIndex {
        /// The ids filed under `key` = `value`. When the graph's property index has no room for
        /// the key, the ids are found by scanning, which answers the same set.
        fn ids_where(&self, key: &str, value: &str) -> BTreeSet<String> {
            if let Some(ids) = self.core.nodes_by_property(key, value) {
                return ids.into_iter().collect();
            }
            self.core
                .node_ids()
                .into_iter()
                .filter(|id| {
                    self.core
                        .get_node_properties(id)
                        .and_then(|blob| medulla_types::msgpack::decode_property_value(&blob).ok())
                        .and_then(|row| row.get(key).cloned())
                        .is_some_and(|stored| match stored {
                            Value::String(text) => text == value,
                            other => format!("{other}") == value,
                        })
                })
                .collect()
        }

        /// The row of `id` when the caller may see it.
        fn visible_row(&self, id: &str) -> Option<Map<String, Value>> {
            let blob = self.core.get_node_properties(id)?;
            #[cfg(feature = "security")]
            {
                let visibility = medulla_graph_core::rls::row_visibility(&blob);
                if !self.isolation.can_see_row(&self.agent, &visibility) {
                    return None;
                }
            }
            #[cfg(not(feature = "security"))]
            let _ = (&self.isolation, &self.agent);
            match medulla_types::msgpack::decode_property_value(&blob) {
                Ok(Value::Object(row)) => Some(row),
                _ => None,
            }
        }

        /// Whether any node is stored under `id`, whoever may see it.
        fn exists(&self, id: &str) -> bool {
            self.core.get_node_properties(id).is_some()
        }

        /// The invocations of `tenant` the caller may see, in id order, narrowed by whichever of
        /// skill revision, principal pseudonym and window are given. The lookups intersect; a
        /// window of at most 62 days adds the day buckets it covers.
        fn invocations(
            &self,
            tenant: &str,
            skill: Option<&str>,
            principal: Option<&str>,
            window: Option<Window>,
        ) -> Vec<SkillInvocation> {
            let mut sets: Vec<BTreeSet<String>> = Vec::new();
            if let Some(skill) = skill {
                sets.push(self.ids_where(INDEX_KEY_SKILL, skill));
            }
            if let Some(principal) = principal {
                sets.push(self.ids_where(INDEX_KEY_PRINCIPAL, principal));
            }
            if let Some(days) = window.and_then(window_days) {
                let mut in_window = BTreeSet::new();
                for day in days {
                    in_window.extend(self.ids_where(INDEX_KEY_DAY, &day.to_string()));
                }
                sets.push(in_window);
            }
            sets.sort_by_key(BTreeSet::len);
            let mut sets = sets.into_iter();
            let ids = match sets.next() {
                Some(first) => sets.fold(first, |kept, next| {
                    kept.intersection(&next).cloned().collect()
                }),
                None => self.ids_where("type", SKILL_INVOCATION_CLASS),
            };
            ids.iter()
                .filter_map(|id| self.visible_row(id))
                .filter(|row| class_of(row) == Some(SKILL_INVOCATION_CLASS))
                .filter_map(|row| from_row(&row))
                .filter(|row| row.tenant_id == tenant)
                .filter(|row| window.is_none_or(|window| window.contains(row.started_ms)))
                .collect()
        }

        /// The run trace of `run_id` in `tenant` the caller may see: the first by id.
        fn run_trace_of(&self, tenant: &str, run_id: &str) -> Option<String> {
            self.run_trace_where(tenant, run_id, None)
        }

        /// The run trace of `run_id` in `tenant` that the caller may see and that `owner` owns:
        /// the first by id. A trace with no owner, or another owner, is not found, so a run that
        /// is absent and a run of another principal look the same.
        fn run_trace_owned_by(&self, tenant: &str, run_id: &str, owner: &str) -> Option<String> {
            self.run_trace_where(tenant, run_id, Some(owner))
        }

        fn run_trace_where(
            &self,
            tenant: &str,
            run_id: &str,
            owner: Option<&str>,
        ) -> Option<String> {
            self.ids_where("run_id", run_id).into_iter().find(|id| {
                self.visible_row(id).is_some_and(|row| {
                    let same_tenant = ["tenant", "tenant_id"]
                        .iter()
                        .filter_map(|key| row.get(*key).and_then(Value::as_str))
                        .all(|found| found == tenant);
                    let owned = owner.is_none_or(|owner| {
                        ["_owner", "_owner_id"]
                            .iter()
                            .filter_map(|key| row.get(*key).and_then(Value::as_str))
                            .any(|found| found == owner)
                    });
                    row.get("node_type").and_then(Value::as_str) == Some(RUN_TRACE)
                        && same_tenant
                        && owned
                })
            })
        }
    }

    fn ingest(
        graph: String,
        index: &InvocationIndex,
        verified: &VerifiedRequestContext,
        events: Vec<Value>,
    ) -> Result<PlannedWrite, String> {
        let tenant = verified.tenant();
        // A person ingests spans of its own sessions only (MEDULLA-IDENTITY-R737): the engine
        // takes the attribution from the verified principal, and a run the caller does not own
        // is refused exactly as a run that does not exist.
        let person = (!verified.is_loop_principal()).then(|| {
            principal_pseudonym(
                tenant,
                &verified.principal_persistence_id(),
                medulla_server_api::knob_config::knobs().skill_pseudonym_salt,
            )
        });
        if events.len() > INGEST_BATCH_MAX {
            return Err(refuse(
                TELEMETRY_BATCH_TOO_LARGE,
                format!(
                    "{} events are more than the {INGEST_BATCH_MAX} one batch holds",
                    events.len()
                ),
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
                    format!(
                        "{} carries an error code outside the closed form",
                        row.invocation_id
                    ),
                ));
            }
            // A person's pseudonym is derived below and never read from the event, so only a
            // service caller has to send a well-formed one.
            let pseudonym_ok = person.is_some()
                || (row.principal_pseudonym.len() == 64
                    && row
                        .principal_pseudonym
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit()));
            if row.invocation_id.is_empty() || row.run_id.is_empty() || !pseudonym_ok {
                return Err(invalid_parameter(
                    "an invocation names its id, its run and, from a service, a 64-hex principal pseudonym",
                ));
            }
            if let Some(own) = &person {
                if index
                    .run_trace_owned_by(tenant, &row.run_id, verified.agent_id())
                    .is_none()
                {
                    return Err(refuse(
                        TELEMETRY_SESSION_UNKNOWN,
                        format!("{} is not a session of the caller", row.run_id),
                    ));
                }
                row.principal_pseudonym = own.clone();
            }
            let skill_known = index.visible_row(&row.skill_ref).is_some_and(|node| {
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
            let node = super::invocation_node_id(tenant, &row.invocation_id);
            if index.exists(&node) || !seen.insert(row.invocation_id.clone()) {
                repeated += 1;
                continue;
            }
            accepted.push(row);
        }
        let ids: Vec<&[u8]> = accepted
            .iter()
            .map(|row| row.invocation_id.as_bytes())
            .collect();
        let receipt = Digest256::framed(b"medulla.skill_telemetry.receipt.v1", &ids)
            .map_err(invalid_parameter)?
            .to_hex();
        let mut operations = Vec::new();
        for row in &accepted {
            let node = super::invocation_node_id(tenant, &row.invocation_id);
            operations.push(json!({"op": "upsert_node", "id": node, "properties": to_row(row)}));
            if let Some(trace) = index.run_trace_of(tenant, &row.run_id) {
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
        index: &InvocationIndex,
        view: &GraphView,
        tenant: &str,
        skill_ref: &str,
        wanted: EvalOutcome,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<TracesAnswer, String> {
        let mut ended: BTreeMap<String, u64> = BTreeMap::new();
        for row in index.invocations(tenant, Some(skill_ref), None, None) {
            if let Some(trace) = index.run_trace_of(tenant, &row.run_id) {
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
        let next_cursor = more
            .then(|| rows.last().map(|(end, id)| format!("{end}:{id}")))
            .flatten();
        Ok(TracesAnswer {
            trace_ids: rows.into_iter().map(|(_, id)| id).collect(),
            next_cursor,
        })
    }
}
