//! `Method::KnowledgeFlag`: any agent flags knowledge that is missing or wrong
//! (MEDULLA-DECISION-ENGINE-R708.3).
//!
//! The handler is the single writer of flag state. It reads the caller's row-filtered view of
//! the graph, runs the pure kernels of `medulla_decision_stats::decision::knowledge_flag` and
//! plans one batch: a `Raise` writes one `KnowledgeFlag` node and its edges, a `Resolve`
//! rewrites the node and adds the resolving edges. It commits nothing itself: the answer is a
//! [`PlannedWrite`] the router commits through the graph gateway under the caller's verified
//! context. `Raise` needs scope `knowledge:flag`; `Resolve` needs `knowledge:curate`. A refusal
//! plans nothing.

use medulla_decision_statistical_types::knowledge_flag::KnowledgeFlagOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::planned_write::PlannedWrite;
use super::SharedState;

/// The scope a `Raise` needs.
pub const SCOPE_FLAG: &str = "knowledge:flag";
/// The scope a `Resolve` needs.
pub const SCOPE_CURATE: &str = "knowledge:curate";

/// Plan one `KnowledgeFlag` operation for the verified caller.
pub async fn handle_knowledge_flag(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: KnowledgeFlagOp,
) -> Result<PlannedWrite, String> {
    #[cfg(feature = "decide")]
    {
        served::plan(state, verified, op).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, op);
        Err("KnowledgeFlag requires the `decide` feature".to_string())
    }
}

#[cfg(feature = "decide")]
pub(crate) mod served {
    use std::collections::BTreeMap;

    use medulla_graph_core::graph::GraphView;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        KnowledgeFlagGet, KnowledgeFlagListFlags, KnowledgeFlagRaise, KnowledgeFlagResolve,
    };
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_types::contract::Digest256;
    use medulla_types::transition_refused::MachineState;
    use serde_json::{json, Map, Value};
    use {
        medulla_decision_statistical_types::knowledge_flag::evidence_digest,
        medulla_decision_statistical_types::knowledge_flag::flag_id_of,
        medulla_decision_statistical_types::knowledge_flag::flag_key,
        medulla_decision_statistical_types::knowledge_flag::FlagKind,
        medulla_decision_statistical_types::knowledge_flag::FlagResolution,
        medulla_decision_statistical_types::knowledge_flag::FlagState,
        medulla_decision_statistical_types::knowledge_flag::KnowledgeFlag,
        medulla_decision_statistical_types::knowledge_flag::KnowledgeFlagList,
        medulla_decision_statistical_types::knowledge_flag::KnowledgeFlagOp,
        medulla_decision_statistical_types::knowledge_flag::KnowledgeFlagReceipt,
        medulla_decision_statistical_types::knowledge_flag::KnowledgeFlagView,
        medulla_decision_statistical_types::knowledge_flag::RaiseOutcome,
        medulla_decision_statistical_types::knowledge_flag::EVIDENCE_AGENT_ASSERTED,
        medulla_decision_statistical_types::knowledge_flag::FLAG_LIST_MAX,
        medulla_decision_statistical_types::knowledge_flag::FLAG_NOT_FOUND,
        medulla_decision_statistical_types::knowledge_flag::FLAG_RESOLVE_ROLE_REQUIRED,
        medulla_decision_statistical_types::knowledge_flag::FLAG_RUN_UNKNOWN,
        medulla_decision_statistical_types::knowledge_flag::FLAG_TARGET_UNKNOWN,
        medulla_decision_statistical_types::knowledge_flag::KNOWLEDGE_FLAG_CLASS,
    };
    use {
        medulla_decision_stats_records::decision::knowledge_flag::raise,
        medulla_decision_stats_records::decision::knowledge_flag::resolve,
        medulla_decision_stats_records::decision::knowledge_flag::FlagKnobs,
        medulla_decision_stats_records::decision::knowledge_flag::FlagRefusal,
    };

    use super::{SCOPE_CURATE, SCOPE_FLAG};
    use crate::decide::planned_write::PlannedWrite;
    use crate::decide::stat_support::{filtered_view_versioned, invalid_parameter};
    use crate::decide::SharedState;

    /// The `node_type` of a run's trace node.
    const RUN_TRACE: &str = "RunTrace";
    /// Flag node to the node it flags.
    pub(crate) const FLAGS: &str = "flags";
    /// Flag node to the run trace it was observed in (vocabulary R817).
    pub(crate) const OBSERVED_IN: &str = "knowledgeObservedIn";
    /// Flag node to the principal that raised it.
    pub(crate) const RAISED_BY: &str = "raisedBy";
    /// Flag node to the correction that resolved it.
    pub(crate) const RESOLVED_BY: &str = "knowledgeResolvedBy";
    /// Flag node to the revision that fixed it.
    pub(crate) const FIXED_BY: &str = "knowledgeFixedBy";
    /// The class of a principal node.
    const PRINCIPAL_CLASS: &str = "Principal";
    /// The class of a correction node (R702).
    const CORRECTION_CLASS: &str = "Correction";

    pub(in crate::decide) fn refused(refusal: FlagRefusal) -> String {
        format!("{}: {}", refusal.code.as_str(), refusal.reason)
    }

    pub(in crate::decide) async fn plan(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: KnowledgeFlagOp,
    ) -> Result<PlannedWrite, String> {
        let graph = op.graph().to_string();
        match &op {
            KnowledgeFlagOp::Raise { .. } if !verified.allows_action(SCOPE_FLAG) => {
                return Err(format!("ACCESS_DENIED: Raise needs the scope {SCOPE_FLAG}"));
            }
            KnowledgeFlagOp::Resolve { .. } if !verified.allows_action(SCOPE_CURATE) => {
                return Err(format!(
                    "{}: Resolve needs the scope {SCOPE_CURATE}",
                    FLAG_RESOLVE_ROLE_REQUIRED.as_str()
                ));
            }
            KnowledgeFlagOp::Get { .. } | KnowledgeFlagOp::List { .. }
                if !verified.allows_action(SCOPE_FLAG) && !verified.allows_action(SCOPE_CURATE) =>
            {
                return Err(format!(
                    "ACCESS_DENIED: reading flags needs the scope {SCOPE_FLAG} or {SCOPE_CURATE}"
                ));
            }
            _ => {}
        }
        let (view, _) = filtered_view_versioned(state, verified.agent_id(), &graph).await?;
        match op {
            KnowledgeFlagOp::Raise {
                knowledge_kind,
                flag_kind,
                target_ref,
                what_observed,
                suggested_correction,
                run_id,
                trace_node_id,
                at_ms,
                ..
            } => {
                let candidate = KnowledgeFlag {
                    flag_id: String::new(),
                    tenant_id: verified.tenant().to_string(),
                    knowledge_kind,
                    flag_kind,
                    target_ref,
                    what_observed,
                    suggested_correction,
                    raised_by: verified.principal_persistence_id(),
                    run_id,
                    trace_node_id,
                    raised_at_ms: at_ms,
                    state: FlagState::Open,
                    raise_count: 0,
                    reopened: false,
                };
                let known_run = run_trace_of(&view, verified.tenant(), &candidate.run_id);
                if known_run.is_none()
                    && !open_run(state, verified, &graph, &candidate.run_id).await
                {
                    return Err(format!(
                        "{}: run {} is not an open or recorded run of the caller",
                        FLAG_RUN_UNKNOWN.as_str(),
                        candidate.run_id
                    ));
                }
                do_raise(graph, &view, verified, candidate, known_run)
            }
            KnowledgeFlagOp::Resolve {
                flag_id,
                resolution,
                correction_id,
                fixed_by,
                ..
            } => do_resolve(
                graph,
                &view,
                verified,
                &flag_id,
                resolution,
                correction_id.as_deref(),
                fixed_by.as_deref(),
            ),
            KnowledgeFlagOp::Get { flag_id, .. } => {
                let flag = flag_by_id(&view, verified.tenant(), &flag_id)?;
                Ok(PlannedWrite {
                    graph,
                    operations: Vec::new(),
                    reply: ResultPayload::of::<KnowledgeFlagGet>(view_of(&view, flag))?,
                })
            }
            KnowledgeFlagOp::List {
                state: only, limit, ..
            } => {
                let mut flags = all_flags(&view, verified.tenant());
                flags.retain(|flag| only.is_none_or(|wanted| flag.state == wanted));
                flags.sort_by(|a, b| {
                    b.raised_at_ms
                        .cmp(&a.raised_at_ms)
                        .then_with(|| a.flag_id.cmp(&b.flag_id))
                });
                let cap = limit.map_or(FLAG_LIST_MAX, |n| (n as usize).min(FLAG_LIST_MAX));
                flags.truncate(cap);
                Ok(PlannedWrite {
                    graph,
                    operations: Vec::new(),
                    reply: ResultPayload::of::<KnowledgeFlagListFlags>(KnowledgeFlagList {
                        flags,
                    })?,
                })
            }
        }
    }

    /// Whether the caller holds an open run of that id (a claimed work item of the caller).
    async fn open_run(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        graph: &str,
        run_id: &str,
    ) -> bool {
        #[cfg(feature = "redb")]
        {
            use medulla_persistence::AsRedb;
            let persistence = state.read().await.persistence.clone();
            let Some(redb) = persistence.as_ref().and_then(|p| p.as_redb()) else {
                return false;
            };
            let Ok(actor) =
                medulla_server_api::batch_compile::principal_fingerprint(verified.principal())
            else {
                return false;
            };
            redb.read_run_work_item(graph, verified.tenant(), run_id, &actor)
                .await
                .ok()
                .flatten()
                .is_some()
        }
        #[cfg(not(feature = "redb"))]
        {
            let _ = (state, verified, graph, run_id);
            false
        }
    }

    /// The id of the `RunTrace` node recording `run_id` in `tenant`, when the view holds one.
    pub(in crate::decide) fn run_trace_of(
        view: &GraphView,
        tenant: &str,
        run_id: &str,
    ) -> Option<String> {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        ids.into_iter().find_map(|id| {
            let row = view.node_row_object(id)?;
            let class = row.get("node_type").and_then(Value::as_str);
            let same_run = row.get("run_id").and_then(Value::as_str) == Some(run_id);
            let same_tenant = ["tenant", "tenant_id"]
                .iter()
                .filter_map(|key| row.get(*key).and_then(Value::as_str))
                .all(|owner| owner == tenant);
            (class == Some(RUN_TRACE) && same_run && same_tenant).then(|| id.clone())
        })
    }

    /// The node a `target_ref` names: the whole reference or the part after its prefix is a
    /// node id of the view (a topic names its gap node `gap:<slug>`).
    fn target_node(view: &GraphView, target_ref: &str) -> Option<String> {
        let rest = medulla_decision_statistical_types::knowledge_flag::target_form(target_ref)
            .map(|(_, rest)| rest.to_string())?;
        let candidates = [target_ref.to_string(), rest.clone(), format!("gap:{rest}")];
        candidates
            .into_iter()
            .find(|id| view.node_properties.contains_key(id))
    }

    fn do_raise(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        candidate: KnowledgeFlag,
        run_trace: Option<String>,
    ) -> Result<PlannedWrite, String> {
        let target = target_node(view, &candidate.target_ref);
        if candidate.flag_kind == FlagKind::Wrong && target.is_none() {
            return Err(format!(
                "{}: {} does not resolve in the caller's graph",
                FLAG_TARGET_UNKNOWN.as_str(),
                candidate.target_ref
            ));
        }
        let id = flag_id_of(&flag_key(&candidate));
        let existing = flag_by_id(view, verified.tenant(), &id).ok();
        let run_open = all_flags(view, verified.tenant())
            .iter()
            .filter(|flag| flag.run_id == candidate.run_id && flag.state == FlagState::Open)
            .count() as u64;
        let knobs = FlagKnobs {
            max_open_per_run: medulla_server_api::knob_config::knobs().flag_max_open_per_run,
        };
        let decision = raise(&candidate, existing.as_ref(), run_open, &knobs).map_err(refused)?;
        let flag = decision.flag;
        let receipt_id = receipt_id_of(&flag);
        let mut operations = vec![node_operation(
            &flag,
            &receipt_id,
            view.node_row_object(&flag.flag_id),
            &[],
        )];
        if decision.outcome == RaiseOutcome::Created {
            let principal = format!("principal:{}", flag.raised_by);
            if !view.node_properties.contains_key(&principal) {
                operations.push(json!({
                    "op": "upsert_node",
                    "id": principal,
                    "properties": {"type": PRINCIPAL_CLASS, "node_type": PRINCIPAL_CLASS,
                                   "tenant_id": flag.tenant_id},
                }));
            }
            operations.push(edge_operation(
                &flag.flag_id,
                &principal,
                RAISED_BY,
                &receipt_id,
            ));
            if let Some(target) = target {
                operations.push(edge_operation(&flag.flag_id, &target, FLAGS, &receipt_id));
            }
            let trace = flag
                .trace_node_id
                .clone()
                .filter(|node| view.node_properties.contains_key(node))
                .or(run_trace);
            if let Some(trace) = trace {
                operations.push(edge_operation(
                    &flag.flag_id,
                    &trace,
                    OBSERVED_IN,
                    &receipt_id,
                ));
            }
        }
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<KnowledgeFlagRaise>(KnowledgeFlagReceipt {
                flag,
                outcome: Some(decision.outcome),
                receipt_id,
            })?,
        })
    }

    fn do_resolve(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        flag_id: &str,
        resolution: FlagResolution,
        correction_id: Option<&str>,
        fixed_by: Option<&str>,
    ) -> Result<PlannedWrite, String> {
        let current = flag_by_id(view, verified.tenant(), flag_id)?;
        let flag = resolve(&current, resolution).map_err(refused)?;
        let receipt_id = receipt_id_of(&flag);
        let mut operations = resolution_operations(view, &flag, &receipt_id, resolution);
        if let Some(correction_id) = correction_id {
            let node =
                correction_node(view, verified.tenant(), correction_id).ok_or_else(|| {
                    invalid_parameter(format!(
                        "correction {correction_id} is not in the caller's graph"
                    ))
                })?;
            operations.push(edge_operation(
                &flag.flag_id,
                &node,
                RESOLVED_BY,
                &receipt_id,
            ));
        }
        if let Some(revision) = fixed_by {
            if !view.node_properties.contains_key(revision) {
                return Err(invalid_parameter(format!(
                    "revision {revision} is not in the caller's graph"
                )));
            }
            operations.push(edge_operation(
                &flag.flag_id,
                revision,
                FIXED_BY,
                &receipt_id,
            ));
        }
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<KnowledgeFlagResolve>(KnowledgeFlagReceipt {
                flag,
                outcome: None,
                receipt_id,
            })?,
        })
    }

    /// The node upsert of a flag that left `Open` at the authoritative time: it records when,
    /// and how many times a curation fixed it.
    fn resolution_operations(
        view: &GraphView,
        flag: &KnowledgeFlag,
        receipt_id: &str,
        resolution: FlagResolution,
    ) -> Vec<Value> {
        let existing = view.node_row_object(&flag.flag_id);
        let fixed_before = existing
            .as_ref()
            .and_then(|row| row.get("curated_times").and_then(Value::as_u64))
            .unwrap_or(0);
        let fixed = fixed_before + u64::from(resolution == FlagResolution::Adopt);
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        vec![node_operation(
            flag,
            receipt_id,
            existing,
            &[
                ("resolved_at_ms", json!(now_ms)),
                ("curated_times", json!(fixed)),
            ],
        )]
    }

    /// The operations that move one `Open` flag to `Curated` because the proposal node
    /// `fixed_by` was adopted (R709.5): the node and its `knowledgeFixedBy` edge.
    pub(in crate::decide) fn adopt_operations(
        view: &GraphView,
        tenant: &str,
        flag_id: &str,
        fixed_by: &str,
    ) -> Result<Vec<Value>, String> {
        let current = flag_by_id(view, tenant, flag_id)?;
        let flag = resolve(&current, FlagResolution::Adopt).map_err(refused)?;
        let receipt_id = receipt_id_of(&flag);
        let mut operations = resolution_operations(view, &flag, &receipt_id, FlagResolution::Adopt);
        operations.push(edge_operation(
            &flag.flag_id,
            fixed_by,
            FIXED_BY,
            &receipt_id,
        ));
        Ok(operations)
    }

    /// The correction node of `correction_id` in `tenant`: a `Correction` node whose
    /// `correction_id` property matches, or the node of that id.
    fn correction_node(view: &GraphView, tenant: &str, correction_id: &str) -> Option<String> {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        ids.into_iter().find_map(|id| {
            let row = view.node_row_object(id)?;
            let class = ["type", "node_type", "class"]
                .iter()
                .find_map(|key| row.get(*key).and_then(Value::as_str));
            let named = ["correction_id", "receipt_id"]
                .iter()
                .any(|key| row.get(*key).and_then(Value::as_str) == Some(correction_id))
                || id == correction_id;
            let owned = ["tenant", "tenant_id"]
                .iter()
                .filter_map(|key| row.get(*key).and_then(Value::as_str))
                .all(|owner| owner == tenant);
            (class == Some(CORRECTION_CLASS) && named && owned).then(|| id.clone())
        })
    }

    /// The receipt id every node and edge of one op shares: the digest of the flag id, its state
    /// and its raise count.
    pub(in crate::decide) fn receipt_id_of(flag: &KnowledgeFlag) -> String {
        let count = flag.raise_count.to_string();
        Digest256::framed(
            b"medulla.knowledge_flag.receipt.v1",
            &[
                flag.flag_id.as_bytes(),
                flag.state.stored().as_bytes(),
                count.as_bytes(),
                flag.raised_at_ms.to_string().as_bytes(),
            ],
        )
        .expect("a receipt frames four short fields")
        .to_hex()
    }

    /// The node upsert of `flag`: the stored row of an existing flag keeps the properties the
    /// flag type does not carry, and `extras` are written over it.
    fn node_operation(
        flag: &KnowledgeFlag,
        receipt_id: &str,
        existing: Option<Map<String, Value>>,
        extras: &[(&str, Value)],
    ) -> Value {
        let mut properties = existing.unwrap_or_default();
        if let Ok(Value::Object(map)) = serde_json::to_value(flag) {
            properties.extend(map);
        }
        for (key, value) in extras {
            properties.insert((*key).to_string(), value.clone());
        }
        properties.insert("type".into(), KNOWLEDGE_FLAG_CLASS.into());
        properties.insert("node_type".into(), KNOWLEDGE_FLAG_CLASS.into());
        properties.insert("tenant".into(), flag.tenant_id.clone().into());
        properties.insert("evidence_class".into(), EVIDENCE_AGENT_ASSERTED.into());
        // The machine state as the vocabulary names it (`flagStatus`, MEDULLA-ORCH-VOCAB-R817.4).
        properties.insert("flagStatus".into(), flag.state.stored().into());
        properties.insert(
            "evidence_digest".into(),
            evidence_digest(&flag.what_observed).into(),
        );
        properties.insert("receipt_id".into(), receipt_id.into());
        json!({"op": "upsert_node", "id": flag.flag_id, "properties": properties})
    }

    pub(in crate::decide) fn edge_operation(
        source: &str,
        target: &str,
        relationship: &str,
        receipt_id: &str,
    ) -> Value {
        json!({
            "op": "upsert_edge",
            "source": source,
            "target": target,
            "properties": {"relationship": relationship, "receipt_id": receipt_id},
        })
    }

    /// The flag stored under `flag_id`, as the caller sees it. A flag of another tenant, or one
    /// the caller's view hides, is not found.
    pub(in crate::decide) fn flag_by_id(
        view: &GraphView,
        tenant: &str,
        flag_id: &str,
    ) -> Result<KnowledgeFlag, String> {
        view.node_row_object(flag_id)
            .and_then(|row| flag_of_row(&row))
            .filter(|flag| flag.tenant_id == tenant)
            .ok_or_else(|| format!("{}: no flag {flag_id}", FLAG_NOT_FOUND.as_str()))
    }

    pub(in crate::decide) fn all_flags(view: &GraphView, tenant: &str) -> Vec<KnowledgeFlag> {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| view.node_row_object(id).and_then(|row| flag_of_row(&row)))
            .filter(|flag| flag.tenant_id == tenant)
            .collect()
    }

    /// The flag a stored `KnowledgeFlag` node holds.
    fn flag_of_row(row: &Map<String, Value>) -> Option<KnowledgeFlag> {
        if row.get("type").and_then(Value::as_str) != Some(KNOWLEDGE_FLAG_CLASS) {
            return None;
        }
        let known: BTreeMap<&str, &Value> = [
            "flag_id",
            "tenant_id",
            "knowledge_kind",
            "flag_kind",
            "target_ref",
            "what_observed",
            "suggested_correction",
            "raised_by",
            "run_id",
            "trace_node_id",
            "raised_at_ms",
            "state",
            "raise_count",
            "reopened",
        ]
        .into_iter()
        .filter_map(|key| row.get(key).map(|value| (key, value)))
        .collect();
        serde_json::from_value(json!(known)).ok()
    }

    /// The flag with the nodes it links to.
    fn view_of(view: &GraphView, flag: KnowledgeFlag) -> KnowledgeFlagView {
        let linked = |relationship: &str| -> Option<String> {
            let mut targets: Vec<String> = view
                .edge_properties
                .iter()
                .filter(|((source, _), _)| source == &flag.flag_id)
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
                .map(|((_, target), _)| target.clone())
                .collect();
            targets.sort();
            targets.into_iter().next()
        };
        let evidence_class = view
            .node_row_object(&flag.flag_id)
            .and_then(|row| {
                row.get("evidence_class")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| EVIDENCE_AGENT_ASSERTED.to_string());
        KnowledgeFlagView {
            evidence_class,
            flags: linked(FLAGS),
            observed_in: linked(OBSERVED_IN),
            raised_by: flag.raised_by.clone(),
            resolved_by: linked(RESOLVED_BY),
            fixed_by: linked(FIXED_BY),
            flag,
        }
    }
}
