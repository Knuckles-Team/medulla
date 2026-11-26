//! Durable Agent Component publish, retire and pin admission.
//!
//! The public store type remains in the parent module, and the shared
//! [`super::super::agent_revision`] protocol owns replay admission, owner rows,
//! the typed receipt and the native commit. What is component-specific is the
//! entry a publish admits: every component it pins resolved inside the
//! admitting write.

use redb::ReadableTable;

use super::super::agent_library::validate_context;
use super::super::agent_revision::{
    require_context_tenant, retire_revision, revision_scope, write_revision_with_rows,
    RevisionWrite,
};
use super::*;
use crate::connector_pack::commit::{apply_holders, ConnectorPackHolderCommit};

/// The refusal a publish returns when its staged body does not match its pin
/// (MEDULLA-TYPED-PACKS-R002.7).
fn staged_body_invalid(why: &str) -> String {
    format!(
        "{}: {why}",
        medulla_agent_types::agent_component::facts::PROMPT_SCHEMA_INVALID
    )
}

/// A pinned system prompt publishes with exactly the holder its pin names; every
/// other draft publishes with none.
fn check_staged_body(
    request: &AgentComponentPublishRequest,
    expected_revision: u64,
    staged_body: Option<&ConnectorPackHolderCommit>,
) -> Result<(), String> {
    use medulla_agent_types::agent_component::AgentComponentFacts;
    let pin = match &request.component.facts {
        AgentComponentFacts::SystemPrompt {
            body_sha256: Some(hex),
            ..
        } => Some(hex.as_str()),
        _ => None,
    };
    match (pin, staged_body) {
        (None, None) => Ok(()),
        (Some(_), None) => Err(staged_body_invalid(
            "a pinned system prompt publish carries its staged body",
        )),
        (None, Some(_)) => Err(staged_body_invalid(
            "only a pinned system prompt carries a staged body",
        )),
        (Some(hex), Some(holder)) => {
            if holder.body_sha256.to_hex() != hex {
                return Err(staged_body_invalid("the staged body is not the pinned body"));
            }
            if holder.component_id != request.component.component_id {
                return Err(staged_body_invalid(
                    "the staged body names another component",
                ));
            }
            if holder.entry_revision != expected_revision + 1 {
                return Err(staged_body_invalid(
                    "the staged body names another revision",
                ));
            }
            Ok(())
        }
    }
}

fn prepare_component_entry(
    txn: &medulla_transaction::AdmittedMutation<'_, medulla_storage::AgentLibraryOwner>,
    request: &AgentComponentPublishRequest,
    next_revision: u64,
    replay_context: &AgentLibraryMutationContext,
) -> Result<AgentComponentEntry, String> {
    super::super::decision_record::verify_policy_component(&request.component)?;
    super::pins::resolve_component_pins_in_write(
        txn,
        &request.context.tenant_id,
        "agent component",
        &request.component.pinned_components(),
    )?;
    AgentComponentEntry::create(
        request.component.clone(),
        next_revision,
        AgentLibraryLifecycle::Published,
        replay_context.created_at_ms,
        replay_context.created_at_ms,
    )
}

impl AgentLibraryStore {
    /// Publish the next component revision. `staged_body` is the engine body holder
    /// of a pinned system prompt (MEDULLA-TYPED-PACKS-R002.7): it commits in the same
    /// owner write as the revision and head, or not at all. Every other publish
    /// passes `None`.
    pub fn publish_component(
        &self,
        request: AgentComponentPublishRequest,
        staged_body: Option<ConnectorPackHolderCommit>,
    ) -> Result<AgentComponentWriteResult, String> {
        validate_context(self, &request.context)?;
        request.component.validate()?;
        require_context_tenant::<ComponentLayer>(&request.context, &request.component.tenant_id)?;
        super::super::decision_jobs::require_head_receipt(self, &request)?;
        let (expected_revision, owner) =
            revision_scope(self, &request.context, ComponentLayer::NOUN)?;
        check_staged_body(&request, expected_revision, staged_body.as_ref())?;
        let txn = self.mutations.open_write(&owner)?;
        let tenant = request.context.tenant_id.clone();
        write_revision_with_rows::<ComponentLayer>(
            self,
            txn,
            &owner,
            (
                component_tables(),
                RevisionWrite {
                    context: &request.context,
                    kind: AgentComponentMutationKind::Publish,
                    record_id: &request.component.component_id,
                    expected_revision,
                    definition_digest: Some(&request.component.content_digest),
                },
            ),
            |txn, next_revision, replay_context| {
                prepare_component_entry(txn, &request, next_revision, replay_context)
            },
            |rows| apply_holders(rows, &tenant, staged_body.as_slice()),
        )
    }

    /// Record a source-effect receipt on one committed component revision
    /// (MEDULLA-TYPED-PACKS-R094.8).
    ///
    /// The fact lives in the tenant-scoped write-back idempotency table, keyed by
    /// component, revision and receipt digest, so no table or layout changes. The
    /// same receipt again writes nothing; the same digest with other bytes refuses
    /// with `RECEIPT_DIGEST_REUSED`.
    pub fn record_source_effect(
        &self,
        receipt: &medulla_agent_types::agent_component::SourceEffectReceipt,
    ) -> Result<medulla_agent_types::agent_component::SourceEffectRecordResult, String> {
        use medulla_agent_types::agent_component::{
            SourceEffectRecordResult, RECEIPT_DIGEST_REUSED, RECEIPT_REVISION_UNKNOWN,
        };
        receipt.validate()?;
        medulla_agent_types::agent_library::validate_key(
            &receipt.tenant_id,
            &receipt.component_id,
        )?;
        let revision_exists = {
            let read = self.read()?;
            let revisions = read.open_owner_table(medulla_storage::AGENT_COMPONENT_REVISIONS)?;
            let found = revisions
                .get((
                    receipt.tenant_id.as_str(),
                    receipt.component_id.as_str(),
                    receipt.revision,
                ))
                .map_err(|error| error.to_string())?
                .is_some();
            found
        };
        if !revision_exists {
            return Err(format!(
                "{RECEIPT_REVISION_UNKNOWN}: the tenant has no such revision of the component"
            ));
        }
        let encoded = rmp_serde::to_vec_named(receipt).map_err(|error| error.to_string())?;
        let key = receipt.key();
        let mut recorded = false;
        self.maintain_control_rows(
            &receipt.tenant_id,
            "agent_component_source_effect",
            |owner| {
                let mut table = owner.open_table(medulla_storage::WRITE_BACK_IDEMPOTENCY)?;
                let existing = table
                    .get((receipt.tenant_id.as_str(), key.as_str()))
                    .map_err(|error| error.to_string())?
                    .map(|row| row.value().to_vec());
                match existing {
                    Some(stored) if stored == encoded => Ok(()),
                    Some(_) => Err(format!(
                        "{RECEIPT_DIGEST_REUSED}: the receipt digest is recorded with other bytes"
                    )),
                    None => {
                        table
                            .insert((receipt.tenant_id.as_str(), key.as_str()), encoded.as_slice())
                            .map_err(|error| error.to_string())?;
                        recorded = true;
                        Ok(())
                    }
                }
            },
        )?;
        Ok(SourceEffectRecordResult {
            component_id: receipt.component_id.clone(),
            revision: receipt.revision,
            receipt_digest: receipt.receipt_digest.clone(),
            recorded,
        })
    }

    /// Store one optimization proposal against a component revision
    /// (MEDULLA-TYPED-PACKS-R094.6).
    ///
    /// The proposal is a record only: it lives in the tenant-scoped write-back idempotency
    /// table keyed by component, source revision and payload digest, so it never touches the
    /// component head, a revision, a body holder or an outbox event. The same proposal again
    /// writes nothing.
    pub fn store_proposal(
        &self,
        tenant_id: &str,
        component_id: &str,
        proposal: &medulla_agent_types::agent_component::BlueprintProposal,
    ) -> Result<medulla_agent_types::agent_component::BlueprintProposalResult, String> {
        proposal.validate()?;
        medulla_agent_types::agent_library::validate_key(tenant_id, component_id)?;
        let revision_exists = {
            let read = self.read()?;
            let revisions = read.open_owner_table(medulla_storage::AGENT_COMPONENT_REVISIONS)?;
            let found = revisions
                .get((tenant_id, component_id, proposal.source_revision))
                .map_err(|error| error.to_string())?
                .is_some();
            found
        };
        if !revision_exists {
            return Err(
                "COMPONENT_REVISION_UNKNOWN: the tenant has no such source revision of the \
                 component"
                    .to_string(),
            );
        }
        let (source_revision, payload_digest, comparison) = proposal.stored_form();
        let encoded = rmp_serde::to_vec_named(&(source_revision, payload_digest, comparison))
            .map_err(|error| error.to_string())?;
        let key = proposal.key(component_id);
        let mut recorded = false;
        self.maintain_control_rows(tenant_id, "agent_component_proposal", |owner| {
            let mut table = owner.open_table(medulla_storage::WRITE_BACK_IDEMPOTENCY)?;
            let existing = table
                .get((tenant_id, key.as_str()))
                .map_err(|error| error.to_string())?
                .map(|row| row.value().to_vec());
            match existing {
                Some(stored) if stored == encoded => Ok(()),
                Some(_) => Err(
                    "IDEMPOTENCY_CONFLICT: the proposal is recorded with other evidence"
                        .to_string(),
                ),
                None => {
                    table
                        .insert((tenant_id, key.as_str()), encoded.as_slice())
                        .map_err(|error| error.to_string())?;
                    recorded = true;
                    Ok(())
                }
            }
        })?;
        Ok(medulla_agent_types::agent_component::BlueprintProposalResult {
            component_id: component_id.to_string(),
            source_revision,
            payload_digest: payload_digest.to_string(),
            recorded,
        })
    }

    /// Retain the current component revision as a durable tombstone.
    pub fn retire_component(
        &self,
        request: AgentComponentRetireRequest,
    ) -> Result<AgentComponentWriteResult, String> {
        validate_context(self, &request.context)?;
        let (expected_revision, owner) =
            revision_scope(self, &request.context, ComponentLayer::NOUN)?;
        let txn = self.mutations.open_write(&owner)?;
        retire_revision::<ComponentLayer>(
            self,
            txn,
            &owner,
            component_tables(),
            &request.context,
            &request.component_id,
            expected_revision,
        )
    }

    /// Resolve every pinned L1 component reference in the admitting write.
    pub(crate) fn resolve_component_pins_in_write(
        &self,
        write: &medulla_transaction::AdmittedMutation<'_, medulla_storage::AgentLibraryOwner>,
        tenant_id: &str,
        subject: &str,
        pins: &[&medulla_agent_types::agent_component::ComponentDependency],
    ) -> Result<(), String> {
        super::pins::resolve_component_pins_in_write(write, tenant_id, subject, pins)
    }
}
