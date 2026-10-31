//! Query, transaction and wire seams (`QueryApi`, `TxnApi`, `WireApi`).

/// Query handler behaviour reachable from crates below `medulla-h-query`.
pub trait QueryApi: Send + Sync {}

/// Transaction handler behaviour reachable from crates below `medulla-h-txn`.
pub trait TxnApi: Send + Sync {}

/// Wire protocol behaviour reachable from crates below `medulla-wire`.
pub trait WireApi: Send + Sync {}
