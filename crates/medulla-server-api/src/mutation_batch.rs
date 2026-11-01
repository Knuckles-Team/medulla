//! Server-side compiler and serialization lane for canonical MutationBatch.
//!
//! Public mutation surfaces lower their already-validated `Method` values here;
//! persistence owns the atomic commit and handlers publish RAM only afterward.
//!
//! This namespace split is transitional organization around the current
//! implementation. It does not claim the target `MutationKernel` extraction.

mod canonical;
mod commit;
mod compile;
mod digest;
mod terminal_outcome;

#[cfg(feature = "raft")]
pub use canonical::is_work_item_method;
pub use canonical::{
    domain_for, is_capacity_method, is_resource_reservation_method,
    is_resource_reservation_query_method, is_work_item_mutation_method,
};
#[cfg(any(feature = "jobs", all(feature = "raft", feature = "epistemic-tms")))]
pub use commit::commit_internal_graph_methods;
pub use commit::{
    commit_internal_graph_methods_with_nonce, commit_lifecycle, commit_work_item,
    lifecycle_identity_matches, lifecycle_was_committed, lock_graph, lock_txn_coordinator,
    publish_change_envelope_projection, CommitOrigin, InternalGraphCommitRequest,
    LifecycleCommitRequest, WorkItemCommitRequest,
};
#[cfg(feature = "program-optimization")]
pub use commit::{
    commit_program_promotion, resolve_program_promotion_identity, ProgramPromotionRequest,
};
#[cfg(feature = "redb")]
pub use compile::compile_opaque_digest;
#[cfg(feature = "redb")]
pub use compile::compile_opaque_method;
#[cfg(feature = "jobs")]
pub use compile::compile_opaque_method_in_scope;
#[cfg(feature = "query")]
pub use compile::compile_sql_source_batch;
#[cfg(feature = "redb")]
pub use compile::effective_policy_digest;
#[cfg(feature = "redb")]
pub use compile::COMPILED_BATCH_INCARNATION;
pub use compile::{
    authoritative_graph_version, compile_crossmodal, compile_methods, compile_methods_with_outbox,
    CompileBatch,
};
#[cfg(any(
    feature = "raft",
    feature = "blob",
    feature = "tsdb",
    feature = "statechart",
    feature = "jobs",
    feature = "sparql-http"
))]
pub use digest::opaque_request_key;
#[cfg(feature = "redb")]
pub use digest::work_item_batch_identity;
pub use digest::{
    batch_actor, lifecycle_batch_id, opaque_coordinator_key, opaque_idempotency_key_for_context,
    principal_fingerprint,
};

/// The verified coordinates one governed graph-row write is compiled under:
/// the request, the verified caller, the tenant/graph it targets and the
/// placement fence (epoch, graph version, fencing token) it commits behind.
pub struct GraphWriteScope<'a> {
    pub request_id: u64,
    pub graph_name: &'a str,
    pub tenant_scope: &'a str,
    pub verified: &'a crate::auth::VerifiedRequestContext,
    pub graph_version: u64,
    pub placement_epoch: u64,
    pub fencing_token: Option<u64>,
}

impl GraphWriteScope<'_> {
    /// Compile `methods` as one graph-surface batch `batch_id` under this scope.
    pub fn compile(
        &self,
        batch_id: &str,
        methods: Vec<medulla_types::protocol::Method>,
    ) -> Result<medulla_types::mutation_batch::MutationBatch, String> {
        self.compile_with_outbox(batch_id, methods, Vec::new())
    }

    /// Compile graph rows and caller-supplied intents in one canonical batch.
    /// The extra intents are included before the envelope digest is minted.
    pub fn compile_with_outbox(
        &self,
        batch_id: &str,
        methods: Vec<medulla_types::protocol::Method>,
        outbox: Vec<medulla_types::mutation_batch::MutationOutboxIntent>,
    ) -> Result<medulla_types::mutation_batch::MutationBatch, String> {
        let principal = self.verified.principal_persistence_id();
        compile_methods_with_outbox(
            CompileBatch {
                batch_id,
                request_id: self.request_id,
                attempt_nonce: self.verified.attempt_nonce(),
                principal: Some(&principal),
                tenant: self.tenant_scope,
                graph: self.graph_name,
                placement_epoch: self.placement_epoch,
                idempotency_key: self.verified.idempotency_key(),
                expected_graph_version: Some(self.graph_version),
                fencing_token: self.fencing_token,
                created_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
                default_surface: medulla_types::mutation_batch::MutationSurface::Graph,
                authoritative_state: None,
            },
            methods,
            outbox,
        )
    }
}

#[cfg(test)]
mod tests;
