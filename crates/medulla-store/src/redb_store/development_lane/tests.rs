// Test partitions are modules so KISS measures each bounded unit rather
// than an include-expanded aggregate. Shared fixture helpers are re-exported
// below without changing the production module API.
use super::super::{decode_durable, resource_decode, resource_encode, DurableCrypto, NODES};
use super::super::{property_string, read_one_node};
use super::finish::ACTIVE_HOLD_REQUIRES_TERMINAL_WORK_ITEM;
use super::identity::{hold_load, method_name};
use super::lifecycle_shared::observation_fresh;
use super::links::checkpoint_lifecycle_status_matches;
use super::quota::{
    adjust, empty_charge, global_policy_equal, hold_id, load_counter, load_policy, scope_key,
    worktree_key, Metric,
};
use super::reserve_support::{policy_pressure, pressure_max, reserve_counter_check};
use super::results::{load_invocation, store_invocation, REDACTED_PRIVATE_ID};
use super::rows::{clear_native_graph_rows, lane_op_id};
use super::types::{LaneDecision, Scope, ScopeCounter};
use super::validation::request_digest;
use super::{
    commit_development_lane, read_development_lane, read_development_lane_status,
    validate_checkpoint_lane_links, DurableLaneCounter, DurableLaneHold, DurableLaneInvocation,
    DurableLanePolicy, COUNTERS, GLOBAL_POLICY_KEY, HOLDS, INVOCATIONS, LANE_INDEX, MAX_DISK_BYTES,
    MAX_INVOCATIONS_PER_TENANT, POLICIES, PRESSURE_INDEX, REPOSITORY_BRANCH_INDEX, WORKTREE_INDEX,
    WORK_ITEM_INDEX,
};
use crate::redb_store::shard::Shard;
use crate::redb_store::shard::ShardWrite;
use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_mutation_types::mutation_scope::IncarnationId;
use medulla_mutation_types::mutation_scope::LogicalName;
use medulla_mutation_types::mutation_scope::MutationOutboxIntent;
use medulla_mutation_types::mutation_scope::MutationScopeIdentity;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_mutation_types::mutation_scope::ScopeTenantId;
use medulla_mutation_types::mutation_scope::MUTATION_BATCH_VERSION;
use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_protocol_types::mutation_batch::MutationBatchCommit;
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::mutation_batch::VersionExpectation;
use medulla_protocol_types::protocol::Method;
use medulla_result_types::epistemic_operations::DevelopmentLaneCleanupCompleteRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneCleanupCompleteRequestSchemaVersion;
use medulla_result_types::epistemic_operations::DevelopmentLaneCleanupCompleteResult;
use medulla_result_types::epistemic_operations::DevelopmentLaneCleanupCompleteResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneFinishRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneFinishRequestSchemaVersion;
use medulla_result_types::epistemic_operations::DevelopmentLaneFinishRequestTerminalState;
use medulla_result_types::epistemic_operations::DevelopmentLaneFinishResult;
use medulla_result_types::epistemic_operations::DevelopmentLaneFinishResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneHold;
use medulla_result_types::epistemic_operations::DevelopmentLaneHoldHostTargetKind;
use medulla_result_types::epistemic_operations::DevelopmentLaneHoldState;
use medulla_result_types::epistemic_operations::DevelopmentLaneIntent;
use medulla_result_types::epistemic_operations::DevelopmentLaneIntentSchemaVersion;
use medulla_result_types::epistemic_operations::DevelopmentLaneObserveRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneObserveRequestSchemaVersion;
use medulla_result_types::epistemic_operations::DevelopmentLaneObserveResult;
use medulla_result_types::epistemic_operations::DevelopmentLaneObserveResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneQueryRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneQueryResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneQuotaPolicy;
use medulla_result_types::epistemic_operations::DevelopmentLaneQuotaUpdateRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneQuotaUpdateResult;
use medulla_result_types::epistemic_operations::DevelopmentLaneQuotaUpdateResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneRenewRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneRenewRequestSchemaVersion;
use medulla_result_types::epistemic_operations::DevelopmentLaneRenewResult;
use medulla_result_types::epistemic_operations::DevelopmentLaneRenewResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneReserveRequest;
use medulla_result_types::epistemic_operations::DevelopmentLaneReserveRequestSchemaVersion;
use medulla_result_types::epistemic_operations::DevelopmentLaneResult;
use medulla_result_types::epistemic_operations::DevelopmentLaneResultDecision;
use medulla_result_types::epistemic_operations::DevelopmentLaneStatusRequest;
use medulla_result_types::epistemic_operations::ResourceCapacitySnapshot;
use medulla_result_types::epistemic_operations::ResourceRequirement;
use medulla_result_types::epistemic_operations::ResourceReservationRecord;
use medulla_result_types::epistemic_operations::ResourceReservationRecordState;
use medulla_result_types::epistemic_operations::ResourceReservationRecordTargetKind;
use medulla_result_types::epistemic_operations::ResourceTargetSnapshot;
use medulla_result_types::epistemic_operations::ResourceTargetSnapshotKind;
use medulla_storage::{GraphShardOwner, ScopedRead};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

#[path = "test_fixture.rs"]
mod test_fixture;
#[path = "test_helpers.rs"]
mod test_helpers;
#[path = "test_seed.rs"]
mod test_seed;

use test_fixture::*;
use test_helpers::*;
use test_seed::*;

#[path = "test_cleanup_a.rs"]
mod test_cleanup_a;
#[path = "test_cleanup_b.rs"]
mod test_cleanup_b;
#[path = "test_finish_a.rs"]
mod test_finish_a;
#[path = "test_finish_b.rs"]
mod test_finish_b;
#[path = "test_finish_c.rs"]
mod test_finish_c;
#[path = "test_lifecycle_a.rs"]
mod test_lifecycle_a;
#[path = "test_lifecycle_b.rs"]
mod test_lifecycle_b;
#[path = "test_links_current.rs"]
mod test_links_current;
#[path = "test_observe.rs"]
mod test_observe;
#[path = "test_reserve_a.rs"]
mod test_reserve_a;
#[path = "test_reserve_b.rs"]
mod test_reserve_b;
#[path = "test_status_a.rs"]
mod test_status_a;
#[path = "test_status_b.rs"]
mod test_status_b;
#[path = "test_status_c.rs"]
mod test_status_c;
