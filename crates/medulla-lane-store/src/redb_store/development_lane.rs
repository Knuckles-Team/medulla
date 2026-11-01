//! Native development-lane hold and quota authority (RMDD-28).
//!
//! This module deliberately stops at the redb transaction boundary.  It owns
//! the durable lane hold, identity indexes, quota policy/counters, tombstones,
//! and invocation replay rows.  Server dispatch, capability checks, audit/CDC,
//! ReadIndex/Raft routing, and the guarded filesystem effect are separate
//! follow-up seams.  In particular, `worktree_locator` is validated as a
//! managed relative locator but this module never touches the filesystem.

use medulla_result_types::epistemic_operations::DevelopmentLaneHold;
use medulla_result_types::epistemic_operations::DevelopmentLaneQuotaPolicy;
use redb::TableDefinition;
use serde::{Deserialize, Serialize};

const MAX_TEXT: usize = 512;
const MAX_FINGERPRINT: usize = 67;
const MAX_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
pub const MAX_DISK_BYTES: u64 = 1 << 50;
const MAX_COUNT: u64 = 1 << 32;
const MAX_STATUS_LIMIT: u64 = 100;
const MAX_STATUS_SCAN: usize = 512;
pub const MAX_INVOCATIONS_PER_TENANT: usize = 256;
const MAX_INVOCATION_REPAIR_SCAN: usize = 4_096;
pub const GLOBAL_POLICY_KEY: &str = "*";
const WORK_ITEM_METADATA_KEY: &str = "metadata";
const REPOSITORY_WORK_ITEM_EXTENSION_KEY: &str = "repository_work_item";
const LANE_INTENT_EXTENSION_KEY: &str = "development_lane_intent";
const LANE_CLEANUP_EXTENSION_KEY: &str = "development_lane_cleanup";

// The only global-policy route is the typed quota-update request with this
// frozen sentinel tenant. Server capability auth must authorize it as an
// administrator; ordinary tenant updates cannot mutate graph-wide controls.

/// Durable hold identity, keyed `(graph, hold_id)`.
pub const HOLDS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("development_lane_holds");
/// Tenant keyset used only for bounded status pagination.  It remains after
/// cleanup so a terminal hold is still discoverable without a full-table scan.
pub const TENANT_INDEX: TableDefinition<(&str, &str, &str), &str> =
    TableDefinition::new("development_lane_tenant_index");
/// Immutable lane identity `(graph, tenant, lane_id) -> hold_id`.
///
/// The tenant is part of the key, not merely a value checked after lookup:
/// two tenants may intentionally use the same opaque lane id without
/// contending for one another's hold.
pub const LANE_INDEX: TableDefinition<(&str, &str, &str), &str> =
    TableDefinition::new("development_lane_lane_index");
/// Repository/branch exclusivity `(tenant, repository, branch) -> hold_id`.
pub const REPOSITORY_BRANCH_INDEX: TableDefinition<(&str, &str, &str), &str> =
    TableDefinition::new("development_lane_repository_branch_index");
/// Managed worktree locator exclusivity `(host, workspace, locator) -> hold_id`.
/// The key is graph-wide so two tenants cannot claim the same host path.
pub const WORKTREE_INDEX: TableDefinition<(&str, &str), &str> =
    TableDefinition::new("development_lane_worktree_index");
/// One allocation winner per WorkItem attempt.
pub const WORK_ITEM_INDEX: TableDefinition<(&str, &str, u64), &str> =
    TableDefinition::new("development_lane_work_item_index");
/// Exact maintained count/predicted/retained counters by scope.
pub const COUNTERS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("development_lane_counters");
/// Sorted pressure index `(graph, tenant, scope, metric, value, counter_key)`.
/// The final counter key makes equal values unique; reading the last row for a
/// scope/metric is an O(1) exact maximum, so quota-policy CAS never scans live
/// holds or counter rows.
pub const PRESSURE_INDEX: TableDefinition<(&str, &str, &str, &str, u64, &str), u8> =
    TableDefinition::new("development_lane_pressure_index");
/// Server-owned policy and its monotonic numeric revision, keyed by tenant.
pub const POLICIES: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("development_lane_policies");
/// Mutation invocation replay/input-conflict rows `(graph, tenant, key)`.
pub const INVOCATIONS: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("development_lane_invocations");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableLaneHold {
    pub hold: DevelopmentLaneHold,
    /// Monotonic observation revision is kept beside the generated public row;
    /// the v1 DTO intentionally exposes the resulting footprint, not internal
    /// observation bookkeeping.
    pub observation_revision: u64,
    pub last_observed_at_ms: Option<u64>,
    pub terminal_state: Option<String>,
    pub terminal_expected_hold_revision: Option<u64>,
    pub cleanup_removal_proof_ref: Option<String>,
    pub cleanup_expected_hold_revision: Option<u64>,
    /// The WorkItem tuple that authorized the terminal transition.  A cancel
    /// transition deliberately advances the WorkItem lease epoch/fencing
    /// token; retaining this pre-terminal tuple lets a lost acknowledgement
    /// retry the already-atomic lane finish without borrowing a new fence.
    #[serde(default)]
    pub terminal_source_attempt: Option<u64>,
    #[serde(default)]
    pub terminal_source_lease_epoch: Option<u64>,
    #[serde(default)]
    pub terminal_source_fencing_token: Option<u64>,
    #[serde(default)]
    pub terminal_source_work_item_fence: Option<String>,
    pub resource_reservation_id: String,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableLanePolicy {
    pub policy: DevelopmentLaneQuotaPolicy,
    pub policy_revision: u64,
    pub global_policy_revision: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DurableLaneCounter {
    pub active_count: u64,
    pub predicted_disk_bytes: u64,
    pub observed_disk_bytes: u64,
    pub retained_disk_bytes: u64,
    pub revision: u64,
    pub policy_revision: u64,
    /// Global counters are shared by every tenant. Their CAS revision is
    /// separate from tenant policy revisions.
    pub global_policy_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableLaneInvocation {
    pub method: String,
    pub request_digest: String,
    pub result: Vec<u8>,
}

// Cohesive implementation partitions keep the durable lane authority in one
// private namespace while giving each responsibility its own source unit.
#[path = "development_lane/cleanup.rs"]
mod cleanup;
#[path = "development_lane/dispatch.rs"]
mod dispatch;
#[path = "development_lane/finish.rs"]
pub mod finish;
#[path = "development_lane/identity.rs"]
pub mod identity;
#[path = "development_lane/invocation.rs"]
mod invocation;
#[path = "development_lane/lifecycle_shared.rs"]
pub mod lifecycle_shared;
#[path = "development_lane/links.rs"]
pub mod links;
#[path = "development_lane/quota.rs"]
pub mod quota;
#[path = "development_lane/renew_observe.rs"]
mod renew_observe;
#[path = "development_lane/reserve.rs"]
mod reserve;
#[path = "development_lane/reserve_support.rs"]
pub mod reserve_support;
#[path = "development_lane/results.rs"]
pub mod results;
#[path = "development_lane/rows.rs"]
pub mod rows;
#[path = "development_lane/status.rs"]
pub mod status;
#[path = "development_lane/types.rs"]
pub mod types;
#[path = "development_lane/validation.rs"]
pub mod validation;
#[path = "development_lane/work_item.rs"]
pub mod work_item;

// These crate-visible entry points retain the flat development_lane path
// used by redb_store and its sibling modules.
#[cfg(any(test, feature = "server"))]
pub use dispatch::{commit_development_lane, read_development_lane};
pub use finish::transition_work_item_terminal_hold;
pub use links::{
    validate_checkpoint_lane_links, validate_current_lane_links_in_wtx, validate_lane_links_in_wtx,
};
pub use rows::{
    clear_native_graph_rows_in_wtx, clear_native_graph_rows_in_wtx_with_lane_tables,
    retire_graph_rows,
};
#[cfg(any(test, feature = "server"))]
pub use status::read_development_lane_status;

/// Open the five development-lane tables inside one admitted write.
#[allow(clippy::type_complexity)]
pub fn open_native_operation_lane_tables<'txn>(
    write: &'txn medulla_store::redb_store::shard::ShardWrite<'txn>,
    graph_fname: &str,
) -> Result<
    (
        medulla_storage::ScopedOwnerTableMut<'txn, (&'static str, &'static str), &'static [u8]>,
        medulla_storage::ScopedOwnerTableMut<'txn, (&'static str, &'static str, u64), &'static str>,
        medulla_storage::ScopedOwnerTableMut<'txn, (&'static str, &'static str), &'static [u8]>,
        medulla_storage::ScopedOwnerTableMut<
            'txn,
            (
                &'static str,
                &'static str,
                &'static str,
                &'static str,
                u64,
                &'static str,
            ),
            u8,
        >,
        medulla_storage::ScopedOwnerTableMut<'txn, (&'static str, &'static str), &'static [u8]>,
    ),
    String,
> {
    let lane_holds = write
        .graph(graph_fname)?
        .open_scoped_table(HOLDS)
        .map_err(|e| e.to_string())?;
    let lane_work_item_index = write
        .graph(graph_fname)?
        .open_scoped_table(WORK_ITEM_INDEX)
        .map_err(|e| e.to_string())?;
    let lane_counters = write
        .graph(graph_fname)?
        .open_scoped_table(COUNTERS)
        .map_err(|e| e.to_string())?;
    let lane_pressure_index = write
        .graph(graph_fname)?
        .open_scoped_table(PRESSURE_INDEX)
        .map_err(|e| e.to_string())?;
    let lane_policies = write
        .graph(graph_fname)?
        .open_scoped_table(POLICIES)
        .map_err(|e| e.to_string())?;
    Ok((
        lane_holds,
        lane_work_item_index,
        lane_counters,
        lane_pressure_index,
        lane_policies,
    ))
}
