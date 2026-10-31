//! Helpers moved down from the server dispatch layer to their kernel users.

/// The replicated-apply scope carried by every committed native command.
#[cfg(feature = "raft")]
#[derive(Clone, Copy)]
pub(crate) struct ReplicatedApplyScope {
    /// Leader-selected commit time every replica applies.
    pub(crate) committed_at_ms: u64,
    /// Placement epoch of the committing authority.
    pub(crate) placement_epoch: u64,
    /// Fencing token of the committing authority, when present.
    pub(crate) fencing_token: Option<u64>,
    /// Whether this command may bootstrap identity.
    pub(crate) identity_bootstrap: bool,
}

#[cfg(feature = "raft")]
tokio::task_local! {
    /// The replicated-apply scope of the command currently being applied.
    pub(crate) static REPLICATED_APPLY: ReplicatedApplyScope;
}

/// One authoritative clock for a replicated native command. Followers must not
/// sample their local wall clocks while applying the same committed entry.
pub(crate) fn authoritative_now_ms() -> u64 {
    #[cfg(feature = "raft")]
    if let Ok(value) = REPLICATED_APPLY.try_with(|scope| scope.committed_at_ms) {
        return value;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// [`authoritative_now_ms`] in whole seconds.
pub(crate) fn authoritative_now_secs() -> u64 {
    authoritative_now_ms() / 1_000
}

/// True while applying a committed replicated command.
#[cfg(feature = "raft")]
pub(crate) fn is_replicated_apply() -> bool {
    REPLICATED_APPLY.try_with(|_| ()).is_ok()
}

/// Placement epoch and fencing token of the command being applied, if any.
#[cfg(feature = "raft")]
pub(crate) fn replicated_placement_authority() -> Option<(u64, Option<u64>)> {
    REPLICATED_APPLY
        .try_with(|scope| (scope.placement_epoch, scope.fencing_token))
        .ok()
}

/// Without raft there is never a replicated apply.
#[cfg(not(feature = "raft"))]
pub(crate) fn is_replicated_apply() -> bool {
    false
}

/// Without raft there is never a placement authority.
#[cfg(not(feature = "raft"))]
pub(crate) fn replicated_placement_authority() -> Option<(u64, Option<u64>)> {
    None
}
