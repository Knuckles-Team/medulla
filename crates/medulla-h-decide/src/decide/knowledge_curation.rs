//! `Method::KnowledgeCuration`: the scheduled curation cycle that turns flags, traces,
//! corrections, failure patterns and source changes into non-contradicting knowledge updates
//! (MEDULLA-DECISION-ENGINE-R709.3 to R709.7).
//!
//! The curator is a worker agent that runs on a cadence under the loop lease of R653. The
//! engine never calls a model: `Inputs` serves what the curator reads, `Propose` checks one
//! proposal for contradiction and records it as an evaluated revision, `Status` closes the
//! flags of an adopted proposal and `Metrics` measures the closed loop. The handler commits
//! nothing to the graph itself: it answers a [`PlannedWrite`] the router commits through the
//! graph gateway under the caller's verified context. The revision, the proposal record and the
//! proposed body go to the decision store in one owner write before the plan is answered; each is
//! keyed by the proposal id, so a replay writes nothing twice. A refusal writes nothing and
//! leaves every flag `Open`.

use medulla_decision_types::knowledge_curation::CurationOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::planned_write::PlannedWrite;
use super::SharedState;

/// The scope every curation op needs.
pub const SCOPE_CURATE: &str = "knowledge:curate";

/// Plan one `KnowledgeCuration` operation for the verified caller.
pub async fn handle_knowledge_curation(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: CurationOp,
) -> Result<PlannedWrite, String> {
    #[cfg(all(feature = "decide", feature = "redb"))]
    {
        served::plan(state, verified, op).await
    }
    #[cfg(not(all(feature = "decide", feature = "redb")))]
    {
        let _ = (state, verified, op);
        Err("KnowledgeCuration requires the `decide` and `redb` features".to_string())
    }
}

#[cfg(all(feature = "decide", feature = "redb"))]
mod served {
    use std::collections::BTreeSet;

    use medulla_decision_outcome_types::decision::promotion::RevisionStage;
    use medulla_decision_outcome_types::decision::revision_list::RevisionRow;
    use medulla_decision_stats::decision::knowledge_curation::{
        check_contradictions, is_due, loop_metrics, FlagMeasure, Invariant, KnowledgeIndex,
    };
    use medulla_decision_types::decision::held_out::RevisionSubject;
    use medulla_decision_types::knowledge_curation::{
        ContradictionFinding, CorrectionRef, CurationAdmitted, CurationEditOp, CurationInputs,
        CurationProposal, CurationStatus, FlagStateRow, PatternRef, SourceChangeRef, TraceRef,
        CURATION_BODY_MAX_BYTES, CURATION_EDIT_TOO_LARGE, CURATION_FLAG_UNKNOWN,
        CURATION_LEASE_REQUIRED, CURATION_NOT_DUE, CURATION_PAGE_MAX, CURATION_PROPOSAL_CLASS,
        CURATION_PROPOSAL_NOT_FOUND, CURATION_ROLE_REQUIRED,
    };
    use medulla_decision_types::knowledge_flag::{FlagState, KnowledgeFlag, KnowledgeKind};
    use medulla_graph_core::graph::GraphView;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        KnowledgeCurationInputs, KnowledgeCurationMetrics, KnowledgeCurationPropose,
        KnowledgeCurationStatus,
    };
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_types::contract::Digest256;
    use serde::{Deserialize, Serialize};
    use serde_json::{json, Map, Value};

    use super::{CurationOp, SCOPE_CURATE};
    use crate::decide::knowledge_flag::served::{all_flags, edge_operation, flag_by_id};
    use crate::decide::planned_write::PlannedWrite;
    use crate::decide::stat_support::{filtered_view_versioned, invalid_parameter};
    use crate::decide::SharedState;

    /// The proposal record key prefix of the decision store.
    const PROPOSAL_PREFIX: &str = "knowledge-proposal:";
    /// The proposed body key prefix.
    const BODY_PREFIX: &str = "knowledge-body:";
    /// The cycle marker key prefix: one row per `Propose`, ordered by time.
    const CYCLE_PREFIX: &str = "knowledge-curation-cycle:";
    /// The most rows one store scan reads.
    const SCAN_LIMIT: usize = 100_000;
    /// Proposal node to each flag it addresses.
    const ADDRESSES: &str = "addresses";
    /// Proposal node to each trace the curator read.
    const READ: &str = "read";
    /// Proposal node to the knowledge node it proposes to edit.
    const CURATION_PROPOSES: &str = "curationProposes";

    /// What the store keeps about one admitted proposal.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct ProposalRecord {
        proposal: CurationProposal,
        revision_key: String,
        revision_id: String,
        receipt_id: String,
        created_ms: u64,
    }

    fn refuse(
        code: medulla_types::refusal_code::RefusalCode,
        detail: impl std::fmt::Display,
    ) -> String {
        format!("{}: {detail}", code.as_str())
    }

    pub(super) async fn plan(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: CurationOp,
    ) -> Result<PlannedWrite, String> {
        if !verified.allows_action(SCOPE_CURATE) {
            return Err(refuse(
                CURATION_ROLE_REQUIRED,
                format!("curation needs the scope {SCOPE_CURATE}"),
            ));
        }
        let graph = op.graph().to_string();
        let (view, _) = filtered_view_versioned(state, verified.agent_id(), &graph).await?;
        let store = state.write().await.ensure_agent_library()?;
        let tenant = verified.tenant();
        match op {
            CurationOp::Inputs {
                loop_id,
                epoch,
                force,
                cursor,
                limit,
                ..
            } => {
                require_lease(state, verified, &loop_id, epoch).await?;
                let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
                let cadence = medulla_server_api::knob_config::knobs().curation_cadence_seconds;
                let due = is_due(last_cycle_ms(&store, tenant)?, now_ms, cadence);
                if !due && !force {
                    return Err(refuse(
                        CURATION_NOT_DUE,
                        "the curation cadence has not elapsed (pass force to read anyway)",
                    ));
                }
                let inputs = inputs_of(&view, tenant, cursor.as_deref(), limit, due)?;
                Ok(read_only(
                    graph,
                    ResultPayload::of::<KnowledgeCurationInputs>(inputs)?,
                ))
            }
            CurationOp::Propose {
                loop_id,
                epoch,
                proposal,
                body,
                ..
            } => {
                require_lease(state, verified, &loop_id, epoch).await?;
                propose(graph, &view, verified, &store, *proposal, body)
            }
            CurationOp::Status { proposal_id, .. } => {
                status(graph, &view, verified, &store, &proposal_id)
            }
            CurationOp::Metrics { from_ms, to_ms, .. } => {
                let measures: Vec<FlagMeasure> = flag_rows(&view, tenant)
                    .into_iter()
                    .map(|(flag, row)| FlagMeasure {
                        raised_at_ms: flag.raised_at_ms,
                        state: flag.state,
                        reopened: flag.reopened,
                        curated_times: row
                            .get("curated_times")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        resolved_at_ms: row.get("resolved_at_ms").and_then(Value::as_u64),
                    })
                    .collect();
                Ok(read_only(
                    graph,
                    ResultPayload::of::<KnowledgeCurationMetrics>(loop_metrics(
                        &measures, from_ms, to_ms,
                    ))?,
                ))
            }
        }
    }

    fn read_only(graph: String, reply: ResultPayload) -> PlannedWrite {
        PlannedWrite {
            graph,
            operations: Vec::new(),
            reply,
        }
    }

    async fn require_lease(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        loop_id: &str,
        epoch: u64,
    ) -> Result<(), String> {
        if crate::decide::loops::holds_live_lease(state, verified, loop_id, epoch).await {
            Ok(())
        } else {
            Err(refuse(
                CURATION_LEASE_REQUIRED,
                format!("the caller holds no live lease of loop {loop_id} at epoch {epoch}"),
            ))
        }
    }

    /// When the last curation cycle ran, from the newest cycle marker.
    fn last_cycle_ms(
        store: &medulla_persistence_records::agent_library::AgentLibraryStore,
        tenant: &str,
    ) -> Result<Option<u64>, String> {
        Ok(store
            .decision_artifacts_with_prefix(tenant, CYCLE_PREFIX, SCAN_LIMIT)?
            .into_iter()
            .filter_map(|(key, _)| {
                key.strip_prefix(CYCLE_PREFIX)?
                    .split(':')
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .max())
    }

    /// Each flag of the caller's tenant with its stored row.
    fn flag_rows(view: &GraphView, tenant: &str) -> Vec<(KnowledgeFlag, Map<String, Value>)> {
        all_flags(view, tenant)
            .into_iter()
            .filter_map(|flag| {
                let row = view.node_row_object(&flag.flag_id)?;
                Some((flag, row))
            })
            .collect()
    }

    fn class_of(row: &Map<String, Value>) -> Option<&str> {
        ["type", "node_type"]
            .iter()
            .find_map(|key| row.get(*key).and_then(Value::as_str))
    }

    fn owned_by(row: &Map<String, Value>, tenant: &str) -> bool {
        ["tenant", "tenant_id"]
            .iter()
            .filter_map(|key| row.get(*key).and_then(Value::as_str))
            .all(|owner| owner == tenant)
    }

    /// The nodes of one class the caller sees, in id order.
    fn nodes_of_class(
        view: &GraphView,
        tenant: &str,
        class: &str,
    ) -> Vec<(String, Map<String, Value>)> {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| Some((id.clone(), view.node_row_object(id)?)))
            .filter(|(_, row)| class_of(row) == Some(class) && owned_by(row, tenant))
            .collect()
    }

    /// The first node `source` links to with `relationship`.
    fn linked(view: &GraphView, source: &str, relationship: &str) -> Option<String> {
        let mut targets: Vec<String> = view
            .edge_properties
            .iter()
            .filter(|((from, _), _)| from == source)
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
            .map(|((_, to), _)| to.clone())
            .collect();
        targets.sort();
        targets.into_iter().next()
    }

    fn inputs_of(
        view: &GraphView,
        tenant: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        due: bool,
    ) -> Result<CurationInputs, String> {
        let page = limit.map_or(CURATION_PAGE_MAX, |n| {
            (n as usize).clamp(1, CURATION_PAGE_MAX)
        });
        let skip: usize = match cursor {
            Some(text) => text
                .parse()
                .map_err(|_| invalid_parameter("cursor is not a position of this read"))?,
            None => 0,
        };
        let mut open: Vec<KnowledgeFlag> = all_flags(view, tenant)
            .into_iter()
            .filter(|flag| flag.state == FlagState::Open)
            .collect();
        open.sort_by(|a, b| {
            a.raised_at_ms
                .cmp(&b.raised_at_ms)
                .then_with(|| a.flag_id.cmp(&b.flag_id))
        });
        let total = open.len();
        let flags: Vec<KnowledgeFlag> = open.into_iter().skip(skip).take(page).collect();
        let next_cursor = (skip + flags.len() < total).then(|| (skip + flags.len()).to_string());
        let mut seen = BTreeSet::new();
        let traces: Vec<TraceRef> = flags
            .iter()
            .filter_map(|flag| {
                let node = linked(view, &flag.flag_id, "knowledgeObservedIn")?;
                let row = view.node_row_object(&node)?;
                seen.insert(node.clone()).then(|| TraceRef {
                    trace_node_id: node,
                    run_id: row
                        .get("run_id")
                        .and_then(Value::as_str)
                        .unwrap_or(&flag.run_id)
                        .to_string(),
                })
            })
            .collect();
        let corrections = nodes_of_class(view, tenant, "Correction")
            .into_iter()
            .map(|(id, row)| CorrectionRef {
                correction_id: row
                    .get("correction_id")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                target: row
                    .get("target")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                correction_node_id: id,
            })
            .take(CURATION_PAGE_MAX)
            .collect();
        let patterns = nodes_of_class(view, tenant, "FailureGap")
            .into_iter()
            .map(|(id, row)| PatternRef {
                signature: row
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                count: row.get("count").and_then(Value::as_u64).unwrap_or(0),
                gap_node_id: id,
            })
            .take(CURATION_PAGE_MAX)
            .collect();
        let source_changes: Vec<SourceChangeRef> = nodes_of_class(view, tenant, "SourceChange")
            .into_iter()
            .map(|(id, row)| SourceChangeRef {
                source_id: row
                    .get("source_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                at_ms: row.get("at_ms").and_then(Value::as_u64).unwrap_or(0),
                change_node_id: id,
            })
            .take(CURATION_PAGE_MAX)
            .collect();
        let watermark_ms = source_changes
            .iter()
            .map(|change| change.at_ms)
            .chain(flags.iter().map(|flag| flag.raised_at_ms))
            .max()
            .unwrap_or(0);
        Ok(CurationInputs {
            flags,
            traces,
            corrections,
            patterns,
            source_changes,
            watermark_ms,
            due,
            next_cursor,
        })
    }

    /// The subject a proposal revises: a skill, or a domain or data-access entry.
    fn subject_of(proposal: &CurationProposal) -> RevisionSubject {
        match proposal.knowledge_kind {
            KnowledgeKind::Skill => RevisionSubject::Skill {
                skill_ref: proposal.target_ref.clone(),
                tier: None,
            },
            KnowledgeKind::Domain | KnowledgeKind::DataAccess => RevisionSubject::KnowledgeEntry {
                entry_ref: proposal.target_ref.clone(),
            },
        }
    }

    /// What the graph and the store hold about existing knowledge.
    fn knowledge_index(
        view: &GraphView,
        tenant: &str,
        records: &[(ProposalRecord, RevisionStage)],
    ) -> KnowledgeIndex {
        let mut index = KnowledgeIndex::default();
        let mut ordered: Vec<&(ProposalRecord, RevisionStage)> = records.iter().collect();
        ordered.sort_by_key(|(record, _)| record.created_ms);
        for (record, stage) in ordered {
            let target = record.proposal.target_ref.clone();
            let digest = record.proposal.edit.after_digest.clone();
            match (stage, digest) {
                (RevisionStage::Champion, Some(digest)) => {
                    index.current.insert(target.clone(), digest);
                    index.pending.remove(&target);
                }
                (RevisionStage::Evaluated | RevisionStage::Published, Some(digest)) => {
                    index.pending.insert(target, digest);
                }
                _ => {}
            }
        }
        for (id, row) in view
            .node_properties
            .keys()
            .filter_map(|id| Some((id, view.node_row_object(id)?)))
            .filter(|(_, row)| owned_by(row, tenant))
        {
            index.known_refs.insert(id.clone());
            for key in ["name", "iri", "column", "term"] {
                if let Some(text) = row.get(key).and_then(Value::as_str) {
                    index.known_refs.insert(text.to_string());
                }
            }
        }
        index
    }

    /// The invariants of the invariant gate: the `Invariant` nodes of the caller's graph.
    fn invariants(view: &GraphView, tenant: &str) -> Vec<Invariant> {
        nodes_of_class(view, tenant, "Invariant")
            .into_iter()
            .map(|(id, row)| Invariant {
                invariant_id: row
                    .get("invariant_id")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                target_prefix: row
                    .get("target_prefix")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                forbidden_ops: row
                    .get("forbidden_ops")
                    .and_then(Value::as_array)
                    .map(|ops| {
                        ops.iter()
                            .filter_map(|op| {
                                serde_json::from_value::<CurationEditOp>(op.clone()).ok()
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    fn proposal_records(
        store: &medulla_persistence_records::agent_library::AgentLibraryStore,
        tenant: &str,
    ) -> Result<Vec<(ProposalRecord, RevisionStage)>, String> {
        let mut out = Vec::new();
        for (_, bytes) in
            store.decision_artifacts_with_prefix(tenant, PROPOSAL_PREFIX, SCAN_LIMIT)?
        {
            let record: ProposalRecord = decode_artifact(&bytes, "knowledge proposal")?;
            let stage = revision_stage(store, tenant, &record)?;
            out.push((record, stage));
        }
        Ok(out)
    }

    /// The stage of the newest revision row of the proposal's revision id: a champion move
    /// writes a new row of the same id (R805.3), so the row with the latest `created_ms` stands.
    fn revision_stage(
        store: &medulla_persistence_records::agent_library::AgentLibraryStore,
        tenant: &str,
        record: &ProposalRecord,
    ) -> Result<RevisionStage, String> {
        let mut newest: Option<RevisionRow> = None;
        for (_, bytes) in store.decision_artifacts_with_prefix(tenant, "revision:", SCAN_LIMIT)? {
            let row: RevisionRow = decode_artifact(&bytes, "revision")?;
            if row.id == record.revision_id
                && newest
                    .as_ref()
                    .is_none_or(|held| row.created_ms > held.created_ms)
            {
                newest = Some(row);
            }
        }
        newest.map(|row| row.stage).ok_or_else(|| {
            format!(
                "CORRUPT_DECISION_ARTIFACT: revision {} is missing",
                record.revision_id
            )
        })
    }

    fn proposal_node_id(proposal_id: &str) -> String {
        format!("proposal:{proposal_id}")
    }

    fn findings_text(findings: &[ContradictionFinding]) -> String {
        findings
            .iter()
            .map(|finding| match &finding.other_ref {
                Some(other) => format!("{} ({other})", finding.code),
                None => finding.code.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn propose(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        store: &medulla_persistence_records::agent_library::AgentLibraryStore,
        proposal: CurationProposal,
        body: String,
    ) -> Result<PlannedWrite, String> {
        let tenant = verified.tenant();
        if body.len() > CURATION_BODY_MAX_BYTES {
            return Err(refuse(
                CURATION_EDIT_TOO_LARGE,
                format!("the body holds more than {CURATION_BODY_MAX_BYTES} bytes"),
            ));
        }
        if proposal.proposal_id.is_empty() || proposal.flag_ids.is_empty() {
            return Err(invalid_parameter(
                "a proposal names an id and at least one flag",
            ));
        }
        if Digest256::sha256(body.as_bytes()).to_hex() != proposal.edit.body_digest {
            return Err(invalid_parameter(
                "edit.body_digest is not the digest of the body",
            ));
        }
        let record_key = format!("{PROPOSAL_PREFIX}{}", proposal.proposal_id);
        let node = proposal_node_id(&proposal.proposal_id);
        if let Some(bytes) = store.decision_artifact(tenant, &record_key)? {
            // A replay of an admitted proposal answers the stored answer and writes nothing.
            let record: ProposalRecord = decode_artifact(&bytes, "knowledge proposal")?;
            if record.proposal != proposal {
                return Err(invalid_parameter(format!(
                    "proposal {} was admitted with other content",
                    proposal.proposal_id
                )));
            }
            return Ok(read_only(
                graph,
                ResultPayload::of::<KnowledgeCurationPropose>(admitted_of(&record))?,
            ));
        }
        for flag_id in &proposal.flag_ids {
            let open = flag_by_id(view, tenant, flag_id)
                .map(|flag| flag.state == FlagState::Open)
                .unwrap_or(false);
            if !open {
                return Err(refuse(
                    CURATION_FLAG_UNKNOWN,
                    format!("flag {flag_id} is not an open flag of the caller"),
                ));
            }
        }
        let records = proposal_records(store, tenant)?;
        let index = knowledge_index(view, tenant, &records);
        let findings = check_contradictions(&proposal, &index, &invariants(view, tenant));
        if let Some(first) = findings.first() {
            return Err(format!("{}: {}", first.code, findings_text(&findings)));
        }

        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let pointer_key = subject_of(&proposal).pointer_key();
        let (subject_kind, subject_ref) = RevisionRow::subject_of(&pointer_key)
            .ok_or_else(|| invalid_parameter("the target names no revision subject"))?;
        let revision = RevisionRow {
            id: format!("rev:{}", proposal.proposal_id),
            subject_kind,
            subject_ref,
            stage: RevisionStage::Evaluated,
            digest: proposal.edit.body_digest.clone(),
            evaluation_receipt_id: Some(proposal.evidence_digest.clone()),
            created_ms: now_ms,
        };
        let receipt_id = Digest256::framed(
            b"medulla.knowledge_curation.receipt.v1",
            &[
                proposal.proposal_id.as_bytes(),
                proposal.evidence_digest.as_bytes(),
                proposal.edit.body_digest.as_bytes(),
            ],
        )
        .map_err(|error| invalid_parameter(error))?
        .to_hex();
        let record = ProposalRecord {
            revision_key: revision.key(),
            revision_id: revision.id.clone(),
            receipt_id: receipt_id.clone(),
            created_ms: now_ms,
            proposal: proposal.clone(),
        };
        let rows = vec![
            (revision.key(), encode_artifact(&revision)?),
            (record_key, encode_artifact(&record)?),
            (
                format!("{BODY_PREFIX}{}", proposal.edit.body_digest),
                body.into_bytes(),
            ),
            (
                format!("{CYCLE_PREFIX}{now_ms:020}:{}", proposal.proposal_id),
                Vec::new(),
            ),
        ];
        let mut operations = vec![json!({
            "op": "upsert_node",
            "id": node,
            "properties": {
                "type": CURATION_PROPOSAL_CLASS,
                "node_type": CURATION_PROPOSAL_CLASS,
                "tenant_id": tenant,
                "proposal_id": proposal.proposal_id,
                "target_ref": proposal.target_ref,
                "knowledge_kind": proposal.knowledge_kind.stored(),
                "edit_op": serde_json::to_value(proposal.edit.op).unwrap_or(Value::Null),
                "body_digest": proposal.edit.body_digest,
                "evidence_digest": proposal.evidence_digest,
                "revision_id": revision.id,
                "stage": "evaluated",
                "receipt_id": receipt_id,
            },
        })];
        for flag_id in &proposal.flag_ids {
            operations.push(edge_operation(&node, flag_id, ADDRESSES, &receipt_id));
        }
        let mut read: Vec<&String> = proposal
            .rationale_trace_ids
            .iter()
            .filter(|trace| view.node_properties.contains_key(*trace))
            .collect();
        read.sort();
        read.dedup();
        for trace in read {
            operations.push(edge_operation(&node, trace, READ, &receipt_id));
        }
        if let Some(target) = view
            .node_properties
            .contains_key(&proposal.target_ref)
            .then_some(&proposal.target_ref)
        {
            operations.push(edge_operation(
                &node,
                target,
                CURATION_PROPOSES,
                &receipt_id,
            ));
        }
        // The one owner write: revision, record, body and cycle marker. A failure writes none.
        store.put_decision_artifacts(tenant, &rows)?;
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<KnowledgeCurationPropose>(admitted_of(&record))?,
        })
    }

    fn admitted_of(record: &ProposalRecord) -> CurationAdmitted {
        CurationAdmitted {
            proposal_id: record.proposal.proposal_id.clone(),
            revision_id: record.revision_id.clone(),
            stage: "evaluated".to_string(),
            receipt_id: record.receipt_id.clone(),
        }
    }

    fn status(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        store: &medulla_persistence_records::agent_library::AgentLibraryStore,
        proposal_id: &str,
    ) -> Result<PlannedWrite, String> {
        let tenant = verified.tenant();
        let bytes = store
            .decision_artifact(tenant, &format!("{PROPOSAL_PREFIX}{proposal_id}"))?
            .ok_or_else(|| {
                refuse(
                    CURATION_PROPOSAL_NOT_FOUND,
                    format!("no proposal {proposal_id}"),
                )
            })?;
        let record: ProposalRecord = decode_artifact(&bytes, "knowledge proposal")?;
        let stage = revision_stage(store, tenant, &record)?;
        let node = proposal_node_id(proposal_id);
        let mut operations = Vec::new();
        let mut adopted = Vec::new();
        // The adopted revision closes every addressed flag that is still open (R709.5).
        if stage == RevisionStage::Champion {
            for flag_id in &record.proposal.flag_ids {
                let still_open = flag_by_id(view, tenant, flag_id)
                    .map(|flag| flag.state == FlagState::Open)
                    .unwrap_or(false);
                if still_open {
                    operations.extend(crate::decide::knowledge_flag::served::adopt_operations(
                        view, tenant, flag_id, &node,
                    )?);
                    adopted.push(flag_id.clone());
                }
            }
            if !operations.is_empty() {
                operations.push(json!({
                    "op": "upsert_node",
                    "id": node,
                    "properties": {
                        "type": CURATION_PROPOSAL_CLASS,
                        "node_type": CURATION_PROPOSAL_CLASS,
                        "tenant_id": tenant,
                        "proposal_id": proposal_id,
                        "stage": "champion",
                    },
                }));
            }
        }
        let flags = record
            .proposal
            .flag_ids
            .iter()
            .filter_map(|flag_id| {
                let state = if adopted.contains(flag_id) {
                    FlagState::Curated
                } else {
                    flag_by_id(view, tenant, flag_id).ok()?.state
                };
                Some(FlagStateRow {
                    flag_id: flag_id.clone(),
                    state,
                })
            })
            .collect();
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<KnowledgeCurationStatus>(CurationStatus {
                proposal_id: proposal_id.to_string(),
                stage: stage_text(stage).to_string(),
                flags,
                adopted,
            })?,
        })
    }

    fn stage_text(stage: RevisionStage) -> &'static str {
        use medulla_types::transition_refused::MachineState;
        stage.stored()
    }
}
