//! `Method::CorrectionRecord`: human corrections for the Learn stage
//! (MEDULLA-DECISION-ENGINE-R702.3 to R702.6).
//!
//! `Record` turns one [`Correction`] into the correction node, its rule
//! candidate, independent outcome and eval case, the three edges between them
//! and the correction's embedding, all in ONE planned batch: the four nodes,
//! three edges and the vector are all present or all absent. The handler
//! commits nothing itself; it answers a [`PlannedWrite`] that the router
//! commits through the graph gateway under the caller's verified context. It
//! is the single writer of correction state.
//!
//! `Nearest` embeds a target and answers the nearest prior corrections of the
//! caller's tenant. `Get` reads the correction and its effects back by receipt
//! id; a correction of another tenant reads as absent.

use medulla_decision_types::correction::CorrectionOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::planned_write::PlannedWrite;
use super::SharedState;

/// Plan one `CorrectionRecord` operation for the verified caller against the
/// request `graph`.
pub async fn handle_correction_record(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    graph: &str,
    op: CorrectionOp,
) -> Result<PlannedWrite, String> {
    #[cfg(feature = "decide")]
    {
        served::plan(state, verified, graph, op).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, graph, op);
        Err("CorrectionRecord requires the `decide` feature".to_string())
    }
}

#[cfg(feature = "decide")]
mod served {
    use medulla_agent_types::agent_guardrail::{
        check_rule_against_invariants, GuardrailRule, RulePromotionVerdict,
    };
    use medulla_decision_stats::decision::correction::{commit_correction, nearest_corrections};
    use medulla_decision_types::correction::{
        Correction, CorrectionEffect, CorrectionNearest, CorrectionOp, CorrectionOutcome,
        CorrectionRead, CorrectionRecorded, EvalCase, OutcomeTag, RuleCandidate,
        CORRECTION_NOT_FOUND, CORRECTION_ROLE_REQUIRED, CORRECTOR_ROLE, KNOBS,
        OUTCOME_SELF_REPORTED,
    };
    use medulla_decision_types::decision::knobs::KnobValue;
    use medulla_decision_types::decision::DecisionPolicyRef;
    use medulla_graph_core::graph::GraphView;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        CorrectionRecordGet, CorrectionRecordNearest, CorrectionRecordRun,
    };
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use serde_json::{json, Map, Value};

    use crate::decide::planned_write::PlannedWrite;
    use crate::decide::program_embedding::{embed_texts, SpacedEmbedder};
    use crate::decide::stat_support::{
        filtered_view_versioned, invalid_parameter, resolve_policy,
    };
    use crate::decide::stat_vectors::graph_vectors;
    use crate::decide::SharedState;

    /// The class of the correction node.
    const CORRECTION_CLASS: &str = "Correction";
    /// Neighbours `Nearest` probes before it tenant-filters and bounds them.
    const NEAREST_PROBE: usize = 64;
    /// The `type` property of every node this handler writes.
    const TYPE: &str = "type";
    /// The tenant stamp every node this handler writes carries; `Get` and
    /// `Nearest` filter on it.
    const TENANT: &str = "tenant_id";
    /// The relationship property of an edge.
    const RELATIONSHIP: &str = "relationship";

    pub(super) async fn plan(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        graph: &str,
        op: CorrectionOp,
    ) -> Result<PlannedWrite, String> {
        match op {
            CorrectionOp::Record { correction } => record(state, verified, graph, correction).await,
            CorrectionOp::Nearest { target } => nearest(state, verified, graph, &target).await,
            CorrectionOp::Get { receipt_id } => get(state, verified, graph, &receipt_id).await,
        }
    }

    /// `"<target>\n<after>"`, the text a correction is embedded from.
    fn correction_text(target: &str, after: &str) -> String {
        format!("{target}\n{after}")
    }

    async fn record(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        graph: &str,
        correction: Correction,
    ) -> Result<PlannedWrite, String> {
        // Every refusal precedes the first node of the plan, so a refusal
        // commits no effect (MEDULLA-DECISION-ENGINE-R702.4).
        if !verified.roles().iter().any(|role| role == CORRECTOR_ROLE) {
            return Err(format!(
                "{}: recording a correction needs the verified role `{CORRECTOR_ROLE}`",
                CORRECTION_ROLE_REQUIRED.as_str()
            ));
        }
        if correction.actor == correction.produced_by {
            return Err(format!(
                "{}: the actor of a correction cannot be the producer of the corrected output",
                OUTCOME_SELF_REPORTED.as_str()
            ));
        }
        let commit = commit_correction(&correction).map_err(invalid_parameter)?;
        let (store, resolved) = {
            let store = state.write().await.ensure_agent_library()?;
            let resolved = resolve_policy(&store, verified.tenant(), &DecisionPolicyRef::Default)?;
            (store, resolved)
        };
        // The candidate rule passes the same invariant gate a promoted rule
        // passes (R129.3); only this tenant's invariants are read.
        let invariants = store.published_rule_invariants(verified.tenant())?;
        let candidate = GuardrailRule {
            rule_id: commit.effect.rule_candidate.rule_id.clone(),
            applies_to_class: correction.target.clone(),
            source_generation: commit.effect.receipt_id.clone(),
        };
        if let RulePromotionVerdict::RejectedInvariant { invariant_iri } =
            check_rule_against_invariants(&candidate, &invariants)
        {
            return Err(format!(
                "RULE_CONTRADICTS_INVARIANT: the candidate rule contradicts invariant {invariant_iri}"
            ));
        }
        // The vector is embedded, and the graph's space checked, before any
        // operation is planned, so a refusal plans nothing.
        let view = graph_vectors(state, verified.agent_id(), graph).await?;
        let embedder = SpacedEmbedder::configured()?;
        if !view.admits_space(&embedder.space.digest) {
            return Err(invalid_parameter(
                "CORRECTION_EMBEDDING_SPACE_MISMATCH: the graph declares another embedding space \
                 than the configured one",
            ));
        }
        view.ensure_space(&embedder.space)?;
        let text = correction_text(&correction.target, &correction.after);
        let tenant = verified.tenant().to_string();
        let digest = resolved.digest.clone();
        let (mut vectors, _calls) = tokio::task::spawn_blocking(move || {
            embed_texts(&embedder, &tenant, &digest, &[text.as_str()])
        })
        .await
        .map_err(|error| invalid_parameter(format!("embedding task failed: {error}")))??;
        let vector = vectors.remove(0);

        let mut operations: Vec<Value> = Vec::new();
        for node in &commit.nodes {
            let mut properties = node.properties.clone();
            properties.insert(TYPE.into(), node.class.into());
            properties.insert("_owner".into(), verified.agent_id().into());
            properties.insert(TENANT.into(), verified.tenant().into());
            operations.push(json!({
                "op": "upsert_node",
                "id": node.id,
                "properties": properties,
            }));
        }
        for edge in &commit.edges {
            operations.push(json!({
                "op": "upsert_edge",
                "source": edge.source,
                "target": edge.target,
                "properties": {RELATIONSHIP: edge.relationship},
            }));
        }
        operations.push(json!({
            "op": "add_embedding",
            "id": commit.nodes[0].id,
            "embedding": vector,
        }));
        let receipt_id = commit.effect.receipt_id.clone();
        Ok(PlannedWrite {
            graph: graph.to_string(),
            operations,
            reply: ResultPayload::of::<CorrectionRecordRun>(CorrectionRecorded {
                receipt_id,
                effect: commit.effect,
            })?,
        })
    }

    async fn nearest(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        graph: &str,
        target: &str,
    ) -> Result<PlannedWrite, String> {
        let store = state.write().await.ensure_agent_library()?;
        let resolved = resolve_policy(&store, verified.tenant(), &DecisionPolicyRef::Default)?;
        let k = match resolved.policy.knob(&KNOBS[0]) {
            KnobValue::Integer(value) => usize::try_from(value).unwrap_or(1).clamp(1, 16),
            _ => 8,
        };
        let view = graph_vectors(state, verified.agent_id(), graph).await?;
        let embedder = SpacedEmbedder::configured()?;
        if !view.admits_space(&embedder.space.digest) {
            return Err(invalid_parameter(
                "CORRECTION_EMBEDDING_SPACE_MISMATCH: the graph holds another embedding space \
                 than the configured one",
            ));
        }
        let tenant = verified.tenant().to_string();
        let digest = resolved.digest.clone();
        let text = target.to_string();
        let (mut vectors, _calls) = tokio::task::spawn_blocking(move || {
            embed_texts(&embedder, &tenant, &digest, &[text.as_str()])
        })
        .await
        .map_err(|error| invalid_parameter(format!("embedding task failed: {error}")))??;
        let query = vectors.remove(0);
        let class_and_tenant = |id: &str| {
            let node = view.visible_properties(id)?;
            Some((
                node.get(TYPE)?.as_str()?.to_string(),
                node.get(TENANT)?.as_str()?.to_string(),
            ))
        };
        let neighbours = view.probe_filtered(&query, NEAREST_PROBE, |node| {
            node.get(TYPE).and_then(Value::as_str) == Some(CORRECTION_CLASS)
                && node.get(TENANT).and_then(Value::as_str) == Some(verified.tenant())
        });
        let node_ids = nearest_corrections(&neighbours, class_and_tenant, verified.tenant(), k);
        Ok(PlannedWrite {
            graph: graph.to_string(),
            operations: Vec::new(),
            reply: ResultPayload::of::<CorrectionRecordNearest>(CorrectionNearest { node_ids })?,
        })
    }

    async fn get(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        graph: &str,
        receipt_id: &str,
    ) -> Result<PlannedWrite, String> {
        let (view, _) = filtered_view_versioned(state, verified.agent_id(), graph).await?;
        let read = read_correction(&view, verified.tenant(), receipt_id).ok_or_else(|| {
            format!(
                "{}: no correction for receipt {receipt_id}",
                CORRECTION_NOT_FOUND.as_str()
            )
        })?;
        Ok(PlannedWrite {
            graph: graph.to_string(),
            operations: Vec::new(),
            reply: ResultPayload::of::<CorrectionRecordGet>(read)?,
        })
    }

    /// One stored node of `class`, owned by `tenant`, that carries `receipt_id`.
    /// A node of another tenant is the same `None` as an absent one.
    fn row(
        view: &GraphView,
        id: &str,
        class: &str,
        tenant: &str,
        receipt_id: &str,
    ) -> Option<Map<String, Value>> {
        let row = view.node_row_object(id)?;
        let same = row.get(TYPE).and_then(Value::as_str) == Some(class)
            && row.get(TENANT).and_then(Value::as_str) == Some(tenant)
            && row.get("receipt_id").and_then(Value::as_str) == Some(receipt_id);
        same.then_some(row)
    }

    fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
        row.get(key).and_then(Value::as_str).map(str::to_string)
    }

    /// Whether the view holds an edge `source -> target` named `relationship`.
    fn has_edge(view: &GraphView, source: &str, target: &str, relationship: &str) -> bool {
        view.edge_properties
            .get(&(source.to_string(), target.to_string()))
            .is_some_and(|blobs| {
                blobs.iter().any(|blob| {
                    medulla_types::msgpack::decode_property_value(blob.as_slice())
                        .ok()
                        .and_then(|edge| {
                            edge.get(RELATIONSHIP)
                                .and_then(Value::as_str)
                                .map(|name| name == relationship)
                        })
                        .unwrap_or(false)
                })
            })
    }

    /// The correction of `receipt_id` read back from its four nodes and three
    /// edges, or `None` when any is absent, hidden or another tenant's.
    fn read_correction(view: &GraphView, tenant: &str, receipt_id: &str) -> Option<CorrectionRead> {
        let correction_id = format!("correction:{receipt_id}");
        let rule_id = format!("rule:{receipt_id}");
        let outcome_id = format!("outcome:{receipt_id}");
        let case_id = format!("case:{receipt_id}");
        let node = row(view, &correction_id, CORRECTION_CLASS, tenant, receipt_id)?;
        let rule = row(view, &rule_id, "RuleCandidate", tenant, receipt_id)?;
        let outcome = row(view, &outcome_id, "CorrectionOutcome", tenant, receipt_id)?;
        let case = row(view, &case_id, "EvalCase", tenant, receipt_id)?;
        for (target, relationship) in [
            (&rule_id, "yields_rule"),
            (&outcome_id, "yields_outcome"),
            (&case_id, "yields_case"),
        ] {
            if !has_edge(view, &correction_id, target, relationship) {
                return None;
            }
        }
        let correction = Correction {
            correction_id: text(&node, "correction_id")?,
            target: text(&node, "target")?,
            before: text(&node, "before")?,
            after: text(&node, "after")?,
            produced_by: text(&node, "produced_by")?,
            actor: text(&node, "actor")?,
            at_ms: node.get("at_ms")?.as_u64()?,
        };
        let effect = CorrectionEffect {
            receipt_id: receipt_id.to_string(),
            rule_candidate: RuleCandidate {
                rule_id: text(&rule, "rule_id")?,
                text_digest: text(&rule, "text_digest")?,
            },
            outcome: CorrectionOutcome {
                outcome_id: text(&outcome, "outcome_id")?,
                tag: match outcome.get("tag")?.as_str()? {
                    "independent" => OutcomeTag::Independent,
                    _ => return None,
                },
                value: outcome.get("value")?.as_f64()?,
            },
            eval_case: EvalCase {
                case_id: text(&case, "case_id")?,
                input_digest: text(&case, "input_digest")?,
                expected_digest: text(&case, "expected_digest")?,
            },
        };
        Some(CorrectionRead {
            receipt_id: receipt_id.to_string(),
            correction,
            effect,
        })
    }
}
