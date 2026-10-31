//! Dispatch re-entry seam (`DispatchApi`): lower crates call back into the method router.

/// Method-router re-entry reachable from crates below `medulla-server`.
pub trait DispatchApi: Send + Sync {}
