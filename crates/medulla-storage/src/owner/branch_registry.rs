//! The branch registry owner table (MEDULLA-BRANCH-MERGE-R001).
//!
//! One row per branch, keyed by `medulla_types::branch::BranchRef::registry_key`,
//! valued with the MessagePack `Branch`. A branch copies no graph data, so this
//! one table is the whole layout. `medulla-storage` declares it; the store in
//! `medulla-persistence` only names it (RF-RULING-004).

use redb::TableDefinition;

/// `registry_key -> msgpack(Branch)`.
pub const BRANCH_REGISTRY: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("branch_registry");
