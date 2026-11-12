# MEDULLA-OBSERVE — Data model

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## State machines (R004)

Each machine is a `MachineDef` over `medulla_types::transition_refused::transition`. A pair of state and event that no edge lists returns `TRANSITION_REFUSED`. Stored forms are the lower-case state names.

### TaskMachine (R004.1)

File: `crates/medulla-observe/src/machine/task.rs`.

States: Registered (initial), Running, Progressing, Stalled, Completed (terminal), Failed (terminal), Cancelled (terminal). Events: Start, Progress, StallDetected, Finish, Fail, Cancel. Guards: none.

| Edge | From | Event | To |
|---|---|---|---|
| T01 | Registered | Start | Running |
| T02 | Running | Progress | Progressing |
| T03 | Progressing | Progress | Progressing |
| T04 | Running | StallDetected | Stalled |
| T05 | Progressing | StallDetected | Stalled |
| T06 | Stalled | Progress | Progressing |
| T07 | Running | Finish | Completed |
| T08 | Progressing | Finish | Completed |
| T09 | Stalled | Finish | Completed |
| T10 | Registered | Fail | Failed |
| T11 | Running | Fail | Failed |
| T12 | Progressing | Fail | Failed |
| T13 | Stalled | Fail | Failed |
| T14 | Registered | Cancel | Cancelled |
| T15 | Running | Cancel | Cancelled |
| T16 | Progressing | Cancel | Cancelled |
| T17 | Stalled | Cancel | Cancelled |

`Failed` carries `TaskFailure`: `TASK_DROPPED`, `TASK_PANICKED`, `NO_PROGRESS`, or the typed refusal code the job returned.

### ReadinessMachine (R004.2)

File: `crates/medulla-observe/src/machine/readiness.rs`. One instance per graph.

States: Opening (initial), Migrating, Materializing, Ready, Degraded. No state is terminal. Events: BeginMigrate, BeginMaterialize, Materialized, Degrade, Reopen. Guards: none.

| Edge | From | Event | To |
|---|---|---|---|
| D01 | Opening | BeginMigrate | Migrating |
| D02 | Opening | BeginMaterialize | Materializing |
| D03 | Opening | Materialized | Ready |
| D04 | Migrating | BeginMaterialize | Materializing |
| D05 | Materializing | Materialized | Ready |
| D06 | Opening | Degrade | Degraded |
| D07 | Migrating | Degrade | Degraded |
| D08 | Materializing | Degrade | Degraded |
| D09 | Ready | Degrade | Degraded |
| D10 | Degraded | Reopen | Opening |
| D11 | Ready | Reopen | Opening |
| D12 | Degraded | Degrade | Degraded |

`Degraded` carries `DegradeReason { code, task: Option<String>, detail: String }`. Codes: `PARTIAL_MATERIALIZATION`, `NO_PROGRESS`, `MIGRATION_REFUSED`, `STATE_LOCK_BUSY`.

### RequestMachine (R004.3)

File: `crates/medulla-observe/src/machine/request.rs`.

States: Received (initial), Admitted, Authorized, Handling, WaitingLock, Responding, Done (terminal), Refused (terminal). Events: Admit, Authorize, Handle, WaitLock, Acquire, Respond, Complete, Refuse. Guards: none.

| Edge | From | Event | To |
|---|---|---|---|
| Q01 | Received | Admit | Admitted |
| Q02 | Received | Refuse | Refused |
| Q03 | Admitted | Authorize | Authorized |
| Q04 | Admitted | Refuse | Refused |
| Q05 | Authorized | Handle | Handling |
| Q06 | Authorized | Refuse | Refused |
| Q07 | Handling | WaitLock | WaitingLock |
| Q08 | WaitingLock | Acquire | Handling |
| Q09 | Handling | Respond | Responding |
| Q10 | Handling | Refuse | Refused |
| Q11 | WaitingLock | Refuse | Refused |
| Q12 | Responding | Complete | Done |
| Q13 | Responding | Refuse | Refused |

## Registry types (R003, R006)

File: `crates/medulla-observe/src/registry/`.

| Type | Field | Type | Rule |
|---|---|---|---|
| `TaskSpec` | `name` | `String` | Unique among live tasks; at most 128 bytes |
| `TaskSpec` | `owner` | `String` | The subsystem that spawned it |
| `TaskSpec` | `kind` | `TaskKind` | `Materialization`, `Index`, `Migration`, `Maintenance`, `Projection`, `Replication`, `Listener`, `External` |
| `ProgressCursor` | variant | enum | `Keyset { node, edge }`, `Counter(u64)`, `Opaque(String)` |
| `ProgressCursor::Opaque` | value | `String` | At most 256 bytes |
| `TaskRow` | `state` | `TaskState` | Stored form of `TaskMachine` |
| `TaskRow` | `last_progress_unix_ms` | `u64` | 0 before the first progress |
| `TaskRow` | `last_progress_location` | `&'static str` | `file:line` of the last `progress` call |
| `TaskRow` | `registered_location` | `&'static str` | `file:line` of `register` |
| `TaskRow` | `units_done` | `u64` | Monotonic |
| `TaskRow` | `origin_request` | `Option<u64>` | Request that spawned it |
| `RequestRow` | `request_id`, `trace_id`, `method`, `graph`, `principal_ref`, `phase`, `age_ms`, `waiting_for` | mixed | `principal_ref` is a keyed hash; no payload |
| `LockRow` | `name`, `holder`, `hold_ms`, `waiters`, `longest_wait_ms` | mixed | `holder` is a request id or a task name |
| `WorkerRow` | `index`, `busy_ms`, `busy_ratio`, `parks`, `local_queue`, `spinning` | mixed | From stable tokio metrics |

## Wire additions

| Type | Field | Type | Rule |
|---|---|---|---|
| `Request` | `trace_id` | `Option<String>` | W3C `traceparent`, at most 55 chars; outside the signed envelope; omitted when none |
| `HealthReport` | `readiness` | `Option<ReadinessReport>` | Additive; absent from older servers |
| `ReadinessReport` | `ready`, `graphs`, `blockers` | `bool`, `Vec<GraphReadiness>`, `Vec<Blocker>` | At most 100 blockers |
| `Blocker` | `graph`, `state`, `code`, `since_ms`, `task` | mixed | `task` is the blocking task name |
| `IntrospectPage` | `section`, `rows`, `next_cursor`, `seq` | mixed | Ascending by key; `seq` is the registry sequence number |
| `StartupStep` | `name`, `graph`, `started_unix_ms`, `duration_ms`, `outcome`, `detail` | mixed | `outcome` is `Ok`, `Skipped`, `Refused` or `Failed` |
| `BuildInfo` | `version`, `git_sha`, `build_id`, `profile`, `features` | strings | Read once at start |
| `Frame` | `module`, `build_id`, `offset`, `function`, `file_line` | mixed | `function` and `file_line` only when symbols exist |

## Refusal codes (new `ServerErrorCode` variants)

`PARTIAL_MATERIALIZATION`, `NO_PROGRESS`, `INTROSPECT_SECTION_INVALID`, `INTROSPECT_LIMIT_INVALID`, `INTROSPECT_CURSOR_INVALID`, `PROFILE_BUSY`, `PROFILE_DURATION_INVALID`, `PROFILE_UNAVAILABLE`, `STATE_LOCK_BUSY`.

## Scopes

`observe:read` (class admin) for `Introspect`. `observe:profile` (class admin) for `Profile`.

## Knobs (R011)

| Knob | Default | Min | Max | Unit | Class |
|---|---|---|---|---|---|
| `observe.task_stall_after_ms` | 60000 | 100 | 3600000 | ms | Tunable |
| `observe.stall_check_interval_ms` | 5000 | 50 | 60000 | ms | Tunable |
| `observe.health_deadline_ms` | 2000 | 100 | 30000 | ms | Tunable |
| `observe.event_ring_capacity` | 4096 | 256 | 65536 | events | Tunable |
| `observe.task_registry_capacity` | 4096 | 64 | 65536 | tasks | Tunable |
| `observe.progress_idle_limit` | 2 | 1 | 100 | iterations | Tunable |
| `observe.worker_spin_busy_ratio` | 0.9 | 0.5 | 1.0 | ratio | Tunable |
| `observe.lock_hold_warn_ms` | 5000 | 100 | 600000 | ms | Tunable |
| `observe.startup_reports_kept` | 8 | 1 | 64 | reports | Tunable |
| `observe.profile_hz` | 99 | 10 | 1000 | Hz | Tunable |
| `observe.profile_max_seconds` | 60 | 60 | 60 | s | Fixed |

## Durable table (R008.2)

`OBSERVE_STARTUP_REPORT`: key `boot_seq: u64`, value MessagePack `StartupReport`. Registered in the durable table registry. Pruned to `observe.startup_reports_kept` rows at each boot.
