//! MEDULLA-DECISION-ENGINE-R113.2.5: the Agent Library half of the shared-owner commit.
//!
//! A capacity-backed topology decision is published by three owners: the Agent Library holds the
//! decision record and the graph evidence, the capacity ledger holds the lease. This module is
//! the Agent Library participant: the decision-record component revision (with its verbatim
//! body) and the evidence graph revision are written in ONE `open_write` transaction of the
//! Agent Library owner, so those two are never visible apart. The lease is the other
//! participant of the coordinator transaction; this write runs only after the coordinator's
//! durable COMMIT decision.
//!
//! The evidence graph pins the decision record through `synthesis_evidence`. The record is
//! written in the same transaction, so that one pin is checked against the record being written
//! (id, kind and definition digest) instead of being resolved against committed state; every
//! other pin is resolved in the write as a normal graph publish does.

use std::cell::RefCell;

use medulla_agent_types::agent_component::{
    AgentComponentEntry, AgentComponentKind, AgentComponentMutationKind,
};
use medulla_agent_types::agent_graph::{AgentGraphEntry, AgentGraphPublishRequest};
use medulla_agent_types::agent_library::{AgentLibraryLifecycle, AgentLibraryMutationContext};
use medulla_decision_types::decision::{DecisionErrorCode, DecisionRecord};

use super::super::agent_component::{component_tables, AgentComponentWriteResult, ComponentLayer};
use super::super::agent_graph::{graph_tables, GraphLayer};
use super::super::agent_library::{validate_context, AgentLibraryStore};
use super::super::agent_revision::{
    apply_revision_rows, revision_scope, write_revision_with_rows, RevisionWrite,
};
use super::{record_component, refuse, snapshot_check, sources};

/// What one shared commit made visible together on the Agent Library owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyCommitParts {
    /// The decision-record component revision.
    pub record: AgentComponentWriteResult,
    /// The evidence graph the record is pinned by.
    pub evidence_graph_id: String,
    /// The evidence graph's head revision after the commit.
    pub evidence_revision: u64,
}

#[cfg(test)]
thread_local! {
    /// A test commit fails after the decision-record rows and before the evidence rows are
    /// written. Compiled only under `cfg(test)`.
    pub(crate) static FAIL_AFTER_RECORD_ROWS: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

impl AgentLibraryStore {
    /// Commit `record` and its evidence graph in one Agent Library transaction.
    ///
    /// `coordinator_txn` is the coordinator transaction id: an admission fence it holds itself
    /// does not block its own write, any other holder does (`ADMISSION_WRITE_BLOCKED`). A repeat
    /// of a stored record is a no-op replay that returns the stored evidence head.
    pub fn commit_topology_decision(
        &self,
        mut context: AgentLibraryMutationContext,
        record: &DecisionRecord,
        expected_catalog_digest: &str,
        evidence: AgentGraphPublishRequest,
        coordinator_txn: &str,
    ) -> Result<TopologyCommitParts, String> {
        validate_context(self, &context)?;
        if record.tenant_id != context.tenant_id {
            return Err("ACCESS_DENIED: the record belongs to another tenant".to_string());
        }
        if expected_catalog_digest != record.inputs.catalog_digest {
            return Err(refuse(
                DecisionErrorCode::StaleCatalog,
                "the expected catalog is not the record's",
            ));
        }
        evidence.graph.validate()?;
        if evidence.graph.tenant_id != record.tenant_id
            || evidence.context.tenant_id != record.tenant_id
        {
            return Err("ACCESS_DENIED: the evidence graph belongs to another tenant".to_string());
        }
        let pin = evidence.graph.synthesis_evidence.as_ref().ok_or_else(|| {
            refuse(
                DecisionErrorCode::DerivationRejected,
                "the evidence graph does not pin the decision record",
            )
        })?;
        if pin.component_id != record.record_id
            || pin.kind != AgentComponentKind::DecisionRecord
            || pin.definition_digest != record.record_digest
        {
            return Err(refuse(
                DecisionErrorCode::DerivationRejected,
                "the evidence graph pins a different record than the one committed",
            ));
        }
        let graph_id = evidence.graph.graph_id.clone();
        if let Some(existing) = self.current_component(&context.tenant_id, &record.record_id)? {
            let replayed = super::committed_replay(existing, record)?;
            let head = self
                .current_graph(&record.tenant_id, &graph_id)?
                .ok_or_else(|| {
                    "CORRUPT_DECISION_RECORD: the committed record has no evidence graph"
                        .to_string()
                })?;
            return Ok(TopologyCommitParts {
                record: replayed,
                evidence_graph_id: graph_id,
                evidence_revision: head.entry_revision,
            });
        }

        context.expected_revision = Some(0);
        let body = serde_json::to_vec(record).map_err(|error| error.to_string())?;
        let draft = record_component(&context, record);
        let (expected_revision, owner) = revision_scope(self, &context, "decision record")?;
        let graph_expected = evidence.context.expected_revision.ok_or_else(|| {
            "agent graph writes require an explicit expected_revision".to_string()
        })?;
        let txn = self.mutations.open_write(&owner)?;
        let tenant = context.tenant_id.clone();
        let record_id = record.record_id.clone();
        let staged_graph: RefCell<Option<(AgentGraphEntry, Vec<u8>)>> = RefCell::new(None);
        let written = write_revision_with_rows::<ComponentLayer>(
            self,
            txn,
            &owner,
            (
                component_tables(),
                RevisionWrite {
                    context: &context,
                    kind: AgentComponentMutationKind::Publish,
                    record_id: &record.record_id,
                    expected_revision,
                    definition_digest: Some(&record.record_digest),
                },
            ),
            |txn, next_revision, replay_context| {
                snapshot_check(txn, record)?;
                sources::check_sources(txn, record)?;
                // Fences: any admission other than this transaction's own that holds a
                // component the record was decided over, or the evidence graph pins, blocks the
                // whole commit.
                let fenced = record
                    .inputs
                    .candidates
                    .iter()
                    .map(|candidate| candidate.component_id.as_str())
                    .chain(
                        evidence
                            .graph
                            .pinned_components()
                            .into_iter()
                            .map(|dependency| dependency.component_id.as_str()),
                    );
                for component_id in fenced {
                    crate::agent_library::admission::require_unfenced_except(
                        txn,
                        &record.tenant_id,
                        component_id,
                        coordinator_txn,
                    )?;
                }
                // Resolve every pin but the evidence pin (the record is written in this very
                // transaction); the entry itself is built from the full graph.
                let mut resolvable = evidence.graph.clone();
                resolvable.synthesis_evidence = None;
                let heads = txn.open_read_table(medulla_storage::AGENT_GRAPH_HEADS)?;
                let head = heads
                    .get((record.tenant_id.as_str(), graph_id.as_str()))
                    .map_err(|error| error.to_string())?
                    .map(|value| value.value())
                    .unwrap_or(0);
                drop(heads);
                if head != graph_expected {
                    return Err(format!(
                        "agent graph head is at revision {head}, not the expected {graph_expected}"
                    ));
                }
                let composition =
                    self.admit_graph_references_in_write(txn, &record.tenant_id, &resolvable)?;
                let graph_entry = AgentGraphEntry::create(
                    evidence.graph.clone(),
                    composition.total_work,
                    graph_expected + 1,
                    AgentLibraryLifecycle::Published,
                    replay_context.created_at_ms,
                    replay_context.created_at_ms,
                )?;
                let bytes = medulla_storage::encode_bounded(&graph_entry, "agent graph revision")?;
                *staged_graph.borrow_mut() = Some((graph_entry, bytes));
                AgentComponentEntry::create(
                    draft,
                    next_revision,
                    AgentLibraryLifecycle::Published,
                    replay_context.created_at_ms,
                    replay_context.created_at_ms,
                )
            },
            |rows| {
                let mut table = rows.open_table(medulla_storage::DECISION_RECORDS)?;
                table
                    .insert((tenant.as_str(), record_id.as_str()), body.as_slice())
                    .map_err(|error| error.to_string())?;
                drop(table);
                #[cfg(test)]
                if FAIL_AFTER_RECORD_ROWS.with(|flag| flag.get()) {
                    return Err("injected failure after the decision-record rows".to_string());
                }
                let (entry, bytes) = staged_graph
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| "the evidence graph entry was not staged".to_string())?;
                apply_revision_rows::<GraphLayer>(
                    rows,
                    graph_tables(),
                    &evidence.context,
                    graph_expected,
                    &entry,
                    bytes.as_slice(),
                )
            },
        )?;
        Ok(TopologyCommitParts {
            record: written,
            evidence_graph_id: graph_id,
            evidence_revision: graph_expected + 1,
        })
    }
}
