//! Server-side compiler and serialization lane for canonical MutationBatch.
//!
//! Public mutation surfaces lower their already-validated `Method` values here;
//! persistence owns the atomic commit and handlers publish RAM only afterward.
//!
//! This namespace split is transitional organization around the current
//! implementation. It does not claim the target `MutationKernel` extraction.

mod commit;

#[cfg(any(feature = "jobs", all(feature = "raft", feature = "epistemic-tms")))]
pub use commit::commit_internal_graph_methods;
pub use commit::{
    commit_internal_graph_methods_with_nonce, commit_lifecycle, commit_work_item,
    lifecycle_identity_matches, lifecycle_was_committed, lock_txn_coordinator,
    publish_change_envelope_projection, CommitOrigin, InternalGraphCommitRequest,
    LifecycleCommitRequest, WorkItemCommitRequest,
};
#[cfg(feature = "program-optimization")]
pub use commit::{
    commit_program_promotion, resolve_program_promotion_identity, ProgramPromotionRequest,
};
#[cfg(test)]
mod tests;
