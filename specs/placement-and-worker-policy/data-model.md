# MEDULLA-PLACEMENT — Data model

Governing plan: [plan.md](plan.md). This file lists the types, vocabulary and settings that the rows own. Each ontology class extends an engine core class through the pack rules of the typed-packs spec.

## Decision types (`crates/medulla-decision-types/src/decision/statistical/placement.rs`)

| Type | Fields or members | Row |
|---|---|---|
| `QuestionKind::Placement` | wire name `placement`, evaluate-only | R001.2 |
| `PlacementTarget` | `InProcess`, `Standalone`, `Compose`, `Helm`, `Swarm`; option id `placement:<name>` | R001.3 |
| `PlacementRequestBody` | `subject: PlacementSubject`, `constraints: PlacementConstraints`, `candidate_targets` | R001.4 |
| `PlacementSubject` | `workload_iri`, `cpu_millis`, `memory_bytes`, `replicas`, `needs_persistent_volume` | R001.4 |
| `PlacementConstraints` | `requires_ha`, `forbidden_targets`, `max_relative_cost`, `capacity_cells`, `priority` | R001.4 |
| `TargetFacts` | `supports_ha`, `supports_volumes`, `max_replicas`, `relative_cost` | R001.5 |
| `PlacementVerdict` | `feasible: Vec<(PlacementTarget, cost)>`, `eliminated: Vec<(PlacementTarget, PlacementReason)>` | R001.6 |
| `HeavyCapability` | `Gpu`, `QuantumSimulation`, `LargeMemory`, `Backtest`, `VideoTranscode` | R004.1 |
| `HostCapability` | `host_iri`, `capability`, `headroom` | R004.1 |
| `RoutingFact` | `job_class`, `required_capability` | R004.1 |
| `RoutingDecision` (new) | `job_id`, `required_capability`, `candidates` (at most 64), `chosen`, `topology_generation` | R004.5 |
| `HostBudget` (new, `placement_plan.rs`) | `host_iri`, `cpu_millis`, `memory_bytes`, `storage_bytes`, `capabilities` | R700.1 |
| `ServiceSpec` (new) | `service_iri`, `cpu_millis`, `memory_bytes`, `storage_bytes`, `required_capability`, `current_host` | R700.1 |
| `PlacementPlan` (new) | `placements`, `moves`, `certificate_digest` | R700.1 |

## Ontology (`crates/medulla-core/ontology/placement.ttl`, new)

| Term | Kind | Meaning |
|---|---|---|
| `placement:Target` | class | One deployment target with its declared facts |
| `placement:supportsHa`, `placement:supportsVolumes` | property | Declared support, one value per target |
| `placement:maxReplicas`, `placement:relativeCost` | property | Non-negative integers, one value per target |

## Engine settings

| Setting | Type | Default and range | Row |
|---|---|---|---|
| `SchedulerConfig.cores_per_mille` | `u16` | 360, at most 1000 | R002.2 |
| `SchedulerConfig.memory_per_worker_bytes` | `u64` | 3 GiB, nonzero | R002.2 |
| `SchedulerConfig.max_queue_depth` | `u64` | set by the operator | R002.2 |
| `SchedulerConfig.analytics_workers` (new) | `u32` | 1, range [1, 32] | R002.14 |
| `jobs.schedule_poll_s` (new) | seconds | 1, range [1, 60] | R751.753 |
| `placement.node_budget` (knob) | nodes | 200,000, range [1,000, 5,000,000] | R700.2 |

## Worker and job records

- `WorkerRecord{worker_id, cores, memory_bytes, lanes_supported, last_heartbeat_ms, expires_at_ms}` in `crates/medulla-work-types/src/worker_registry.rs` (R002.1).
- `WorkerSlotGrant` and `WorkerGrant{slots, reserved_lane}` in `crates/medulla-work-types/src/worker_registry/grants.rs` (R002.3, R002.4).
- `PriorityClass` and `ClaimPacer` in `crates/medulla-resource/src/admission.rs` (R750).
- `ScheduleSpec`, `ScheduleTick`, `TickLedger`, `DaemonLeadership`, `ResourceBudget`, `AllocationSlice` and `ReleaseChannel` in `crates/medulla-jobs/src/schedule.rs` (R751).

## Client types (`medulla/placement.py`)

`TenantPool` (bounded LRU of per-tenant clients), `Breaker` (`closed`, `open`, `half_open`), `PlacementDirectory`, `PlacementMetrics` and `PlacementStaleError` (R611).

## Host capability types

`HostCapabilities { host_iri, capabilities, headroom, declared_at_ms }` (`crates/medulla-work-types/src/host_capabilities.rs`, new), the closed enum `JobCapability` with `JOB_CAPABILITY_STRINGS` (`crates/medulla-work-types/src/job_capability.rs`, new) and the `host_iri` field of `WorkerRecord` (`crates/medulla-work-types/src/worker_registry.rs`) (R004.3). One `HostCapabilities` node per host lives in the scheduler system graph.

## State machine types

- Rows `MEDULLA-PLACEMENT-R780`, `MEDULLA-PLACEMENT-R781`, `MEDULLA-PLACEMENT-R782`, `MEDULLA-PLACEMENT-R783`: each machine type stores one state field and exposes one `apply` (or `step`) function that returns `TransitionRefused` for an illegal transition.
- Deleted fields: the dual-truth flags and literals named in the rows (for example `terminal`, `tombstone`, `reclaimed`, `cancel_requested`, `active`, `revoked`, `destination_installed`).

### State machine fields

- `LaneHoldMachine`. States: `Absent`, `Active`, `Released`, `Expired`, `CleanupPending`, `Cleaned`. Persisted state: `state` (new field of the hold record, enum `HoldState`, `crates/medulla-result-types/src/epistemic_operations/development_state.rs:23`). `cleanup_hold_id` is data and never a flag.
- `ResourceReservationMachine`. States: `Absent`, `Reserved`, `Released`, `Reclaimed`, `Expired`, `Superseded`. Persisted state: `state` (enum `ResourceReservationRecordState`, `crates/medulla-result-types/src/epistemic_operations/resource_core.rs:32`). `tombstone` is deleted and `is_terminal` is derived.
- `CapacityLeaseMachine`. States: `Active`, `Released`, `Reclaimed`. Persisted state: `status` (enum `CapacityLeaseState`, `crates/medulla-mutation-types/src/capacity_lease.rs:195`). `renewed_count` is data. `effective_status(now)` derives the Expired view.
- `ControlLeaseMachine`. States: `Active`, `Consumed`, `Revoked`, `Expired`. Persisted state: `status` (enum `ControlLeaseStatus`, `crates/medulla-work-types/src/control_lease.rs`). `Expired` is the stored sweeper result. `effective_status(now)` derives it for reads before the sweep.
- `MembershipShrinkMachine`. States: `Proposed`, `DrainRequested`, `Drained`, `LearnerCaughtUp`, `LeadershipTransferred`, `SafetyChecked`, `RemovalCommitted`, `Completed`, `Aborted`. Persisted state: `phase` (enum `MembershipShrinkPhase`, `crates/medulla-raft/src/raft/membership_shrink.rs:26`), persisted in the shrink journal. `terminal()` is derived.
