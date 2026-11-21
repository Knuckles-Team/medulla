//! Conflict-decision seam (`ConflictDecider`, MEDULLA-DECISION-ENGINE-R828).
//!
//! The `BranchMerge` handler (`merge_conflicts`, MEDULLA-BRANCH-MERGE-R005) lives in the graph
//! handler crate and the `Decide` handler in the decision handler crate; a handler crate never
//! calls a sibling, so the graph side receives the decision through this trait. `medulla-h-decide`
//! implements it and the server installs that implementation on the `conflict` slot of the
//! [`super::registry::SeamRegistry`] at startup.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalDecisionRecord};

use super::reentry::SeamFuture;
use super::registry::SeamRegistry;
use crate::authority_context::VerifiedRequestContext;
use crate::state::ServerState;

/// Refusal when no conflict decider is installed in this server build.
pub const CONFLICT_DECIDER_MISSING: &str =
    "UNAVAILABLE: the conflict decider is not installed in this server build";

/// Decides one branch-merge conflict: a `Decide` with `QuestionKind::ResolveConflict` run under
/// the caller's verified context. The decision writes nothing; the caller writes the resolution.
pub trait ConflictDecider: Send + Sync {
    /// Decide `request` and return its sealed record: the winner, or the abstention and its code.
    fn decide_conflict<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        verified: &'a VerifiedRequestContext,
        request: DecideRequest,
    ) -> SeamFuture<'a, Result<StatisticalDecisionRecord, String>>;
}

/// The installed conflict decider, if any.
pub async fn conflict_decider(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn ConflictDecider>> {
    state.read().await.seams.conflict.get().cloned()
}

/// Install the conflict decider once; a second install returns the rejected value.
pub fn install_conflict_decider(
    seams: &SeamRegistry,
    decider: Arc<dyn ConflictDecider>,
) -> Result<(), Arc<dyn ConflictDecider>> {
    seams.conflict.set(decider)
}

/// Decide one conflict through the installed decider; a missing installation refuses.
pub async fn decide_conflict(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    request: DecideRequest,
) -> Result<StatisticalDecisionRecord, String> {
    let Some(decider) = conflict_decider(state).await else {
        return Err(CONFLICT_DECIDER_MISSING.to_string());
    };
    decider.decide_conflict(state, verified, request).await
}
