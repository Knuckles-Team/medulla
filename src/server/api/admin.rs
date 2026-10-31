//! Admin and decide handler seams (`AdminApi`, `DecideApi`).

/// Admin handler behaviour reachable from crates below `medulla-h-admin`.
pub trait AdminApi: Send + Sync {}

/// Decide handler behaviour reachable from crates below `medulla-h-decide`.
pub trait DecideApi: Send + Sync {}
