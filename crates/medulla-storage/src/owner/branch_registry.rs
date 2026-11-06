//! The branch registry owner table (MEDULLA-BRANCH-MERGE-R001).
//!
//! One row per branch, keyed by `medulla_types::branch::BranchRef::registry_key`,
//! valued with the MessagePack `Branch`. A branch copies no graph data, so this
//! row table plus the R002 commit log is the whole layout. `medulla-storage` declares it; the store in
//! `medulla-persistence` only names it (RF-RULING-004).

use redb::TableDefinition;

/// `registry_key -> msgpack(Branch)`.
pub const BRANCH_REGISTRY: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("branch_registry");

/// MEDULLA-BRANCH-MERGE-R002: the append-only branch commit log,
/// `"<registry_key>#<seq:020>" -> msgpack(StoredBranchCommit)`. The zero-padded
/// sequence keeps one branch's commits in commit order under a range scan.
pub const BRANCH_COMMITS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("branch_commits");
