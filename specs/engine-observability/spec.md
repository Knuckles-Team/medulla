# MEDULLA-OBSERVE — Engine observability and explicit runtime state

Owner: medulla. Sole owner of runtime introspection, the task registry, runtime state machines, progress proofs, readiness reporting, the startup report, trace correlation and on-demand profiling.
Cross-repo IDs: `ARBORA-HOST-R008.1007.1` sends the caller's trace id (consumer of `MEDULLA-OBSERVE-R002.2`). The clustering kernel binds its training-run progress to `MEDULLA-OBSERVE-R003` (consumer). See [contracts/task-registry.md](contracts/task-registry.md) and [contracts/trace-correlation.md](contracts/trace-correlation.md).

## Delivery mapping

Component: Observability
Depends-on: MEDULLA-ORCH-VOCAB-R811.1, MEDULLA-ORCH-VOCAB-R811.4
References: [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/spec.md), [MEDULLA-CONTRACT](../public-engine-contract-and-release/spec.md), [MEDULLA-DECISION-ENGINE](../decision-engine/spec.md), [MEDULLA-IDENTITY](../engine-identity-and-scope/spec.md)
Code: ../../crates/medulla-protocol-types/src/protocol/support_04.rs ../../crates/medulla-persistence/src/cold_offload.rs ../../crates/medulla-server-router/src/dispatch/router/service_control.rs ../../crates/medulla-kernel/src/otel.rs ../../crates/medulla-h-admin/src/boot_maintenance.rs

## User Scenarios & Testing

### User Story 1 - Find why the pod is not ready in seconds (Priority: P1)

An operator sees a server pod that never becomes ready. One `Health` call or one `GET /readyz` names each graph that blocks, its state, the reason and the task behind it. The startup report lists every post-upgrade step with its duration and outcome, also from the previous boot.

**Acceptance Scenarios**:

1. **Given** a graph stuck in Materializing, **When** the operator calls `Health`, **Then** the answer names the graph, the state `Materializing`, the reason and the blocking task, within the deadline.
2. **Given** the state lock is held, **When** the operator calls `Health`, **Then** the answer is `degraded` with `STATE_LOCK_BUSY` and the holder, not a hang.
3. **Given** the namespace migration refused the commons graph on the last boot, **When** the operator reads `Introspect startup`, **Then** the step shows `Refused` with its code and the graph.

### User Story 2 - See what the engine is doing now (Priority: P1)

An operator with the observe scope lists in-flight requests, background tasks with their progress cursors, runtime workers and lock holders. The answer needs no node access and no debugger.

**Acceptance Scenarios**:

1. **Given** a request that waits for a graph lock, **When** the operator reads `requests` and `locks`, **Then** the request shows the phase `WaitingLock` and its age, and the lock shows its holder and waiter.
2. **Given** a lazy paging task, **When** the operator reads `tasks`, **Then** the task shows its name, owner, state, cursor and last-progress time.

### User Story 3 - A stuck loop names itself (Priority: P1)

A loop that makes no progress fails with a typed error that names the loop and its source line. A task that stops moving goes Stalled and logs its location. A seek cursor replaces the quadratic skip scan.

**Acceptance Scenarios**:

1. **Given** a paging cursor that never advances, **When** the loop runs, **Then** it ends with `NO_PROGRESS` naming the loop and `file:line`, and the graph is Degraded.
2. **Given** a task idle past the stall interval, **When** the watchdog scans, **Then** the task is Stalled and one ERROR line holds its location and age.

### User Story 4 - Follow one request end to end (Priority: P1)

A caller sends a trace id. Every engine log line for that request, and for each task the request spawned, shows the module target, the request id and the trace id.

**Acceptance Scenarios**:

1. **Given** `RUST_LOG=medulla::request=debug`, **When** a request runs, **Then** its lines match the filter and show both ids.
2. **Given** a request that spawns a task, **When** the task logs, **Then** the line shows the same trace id.

### User Story 5 - Symbolize a production stack (Priority: P2)

An operator takes a stack from a stripped release. The build identity gives the build-id. One command fetches the symbol artifact of that build and resolves the stack. A profile call returns stacks that carry the build-id and offsets.

**Acceptance Scenarios**:

1. **Given** a release tag, **When** the operator runs `scripts/symbolize.sh <tag> <platform> <offset>`, **Then** the output names the function and `file:line`.
2. **Given** a pinned worker, **When** the operator calls `Profile Stacks`, **Then** the folded stacks show the spinning function.

### User Story 6 - Typed refusals reach the client (Priority: P1)

A typed refusal such as `PARTIAL_MATERIALIZATION` reaches the client with its code and detail. An uncoded refusal becomes `INTERNAL` with a cause reference that matches one WARN line.

**Acceptance Scenarios**:

1. **Given** a partly materialized graph, **When** a client reads it, **Then** it receives `PARTIAL_MATERIALIZATION` with phase and cursor.
2. **Given** an uncoded handler error, **When** the client receives `INTERNAL`, **Then** the cause reference finds the full text in the log.

### Edge Cases

- The task registry is full: a new job still runs, the drop counts in `medulla_observe_dropped_total`, and the job is absent from `tasks`.
- A task handle drops without `finish`: the task records `Failed(TASK_DROPPED)`.
- A task panics: the task records `Failed(TASK_PANICKED)` with the panic location.
- A malformed trace id is dropped with a DEBUG line; the request still runs.
- Two profiles at once: the second refuses `PROFILE_BUSY`.
- A platform without a sampler refuses `PROFILE_UNAVAILABLE`; `TaskDump` still works.
- An introspection cursor from another section refuses `INTROSPECT_CURSOR_INVALID`.
- A transition in a terminal state refuses `TRANSITION_REFUSED` with the reason `TerminalState`.
- Introspection of a graph the caller cannot see shows no graph name.

## Requirements

- **FR-001**: Typed refusals keep code and detail to the client. Rows: `MEDULLA-OBSERVE-R001`, `MEDULLA-OBSERVE-R001.1`, `MEDULLA-OBSERVE-R001.3`, `MEDULLA-OBSERVE-R001.4`.
- **FR-002**: An uncoded refusal becomes `INTERNAL` with a cause reference. Rows: `MEDULLA-OBSERVE-R001.2`.
- **FR-003**: Logs carry module targets and the caller's trace id. Rows: `MEDULLA-OBSERVE-R002`.
- **FR-004**: Tasks, graph readiness and requests are explicit state machines. Rows: `MEDULLA-OBSERVE-R004`.
- **FR-005**: Every background job registers in one registry. Rows: `MEDULLA-OBSERVE-R003`.
- **FR-006**: One admin operation lists requests, tasks, workers and locks. Rows: `MEDULLA-OBSERVE-R006`.
- **FR-007**: Readiness names each blocker within a deadline. Rows: `MEDULLA-OBSERVE-R007`.
- **FR-008**: Each startup and upgrade step leaves a record. Rows: `MEDULLA-OBSERVE-R008`.
- **FR-009**: Loops prove progress or fail naming themselves. Rows: `MEDULLA-OBSERVE-R005`. The paging rows R873 of the durable kernel spec consume them.
- **FR-010**: The server reports its build identity. Rows: `MEDULLA-OBSERVE-R009`.
- **FR-011**: Profiling runs on demand behind admin auth. Rows: `MEDULLA-OBSERVE-R010`.
- **FR-012**: A chaos test names the culprit in bounded time. Rows: `MEDULLA-OBSERVE-R012`.
- **FR-013**: Release symbols are published and symbolization is one command. Rows: `MEDULLA-OBSERVE-R009.1`. Release symbol artifacts are rows R060 of the contract spec.
- **FR-014**: A guide and a drift test cover the feature; every knob is registered. Rows: `MEDULLA-OBSERVE-R011`, `MEDULLA-OBSERVE-R013`.

## Success Criteria

- **SC-001**: `Health` answers within `observe.health_deadline_ms` (default 2 s) with the state lock held. Rows: `MEDULLA-OBSERVE-R007.3`.
- **SC-002**: A spinning loop or stalled task is named within 10 s of the injected fault. Rows: `MEDULLA-OBSERVE-R012.1`, `MEDULLA-OBSERVE-R012.2`.
- **SC-003**: One `Introspect` section answers in 50 ms p99 with 4096 tasks and 1024 requests. Rows: `MEDULLA-OBSERVE-R006`.
- **SC-004**: Registering a request or a task adds at most 2 microseconds p99. Rows: `MEDULLA-OBSERVE-R003.1`, `MEDULLA-OBSERVE-R006.2`.
- **SC-005**: No typed refusal reaches a client as `INTERNAL`. Rows: `MEDULLA-OBSERVE-R001.3`.
- **SC-006**: Paging a graph of N rows costs O(N), not O(N squared). Rows: `MEDULLA-OBSERVE-R012.3`. The seek cursor is row R873.2 of the durable kernel spec.

## Assumptions

- `MEDULLA-ORCH-VOCAB-R811.1` and `R811.4` deliver the state-machine vocabulary and `TransitionRefused` that the three machines use.
- The wire addition is one optional `Request.trace_id` field and additive `Health` fields. Old clients keep working; stored data does not change.
- Only stable tokio runtime metrics are used. No unstable tokio cfg flag is set.
- The release server keeps `panic = "unwind"` and a stripped shipped binary; symbols ship as separate artifacts.
- `observe:read` and `observe:profile` are admin-class scopes that arbora grants through its normal scope mapping.

## Why a new spec

The search ran `git grep -n -iE "introspect|in-flight|inflight|task registry|watchdog|stall|readiness|ready\b|startup report|traceparent|trace_id|symbol|build-id|profil" -- specs`.
It read MEDULLA-DURABLE-KERNEL, MEDULLA-ENGINE-PARITY, MEDULLA-CONTRACT, MEDULLA-DECISION-ENGINE and MEDULLA-IDENTITY.

- MEDULLA-ENGINE-PARITY-R008 owns `pg_catalog` introspection for SQL tools. It states no engine runtime state.
- MEDULLA-DURABLE-KERNEL owns lazy paging and the semantic index upgrade machine. It states no registry, no progress proof and no report.
- MEDULLA-CONTRACT owns the release workflow and reproducibility. It states no symbol artifact.
- MEDULLA-DECISION-ENGINE-R179 owns the knob registry. This spec adds knobs and cites that row.
- MEDULLA-IDENTITY owns scopes. This spec adds two scopes and cites that spec.

No row owns runtime introspection, the registry or the readiness explanation. The release-symbol rows and the lazy-paging rows have owners, so they extend MEDULLA-CONTRACT (`R060`) and MEDULLA-DURABLE-KERNEL (`R873`) as child rows. This spec owns the rest.

## Scope and interfaces

In scope: the registry, the three machines, the introspection and profile methods, readiness, the startup report, trace correlation, build identity, the refusal cause reference and the chaos proof.

Interfaces: `Method::Introspect`, `Method::Profile`, `Method::Health` (extended), `GET /readyz`, `Request.trace_id`, the `traceparent` header, the log targets `medulla::request`, `medulla::transition`, `medulla::startup`, `medulla::build` and `medulla::refusal`, the metrics `medulla_refusal_unclassified_total` and `medulla_observe_dropped_total`.

Non-goals: the metrics and trace backends (the observability stores), served operations and scope grants (arbora), the caller's correlation context (arbora), the clustering kernel's own state (its spec).

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-OBSERVE-R001` | plan.md "Refusals" | T-001 |  |
| FR-002 | `MEDULLA-OBSERVE-R001.2` | plan.md "Refusals" | T-002 |  |
| FR-003 | `MEDULLA-OBSERVE-R002` | plan.md "Logs and trace" | T-003 |  |
| FR-004 | `MEDULLA-OBSERVE-R004` | plan.md "State machines" | T-004 |  |
| FR-005 | `MEDULLA-OBSERVE-R003` | plan.md "Task registry" | T-005 |  |
| FR-006 | `MEDULLA-OBSERVE-R006` | plan.md "Introspection" | T-006 |  |
| FR-007 | `MEDULLA-OBSERVE-R007` | plan.md "Readiness" | T-007 |  |
| FR-008 | `MEDULLA-OBSERVE-R008` | plan.md "Startup report" | T-008 |  |
| FR-009 | `MEDULLA-OBSERVE-R005` | plan.md "Progress proofs" | T-009 |  |
| FR-010 | `MEDULLA-OBSERVE-R009` | plan.md "Build identity" | T-010 |  |
| FR-011 | `MEDULLA-OBSERVE-R010` | plan.md "Profiling" | T-011 |  |
| FR-012 | `MEDULLA-OBSERVE-R012` | plan.md "Chaos proof" | T-012 |  |
| FR-013 | `MEDULLA-OBSERVE-R009.1` | plan.md "Release symbols" | T-013 |  |
| FR-014 | `MEDULLA-OBSERVE-R013` | plan.md "Build contract" | T-014 |  |
