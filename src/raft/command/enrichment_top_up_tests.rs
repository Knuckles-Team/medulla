use crate::parser::enrichment_snapshot::{EligibleUnit, EnrichmentWorkStage};
use crate::redb_store::enrichment_budget::RepositoryEnrichmentPolicyRevision;
use crate::redb_store::enrichment_top_up::{EnrichmentTopUpTransition, CONSUMER, TOPIC};
use medulla_agent_types::{
    native_control::EnrichmentBudgetCheckpoint, native_control::EnrichmentBudgetPark,
};
use medulla_protocol_types::mutation_batch::RepositoryEnrichmentTopUp;
use medulla_protocol_types::{
    mutation_batch::MutationBatch, mutation_batch::MutationBatchRecord,
    mutation_batch::MutationBatchStatus,
};
use medulla_types::mutation_scope::CommittedVersion;
use medulla_types::mutation_scope::COMPILED_BATCH_INCARNATION;
use medulla_types::MutationOutboxRecord;
use medulla_types::MutationScopeIdentity;

fn transition() -> EnrichmentTopUpTransition {
    let old = medulla_compute::test_support::repository_enrichment_snapshot(
        "tenant-a",
        "graph-a",
        10,
        vec![EligibleUnit {
            content_digest: format!("sha256:{}", "d".repeat(64)),
            parser_capability_digest: "parser:v1".into(),
            stage: EnrichmentWorkStage::Classical,
            input_ref: format!("cas:sha256:{}", "d".repeat(64)),
            content_length: 1,
            compute_units: 11,
            demanded: false,
        }],
    );
    let old_digest = old.digest().unwrap();
    let mut next = old.clone();
    next.source_envelope = "source-two".into();
    next.policy_digest = "e".repeat(64);
    next.budget_units = 14;
    let next_digest = next.digest().unwrap();
    let identity =
        MutationScopeIdentity::fixed_graph("tenant-a", "graph-a", COMPILED_BATCH_INCARNATION)
            .unwrap();
    let old_delivery = MutationOutboxRecord {
        schema_version: medulla_types::MUTATION_BATCH_VERSION,
        batch_id: "source-one".into(),
        ordinal: 0,
        identity,
        committed_version: CommittedVersion::Graph {
            source: 0,
            target: 1,
        },
        commit_sequence: Some(1),
        intent: super::pending_repository_intent(old, TOPIC),
        created_at_ms: 1,
    };
    let replacement_batch =
        MutationBatch::repository_enrichment_top_up(RepositoryEnrichmentTopUp {
            old_delivery: &old_delivery,
            policy_sequence: 1,
            revision_idempotency_key: "topup-1",
            replacement_intent: super::pending_repository_intent(next, TOPIC),
            replacement_batch_id: "source-two",
            serving_principal: &format!("principal:sha256:{}", "a".repeat(64)),
            graph_version: 1,
            placement_epoch: 1,
            fencing_token: 1,
            created_at_ms: 12,
        })
        .unwrap();
    EnrichmentTopUpTransition {
        old_delivery,
        consumer: CONSUMER.into(),
        replacement_batch,
        expected_budget: EnrichmentBudgetCheckpoint {
            schema_version: 1,
            tenant_id: "tenant-a".into(),
            snapshot_digest: old_digest.clone(),
            source_envelope: "source-one".into(),
            next_index: 0,
            page_number: 0,
            reserved_units: 0,
            spent_units: 0,
            remaining_units: 10,
            total_budget_units: 10,
            last_page_key: String::new(),
        },
        expected_park: EnrichmentBudgetPark {
            schema_version: 1,
            source_envelope: "source-one".into(),
            snapshot_digest: old_digest.clone(),
            next_index: 0,
            page_number: 0,
            remaining_units: 10,
            required_units: 11,
            policy_digest: "a".repeat(64),
            parked_at_ms: 11,
        },
        replacement_budget: EnrichmentBudgetCheckpoint {
            schema_version: 1,
            tenant_id: "tenant-a".into(),
            snapshot_digest: next_digest.clone(),
            source_envelope: "source-two".into(),
            next_index: 0,
            page_number: 0,
            reserved_units: 0,
            spent_units: 0,
            remaining_units: 14,
            total_budget_units: 14,
            last_page_key: String::new(),
        },
        revision: RepositoryEnrichmentPolicyRevision {
            schema_version: 1,
            tenant_id: "tenant-a".into(),
            graph: "graph-a".into(),
            repository_id: "repo".into(),
            source_envelope: "source-two".into(),
            snapshot_digest: next_digest,
            policy_digest: "e".repeat(64),
            total_budget_units: 14,
            max_total_units: 14,
            sequence: 1,
            prior_source_envelope: Some("source-one".into()),
            prior_snapshot_digest: Some(old_digest),
            prior_policy_digest: Some("a".repeat(64)),
            caller_subject: Some("principal:sha256:operator".into()),
            verified_action: Some("repository:enrichment:budget:top_up".into()),
            idempotency_key: Some("topup-1".into()),
        },
    }
}

#[test]
fn sealed_top_up_is_source_bound_and_keeps_plaintext_out_of_log() {
    let plan = transition();
    plan.validate("graph-a", 12).unwrap();
    let command =
        super::ReplicatedMutation::enrichment_top_up(&plan, "graph-a", 12, "secret").unwrap();
    let encoded = rmp_serde::to_vec_named(&command).unwrap();
    assert!(!encoded.windows(10).any(|part| part == b"source-one"));
    assert!(command
        .open_enrichment_top_up("graph-a", 12, "wrong")
        .is_err());
    assert!(command
        .open_enrichment_top_up("other", 12, "secret")
        .is_err());
    assert!(command
        .open_enrichment_top_up("graph-a", 12, "secret")
        .unwrap()
        .is_some());
    let mut forged = plan;
    forged.replacement_batch.outbox[0].key = "another-source".into();
    assert!(forged.validate("graph-a", 12).is_err());
    let mut forged = transition();
    forged.replacement_batch.outbox[0].payload.push(0);
    assert!(forged.validate("graph-a", 12).is_err());
}

#[test]
fn serialized_top_up_reopens_only_at_its_original_graph_and_commit_time() {
    let plan = transition();
    let command =
        super::ReplicatedMutation::enrichment_top_up(&plan, "graph-a", 12, "secret").unwrap();
    // The follower and a restarted leader receive serialized log entries,
    // not the leader's in-memory transition. Verify that the sealed record
    // survives that boundary with its graph and commit-time binding.
    let bytes = rmp_serde::to_vec_named(&command).unwrap();
    let replayed: super::ReplicatedMutation = rmp_serde::from_slice(&bytes).unwrap();
    let reopened = replayed
        .open_enrichment_top_up("graph-a", 12, "secret")
        .unwrap()
        .unwrap();
    reopened.validate("graph-a", 12).unwrap();
    assert!(replayed
        .open_enrichment_top_up("graph-a", 13, "secret")
        .is_err());
    assert!(replayed
        .open_enrichment_top_up("graph-b", 12, "secret")
        .is_err());
}

#[test]
fn only_exact_committed_retry_may_cross_a_new_route_fence() {
    let plan = transition();
    assert!(plan.validate_parent_fence(1, Some(1), None, 12).is_ok());
    assert!(plan.validate_parent_fence(2, Some(2), None, 12).is_err());
    let mut record = medulla_protocol_types::mutation_batch::MutationBatchRecord {
        batch: plan.replacement_batch.clone(),
        identity: plan.replacement_batch.identity.clone(),
        status: MutationBatchStatus::Committed,
        committed_version: CommittedVersion::Graph {
            source: 1,
            target: 2,
        },
        result_msgpack: Some(rmp_serde::to_vec_named(&plan).unwrap()),
        committed_at_ms: 12,
    };
    assert!(plan
        .validate_parent_fence(2, Some(2), Some(&record), 12)
        .is_ok());
    // A follower replay after failover can use the new route only for the
    // byte-identical committed transition. Reusing the batch ID with a
    // changed intent payload must not inherit the old receipt.
    let mut altered = plan.clone();
    altered.replacement_batch.outbox[0].payload.push(0);
    assert!(altered
        .validate_parent_fence(2, Some(2), Some(&record), 12)
        .is_err());
    assert!(plan
        .validate_parent_fence(2, Some(2), Some(&record), 13)
        .is_err());
    record.result_msgpack.as_mut().unwrap().push(0);
    assert!(plan
        .validate_parent_fence(2, Some(2), Some(&record), 12)
        .is_err());
    record.result_msgpack = Some(rmp_serde::to_vec_named(&plan).unwrap());
    record.status = MutationBatchStatus::Aborted;
    assert!(plan
        .validate_parent_fence(2, Some(2), Some(&record), 12)
        .is_err());
}
