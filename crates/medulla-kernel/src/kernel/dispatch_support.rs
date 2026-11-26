//! Helpers moved down from the server dispatch layer to their kernel users.

/// The replicated-apply scope carried by every committed native command.
#[cfg(feature = "raft")]
#[derive(Clone, Copy)]
pub struct ReplicatedApplyScope {
    /// Leader-selected commit time every replica applies.
    pub committed_at_ms: u64,
    /// Placement epoch of the committing authority.
    pub placement_epoch: u64,
    /// Fencing token of the committing authority, when present.
    pub fencing_token: Option<u64>,
    /// Whether this command may bootstrap identity.
    pub identity_bootstrap: bool,
}

#[cfg(feature = "raft")]
tokio::task_local! {
    /// The replicated-apply scope of the command currently being applied.
    pub static REPLICATED_APPLY: ReplicatedApplyScope;
}

#[cfg(any(test, feature = "test-support"))]
tokio::task_local! {
    /// Milliseconds a served test has moved the engine clock forward for the task it scopes.
    static ENGINE_CLOCK_OFFSET_MS: u64;
}

/// Run `future` with the engine clock `offset_ms` ahead of the wall clock, so a served test
/// reads a note's age without sleeping. Only the scoped task sees the offset; it exists only
/// in test builds.
#[cfg(any(test, feature = "test-support"))]
pub async fn with_engine_clock_offset<F: std::future::Future>(offset_ms: u64, future: F) -> F::Output {
    ENGINE_CLOCK_OFFSET_MS.scope(offset_ms, future).await
}

/// One authoritative clock for a replicated native command. Followers must not
/// sample their local wall clocks while applying the same committed entry.
pub fn authoritative_now_ms() -> u64 {
    #[cfg(feature = "raft")]
    if let Ok(value) = REPLICATED_APPLY.try_with(|scope| scope.committed_at_ms) {
        return value;
    }
    #[cfg(any(test, feature = "test-support"))]
    let offset_ms = ENGINE_CLOCK_OFFSET_MS.try_with(|offset| *offset).unwrap_or(0);
    #[cfg(not(any(test, feature = "test-support")))]
    let offset_ms = 0;
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
        .saturating_add(offset_ms)
}

/// The engine clock: the authoritative time of the command being served or
/// applied, as one [`medulla_types::clock::Clock`].
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineClock;

impl medulla_types::clock::Clock for EngineClock {
    fn now_ms(&self) -> u64 {
        authoritative_now_ms()
    }
}

/// [`authoritative_now_ms`] in whole seconds.
pub fn authoritative_now_secs() -> u64 {
    authoritative_now_ms() / 1_000
}

/// True while applying a committed replicated command.
#[cfg(feature = "raft")]
pub fn is_replicated_apply() -> bool {
    REPLICATED_APPLY.try_with(|_| ()).is_ok()
}

/// Placement epoch and fencing token of the command being applied, if any.
#[cfg(feature = "raft")]
pub fn replicated_placement_authority() -> Option<(u64, Option<u64>)> {
    REPLICATED_APPLY
        .try_with(|scope| (scope.placement_epoch, scope.fencing_token))
        .ok()
}

/// Without raft there is never a replicated apply.
#[cfg(not(feature = "raft"))]
pub fn is_replicated_apply() -> bool {
    false
}

/// Without raft there is never a placement authority.
#[cfg(not(feature = "raft"))]
pub fn replicated_placement_authority() -> Option<(u64, Option<u64>)> {
    None
}
