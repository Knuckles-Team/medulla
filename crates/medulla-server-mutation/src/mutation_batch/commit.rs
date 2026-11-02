//! Durable commit, serialization, and serving-projection publication.

mod envelope;
mod internal;
mod lifecycle;
mod work_item;

#[cfg(feature = "program-optimization")]
mod program;

pub use envelope::publish_change_envelope_projection;

/// Which wire request a durable commit answers and the verified principal it is
/// attributed to: the attribution pair every commit-request constructor takes.
#[derive(Clone, Copy)]
pub struct CommitOrigin<'a> {
    pub request_id: u64,
    pub principal: Option<&'a str>,
}

#[cfg(any(
    all(test, feature = "redb", feature = "program-optimization"),
    feature = "jobs",
    all(feature = "raft", feature = "epistemic-tms")
))]
pub use internal::commit_internal_graph_methods;
pub use internal::{
    commit_internal_graph_methods_with_nonce, lock_txn_coordinator,
    InternalGraphCommitRequest,
};
pub use lifecycle::{
    commit_lifecycle, lifecycle_identity_matches, lifecycle_was_committed, LifecycleCommitRequest,
};
#[cfg(test)]
pub(crate) use work_item::changed_work_item_ids;
pub use work_item::{commit_work_item, WorkItemCommitRequest};

#[cfg(test)]
use internal::{apply_projectable_method, install_validated_internal_replay_snapshot};

#[cfg(feature = "program-optimization")]
pub use program::{
    commit_program_promotion, resolve_program_promotion_identity, ProgramPromotionRequest,
};

#[cfg(test)]
use medulla_server_api::batch_compile::compile::compile_methods;
use medulla_server_api::batch_compile::compile::CompileBatch;
#[cfg(test)]
use medulla_graph_core::graph::GraphCore;
use medulla_mutation_types::mutation_scope::MutationSurface;
#[cfg(test)]
use medulla_protocol_types::mutation_batch::MutationStateDescriptor;
#[cfg(test)]
use medulla_protocol_types::protocol::Method;
#[cfg(test)]
use medulla_types::contract::Nonce;
#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
mod internal_replay_tests {
    use super::*;

    /// The opaque serving principal this test's job store is opened as.
    /// `medulla_protocol_types::mutation_batch`'s `validate_serving_principal` requires the
    /// `principal:sha256:<64 hex>` shape for every durable mutation authority,
    /// exactly like `medulla_jobs::dev_scope_grant::DEV_PRINCIPAL`; a human-readable
    /// label is refused by the jobs codec before the store ever opens.
    #[cfg(all(feature = "redb", feature = "program-optimization"))]
    const PROMOTION_TEST_PRINCIPAL: &str =
        "principal:sha256:9f2c1d0e4b7a836512cd94ef0a7b61d3428f5c9e0b13a6d748ff205ce9b374a1";

    #[cfg(all(feature = "redb", feature = "program-optimization"))]
    struct PromotionJobScopeVerifier;

    #[cfg(all(feature = "redb", feature = "program-optimization"))]
    impl medulla_storage::ScopeGrantVerifier for PromotionJobScopeVerifier {
        fn verify(
            &self,
            _physical: &medulla_storage::PhysicalStoreIdentity,
            layout: medulla_storage::OwnerLayout,
            _identity: &medulla_mutation_types::mutation_scope::MutationScopeIdentity,
            principal: &str,
            proof: &[u8],
        ) -> Result<(), String> {
            if layout == medulla_storage::OwnerLayout::Jobs
                && principal == PROMOTION_TEST_PRINCIPAL
                && proof == b"promotion-test-proof"
            {
                Ok(())
            } else {
                Err("promotion test scope authority rejected".to_string())
            }
        }
    }

    #[test]
    fn internal_replay_identity_excludes_attempt_fields_but_binds_key_actor_and_payload() {
        fn batch(
            request_id: u64,
            nonce: u8,
            principal: &str,
            key: &str,
            node: &str,
            source: u64,
        ) -> medulla_protocol_types::mutation_batch::MutationBatch {
            compile_methods(
                CompileBatch {
                    batch_id: "internal-replay",
                    request_id,
                    attempt_nonce: Some(Nonce::from_bytes([nonce; 32])),
                    principal: Some(principal),
                    tenant: "graph-a",
                    graph: "graph-a",
                    placement_epoch: 0,
                    idempotency_key: key,
                    expected_graph_version: Some(source),
                    fencing_token: None,
                    created_at_ms: request_id,
                    default_surface: MutationSurface::Job,
                    authoritative_state: Some(MutationStateDescriptor {
                        algorithm: medulla_kernel::graph_delta::ROW_DELTA_ALGORITHM.to_string(),
                        digest: "0".repeat(64),
                        source_graph_version: source,
                        target_graph_version: source + 1,
                    }),
                },
                vec![Method::RemoveNode {
                    node_id: node.to_string(),
                }],
            )
            .unwrap()
        }
        let identity = |batch: &medulla_protocol_types::mutation_batch::MutationBatch| {
            batch
                .envelope
                .operation()
                .unwrap()
                .operation_identity()
                .unwrap()
                .digest()
                .unwrap()
        };
        let first = batch(7, 1, "caller-a", "internal-replay", "node-a", 3);
        let fresh_attempt = batch(19, 2, "caller-a", "internal-replay", "node-a", 9);
        assert_eq!(identity(&first), identity(&fresh_attempt));
        assert_ne!(
            identity(&first),
            identity(&batch(20, 3, "caller-a", "different", "node-a", 9))
        );
        assert_ne!(
            identity(&first),
            identity(&batch(20, 3, "caller-b", "internal-replay", "node-a", 9))
        );
        assert_ne!(
            identity(&first),
            identity(&batch(20, 3, "caller-a", "internal-replay", "other", 9))
        );
    }

    #[test]
    fn internal_replay_snapshot_is_digest_and_version_bound_before_install() {
        let empty = GraphCore::new().snapshot();
        let serving = GraphCore::from_snapshot(empty.clone(), 3).unwrap();
        let staged = GraphCore::from_snapshot(empty, 3).unwrap();
        apply_projectable_method(
            &staged,
            &Method::AddNode {
                node_id: "node-a".to_string(),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"value": 1}))
                    .unwrap(),
            },
        )
        .unwrap();
        let snapshot = staged.snapshot();
        let delta =
            medulla_kernel::graph_delta::GraphRowDelta::between(&serving.snapshot(), &snapshot)
                .unwrap();
        let descriptor = MutationStateDescriptor {
            algorithm: medulla_kernel::graph_delta::ROW_DELTA_ALGORITHM.to_string(),
            digest: hex::encode(Sha256::digest(delta.to_msgpack().unwrap())),
            source_graph_version: 3,
            target_graph_version: 4,
        };

        let mut wrong_digest = descriptor.clone();
        wrong_digest.digest = "0".repeat(64);
        assert!(install_validated_internal_replay_snapshot(
            &serving,
            snapshot.clone(),
            4,
            &wrong_digest,
        )
        .is_err());
        assert_eq!(serving.version(), 3);
        assert!(install_validated_internal_replay_snapshot(
            &serving,
            snapshot.clone(),
            5,
            &descriptor,
        )
        .is_err());
        assert_eq!(serving.version(), 3);

        install_validated_internal_replay_snapshot(&serving, snapshot, 4, &descriptor).unwrap();
        assert_eq!(serving.version(), 4);
    }
}
