//! Sealed, engine-owned EH-557 top-up plan. The served route remains closed.
//! Every replica checks the immutable source and replacement intent before
//! entering the graph-shard transaction.

use crate::redb_store::enrichment_budget::RepositoryEnrichmentPolicyRevision;
use medulla_work_types::native_control::EnrichmentBudgetCheckpoint;
use medulla_work_types::native_control::EnrichmentBudgetPark;
use medulla_compute::parser::enrichment_snapshot::EligibleSnapshot;
use medulla_mutation_types::mutation_scope::MutationOutboxRecord;
use medulla_protocol_types::{
    mutation_batch::MutationBatch, mutation_batch::MutationBatchRecord,
    mutation_batch::MutationBatchStatus,
};
use medulla_protocol_types::{
    mutation_batch::RepositoryEnrichmentTopUp, mutation_batch::VersionExpectation,
};
use serde::{Deserialize, Serialize};

pub const TOPIC: &str = "repository.enrichment.pending";
pub const CONSUMER: &str = "repository-enrichment-v1";
const MAX_INTENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrichmentTopUpTransition {
    pub old_delivery: MutationOutboxRecord,
    pub consumer: String,
    pub replacement_batch: MutationBatch,
    pub expected_budget: EnrichmentBudgetCheckpoint,
    pub expected_park: EnrichmentBudgetPark,
    pub replacement_budget: EnrichmentBudgetCheckpoint,
    pub revision: RepositoryEnrichmentPolicyRevision,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingIntent {
    budget_status: PendingStatus,
    snapshot: EligibleSnapshot,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum PendingStatus {
    Unreserved,
}

fn snapshot(payload: &[u8]) -> Result<EligibleSnapshot, String> {
    if payload.len() > MAX_INTENT_BYTES {
        return Err("CONFLICT: enrichment top-up intent exceeds bound".into());
    }
    let pending: PendingIntent = medulla_types::msgpack::decode_bounded(
        payload,
        medulla_types::msgpack::MsgpackLimits::new(MAX_INTENT_BYTES, 100_000, 64),
    )
    .map_err(|_| "CONFLICT: enrichment top-up intent is invalid")?;
    let PendingStatus::Unreserved = pending.budget_status;
    pending
        .snapshot
        .validate()
        .map_err(|_| "CONFLICT: enrichment top-up snapshot is invalid")?;
    Ok(pending.snapshot)
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

struct BudgetValidation<'a>(&'a EnrichmentTopUpTransition);

impl std::ops::Deref for BudgetValidation<'_> {
    type Target = EnrichmentTopUpTransition;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl BudgetValidation<'_> {
    fn revision_is_invalid(&self) -> bool {
        self.expected_budget.schema_version != 1
            || self.expected_park.schema_version != 1
            || self.replacement_budget.schema_version != 1
            || self.revision.schema_version != 1
            || self.revision.sequence == 0
            || self.revision.max_total_units < self.revision.total_budget_units
    }

    fn revision_identity_is_invalid(&self) -> bool {
        !bounded(self.revision.caller_subject.as_deref().unwrap_or_default())
            || !bounded(self.revision.idempotency_key.as_deref().unwrap_or_default())
            || self.revision.verified_action.as_deref()
                != Some("repository:enrichment:budget:top_up")
            || !valid_digest(&self.revision.policy_digest)
            || self.revision.policy_digest == self.expected_park.policy_digest
    }

    fn budget_position_changed(&self) -> bool {
        self.expected_budget.next_index != self.replacement_budget.next_index
            || self.expected_budget.page_number != self.replacement_budget.page_number
            || self.expected_budget.reserved_units != self.replacement_budget.reserved_units
            || self.expected_budget.spent_units != self.replacement_budget.spent_units
            || self.expected_budget.last_page_key != self.replacement_budget.last_page_key
    }

    fn budget_accounting_is_invalid(&self, added: u64) -> bool {
        self.expected_budget.remaining_units.checked_add(added)
            != Some(self.replacement_budget.remaining_units)
            || self
                .expected_budget
                .spent_units
                .checked_add(self.expected_budget.reserved_units)
                .and_then(|spent| spent.checked_add(self.expected_budget.remaining_units))
                != Some(self.expected_budget.total_budget_units)
            || self
                .replacement_budget
                .spent_units
                .checked_add(self.replacement_budget.reserved_units)
                .and_then(|spent| spent.checked_add(self.replacement_budget.remaining_units))
                != Some(self.replacement_budget.total_budget_units)
    }

    fn validate_budget(&self) -> Result<(), String> {
        let added = self
            .replacement_budget
            .total_budget_units
            .checked_sub(self.expected_budget.total_budget_units)
            .filter(|delta| *delta > 0)
            .ok_or("CONFLICT: enrichment top-up must increase total budget")?;
        if self.revision_is_invalid()
            || self.revision_identity_is_invalid()
            || self.budget_position_changed()
            || self.budget_accounting_is_invalid(added)
        {
            return Err("CONFLICT: enrichment top-up budget or revision is invalid".into());
        }
        Ok(())
    }
}

struct AuthorityValidation<'a>(&'a EnrichmentTopUpTransition);

impl std::ops::Deref for AuthorityValidation<'_> {
    type Target = EnrichmentTopUpTransition;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl AuthorityValidation<'_> {
    fn delivery_is_invalid(&self, committed_at_ms: u64) -> bool {
        self.consumer != CONSUMER
            || self.old_delivery.intent.topic != TOPIC
            || self.old_delivery.intent.key != self.expected_budget.source_envelope
            || self
                .old_delivery
                .commit_sequence
                .is_none_or(|sequence| sequence == 0)
            || self.old_delivery.created_at_ms > committed_at_ms
            || !self.old_delivery.intent.headers.is_empty()
    }

    fn park_is_invalid(&self) -> bool {
        self.expected_park.source_envelope != self.expected_budget.source_envelope
            || self.expected_park.snapshot_digest != self.expected_budget.snapshot_digest
            || self.expected_park.next_index != self.expected_budget.next_index
            || self.expected_park.page_number != self.expected_budget.page_number
            || self.expected_park.remaining_units != self.expected_budget.remaining_units
            || self.expected_park.required_units <= self.expected_budget.remaining_units
    }

    fn prior_revision_is_invalid(&self, graph: &str) -> bool {
        self.expected_park.policy_digest
            != self
                .revision
                .prior_policy_digest
                .as_deref()
                .unwrap_or_default()
            || self.revision.graph != graph
            || self.revision.tenant_id != self.expected_budget.tenant_id
            || self.revision.prior_source_envelope.as_deref()
                != Some(self.expected_budget.source_envelope.as_str())
            || self.revision.prior_snapshot_digest.as_deref()
                != Some(self.expected_budget.snapshot_digest.as_str())
    }

    fn replacement_revision_is_invalid(&self) -> bool {
        self.replacement_budget.source_envelope != self.revision.source_envelope
            || self.replacement_budget.snapshot_digest != self.revision.snapshot_digest
            || self.replacement_budget.total_budget_units != self.revision.total_budget_units
    }

    fn validate_authority(&self, graph: &str, committed_at_ms: u64) -> Result<(), String> {
        if self.delivery_is_invalid(committed_at_ms)
            || self.park_is_invalid()
            || self.prior_revision_is_invalid(graph)
            || self.replacement_revision_is_invalid()
            || self.expected_park.parked_at_ms == 0
            || self.expected_park.parked_at_ms > committed_at_ms
        {
            return Err("CONFLICT: enrichment top-up authority does not match source".into());
        }
        Ok(())
    }
}

impl EnrichmentTopUpTransition {
    /// A retry may carry a newer Raft route fence only when the original
    /// parent and full transition receipt are already committed on this
    /// replica. Fresh writes must match their sealed parent fence exactly.
    pub fn validate_parent_fence(
        &self,
        route_epoch: u64,
        route_token: Option<u64>,
        prior: Option<&MutationBatchRecord>,
        committed_at_ms: u64,
    ) -> Result<(), String> {
        if let Some(record) = prior {
            record.validate()?;
            let proposed = rmp_serde::to_vec_named(&self.replacement_batch)
                .map_err(|_| "CONFLICT: enrichment top-up parent encode failed")?;
            let recorded = rmp_serde::to_vec_named(&record.batch)
                .map_err(|_| "CONFLICT: enrichment top-up receipt encode failed")?;
            let sealed_receipt = rmp_serde::to_vec_named(self)
                .map_err(|_| "CONFLICT: enrichment top-up transition encode failed")?;
            if record.status != MutationBatchStatus::Committed
                || proposed != recorded
                || record.result_msgpack.as_deref() != Some(sealed_receipt.as_slice())
                || record.committed_at_ms != committed_at_ms
            {
                return Err("IDEMPOTENCY_CONFLICT: enrichment top-up prior receipt differs".into());
            }
            return Ok(());
        }
        if route_epoch != self.replacement_batch.placement_epoch
            || route_token != self.replacement_batch.fencing_token
        {
            return Err("STALE_ROUTE: enrichment top-up fresh parent fence changed".into());
        }
        Ok(())
    }

    fn parent_batch_is_invalid(&self, committed_at_ms: u64) -> bool {
        !self.replacement_batch.is_repository_enrichment_top_up()
            || self.replacement_batch.identity != self.old_delivery.identity
            || self.replacement_batch.batch_id != self.revision.source_envelope
            || self.replacement_batch.idempotency_key()
                != self.revision.idempotency_key.as_deref().unwrap_or_default()
            || self.replacement_batch.created_at_ms != committed_at_ms
    }

    fn validate_parent_batch(&self, committed_at_ms: u64) -> Result<(), String> {
        if self.parent_batch_is_invalid(committed_at_ms) {
            return Err("CONFLICT: enrichment top-up parent batch differs".into());
        }
        let VersionExpectation::Graph(graph_version) = self.replacement_batch.version_expectation
        else {
            return Err("CONFLICT: enrichment top-up parent graph version is absent".into());
        };
        let expected_parent =
            MutationBatch::repository_enrichment_top_up(RepositoryEnrichmentTopUp {
                old_delivery: &self.old_delivery,
                policy_sequence: self.revision.sequence,
                revision_idempotency_key: self
                    .revision
                    .idempotency_key
                    .as_deref()
                    .unwrap_or_default(),
                replacement_intent: self.replacement_batch.outbox[0].clone(),
                replacement_batch_id: &self.revision.source_envelope,
                serving_principal: self.replacement_batch.serving_principal(),
                graph_version,
                placement_epoch: self.replacement_batch.placement_epoch,
                fencing_token: self
                    .replacement_batch
                    .fencing_token
                    .ok_or("CONFLICT: enrichment top-up parent fence is absent")?,
                created_at_ms: committed_at_ms,
            })?;
        let encoded_parent = rmp_serde::to_vec_named(&self.replacement_batch)
            .map_err(|_| "CONFLICT: enrichment top-up parent encode failed")?;
        let encoded_expected = rmp_serde::to_vec_named(&expected_parent)
            .map_err(|_| "CONFLICT: enrichment top-up expected parent encode failed")?;
        if encoded_parent != encoded_expected {
            return Err("CONFLICT: enrichment top-up parent authority changed".into());
        }
        Ok(())
    }

    fn old_snapshot_identity_is_invalid(&self, old: &EligibleSnapshot, graph: &str) -> bool {
        old.units
            .get(self.expected_budget.next_index as usize)
            .is_none_or(|unit| unit.compute_units != self.expected_park.required_units)
            || old.tenant_id != self.revision.tenant_id
            || old.graph != graph
            || old.repository_id != self.revision.repository_id
    }

    fn old_snapshot_budget_is_invalid(&self, old: &EligibleSnapshot) -> bool {
        old.source_envelope != self.expected_budget.source_envelope
            || old.policy_digest != self.expected_park.policy_digest
            || old.budget_units != self.expected_budget.total_budget_units
            || old.digest().ok().as_deref() != Some(self.expected_budget.snapshot_digest.as_str())
    }

    fn next_snapshot_is_invalid(&self, next: &EligibleSnapshot) -> bool {
        next.source_envelope != self.revision.source_envelope
            || next.policy_digest != self.revision.policy_digest
            || next.budget_units != self.revision.total_budget_units
            || next.digest().ok().as_deref() != Some(self.revision.snapshot_digest.as_str())
    }

    pub fn validate(&self, graph: &str, committed_at_ms: u64) -> Result<(), String> {
        BudgetValidation(self).validate_budget()?;
        AuthorityValidation(self).validate_authority(graph, committed_at_ms)?;
        self.validate_parent_batch(committed_at_ms)?;
        let replacement_intent = &self.replacement_batch.outbox[0];
        if self
            .old_delivery
            .identity
            .scope()
            .graph_name()
            .is_none_or(|name| name.as_str() != graph)
            || replacement_intent.topic != TOPIC
            || replacement_intent.key != self.revision.source_envelope
        {
            return Err("CONFLICT: enrichment top-up replacement intent is invalid".into());
        }
        let old = snapshot(&self.old_delivery.intent.payload)?;
        let next = snapshot(&replacement_intent.payload)?;
        if self.old_snapshot_identity_is_invalid(&old, graph)
            || self.old_snapshot_budget_is_invalid(&old)
            || self.next_snapshot_is_invalid(&next)
        {
            return Err("CONFLICT: enrichment top-up snapshot authority changed".into());
        }
        let mut expected_next = old;
        expected_next.source_envelope = self.revision.source_envelope.clone();
        expected_next.policy_digest = self.revision.policy_digest.clone();
        expected_next.budget_units = self.revision.total_budget_units;
        if next != expected_next {
            return Err("CONFLICT: enrichment top-up changed eligible source units".into());
        }
        Ok(())
    }
}
