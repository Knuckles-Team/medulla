# Engine observability

The engine reports what it is doing and why it is not ready. This page lists the served call, its
sections, the log targets, the refusal codes, the knobs and the command that symbolizes a stack.

## Introspect

`Introspect` reads one page of one section of the runtime registries. It needs the `observe:read`
scope. A page holds rows ascending by the section's key, a `next_cursor` for the next page and the
registry sequence number the page was read at. A row carries no payload, no token and no property
value; a principal appears only as an opaque keyed reference. The call writes nothing.

`limit` is 1 to 500 (default 100). `cursor` is the `next_cursor` of the previous page.

| Section | Key | What a row says |
|---|---|---|
| `requests` | registry key | One in-flight request: id, trace id, method, graph, phase, age, the lock it waits for |
| `tasks` | task id | One background task: name, owner, kind, state, cursor, units, last progress, origin request |
| `runtime` | `global`, then `worker:<index>` | The tokio runtime: queue depth, and per worker busy time, busy ratio, parks and the `spinning` flag |
| `locks` | lock name | One named lock site: holder, hold time, waiters, longest wait |
| `readiness` | graph name | One graph's readiness state, reason code and the task that blocks it |
| `startup` | start time, step, graph | One boot step with its duration and outcome |
| `build` | `build` | The running binary: version, git SHA, build id, profile, features |
| `events` | sequence number | One accepted state-machine transition |
| `indexes` | index name | One semantic index the engine maintains and whether it is bound |

## Profile

`Profile` needs the `observe:profile` scope and is audited. `kind` is `stacks` or `task_dump`.
`stacks` samples the stack of every thread of the server for `seconds` (1 to 60) at
`observe.profile_hz` and answers folded stacks: for each thread name, each distinct stack with
its sample count. Every frame holds its module path, the module's build id, the offset inside the
module and, when the module has a dynamic symbol for it, the function. Resolve the offsets with
`scripts/symbolize.sh`, below. One profile runs at a time. The sampler is signal based and needs a
Linux build with glibc; elsewhere the call refuses `PROFILE_UNAVAILABLE`. `task_dump` answers the
tasks, the in-flight requests, the lock sites and the last 256 events, each registry read once
under one sequence number, and needs no sampler.

## Refusal codes

| Code | Meaning |
|---|---|
| `INTROSPECT_LIMIT_INVALID` | `limit` is 0 or over 500 |
| `INTROSPECT_CURSOR_INVALID` | `cursor` is not a key of the section |
| `PROFILE_BUSY` | A stack profile is already running |
| `PROFILE_DURATION_INVALID` | `seconds` is 0 or over 60 |
| `PROFILE_UNAVAILABLE` | This build or platform has no stack sampler |
| `PARTIAL_MATERIALIZATION` | A read met a graph that is only partly paged into memory; the detail carries the phase and the cursor |
| `NO_PROGRESS` | A guarded loop made no progress; the message names the loop and its `file:line` |
| `TRANSITION_REFUSED` | A state machine refused an event; the detail names the machine, the state, the event and the reason |

An engine refusal that names no declared code answers `INTERNAL` with a detail ending in a cause
reference, `cause-<8 hex>`. The same reference is on the one WARN log line that holds the full
text. The Python exception carries it as `cause_ref`.

## Readiness

`Health` carries a `readiness` report: whether the engine is ready, every graph's state and at
most 100 blockers, each with the graph, its state, a reason code and the task that blocks it. The
report is built without the server state lock, so `Health` answers within
`observe.health_deadline_ms`; when the lock does not answer it names the holder under the blocker
`STATE_LOCK_BUSY`. The observability listener serves the same report on `GET /readyz`: 200 when
ready, 503 with the blockers otherwise. `GET /healthz` stays liveness.

## Log targets

| Target | What it logs |
|---|---|
| `medulla::request` | The request span: every line of a request shows `request_id`, `trace_id`, `op` and `graph` |
| `medulla::transition` | Each state-machine transition at DEBUG; a transition into a failed, stalled or degraded state at ERROR or WARN; each refused transition at WARN |
| `medulla::refusal` | The full text of an uncoded refusal, with its cause reference |
| `medulla::startup` | One line per boot step with its outcome |
| `medulla::build` | One line at start with the version, git SHA, build id, profile and features |

A caller's W3C `traceparent` goes in the request's `trace_id` field; a malformed one is dropped and
the request runs. A task a request spawns shows the same ids and records the request as its origin.

## Knobs

| Knob | Default | Unit | What it bounds |
|---|---|---|---|
| `observe.task_stall_after_ms` | 60000 | ms | Idle time after which the watchdog marks a task stalled |
| `observe.stall_check_interval_ms` | 5000 | ms | Time between two watchdog scans |
| `observe.health_deadline_ms` | 2000 | ms | Longest `Health` and `/readyz` wait for the state |
| `observe.event_ring_capacity` | 4096 | events | Transition events the ring keeps |
| `observe.task_registry_capacity` | 4096 | tasks | Live tasks the registry lists |
| `observe.progress_idle_limit` | 2 | iterations | Equal cursors in a row before a guarded loop fails |
| `observe.worker_spin_busy_ratio` | 0.9 | ratio | Busy ratio at which a worker is flagged spinning |
| `observe.lock_hold_warn_ms` | 5000 | ms | A lock held this long logs at WARN with its holder |
| `observe.startup_reports_kept` | 8 | reports | Boot reports kept in the control store |
| `observe.profile_hz` | 99 | Hz | Stack sampling rate |
| `observe.profile_max_seconds` | 60 | s | Longest profile call; fixed |


## Symbolize a stack


Start from the offsets of the stack and the build-id the running server prints with
`--build-id`:

```sh
scripts/symbolize.sh --build-id <build-id> <tag> <platform> <offset>...
```

The command downloads the symbol artifact of the tag with `gh release download`, verifies its
sha256, checks that the symbol file inside carries the build-id of the running server, and
resolves each offset with `addr2line`. It prints one line per offset:
`<offset> <function> <file:line>`.

`--artifact-dir <dir>` uses a symbol artifact and its `.sha256` file that are already in `<dir>`
instead of downloading them.

### Worked example

```sh
$ medulla-server --build-id
3f1c9a0e5b7d2c48a6e1f0b9d4c7a2e85b6d1f03
$ scripts/symbolize.sh --build-id 3f1c9a0e5b7d2c48a6e1f0b9d4c7a2e85b6d1f03 \
      v3.3.0 linux-aarch64 0x2a4f10 0x2a5b8c
0x2a4f10 medulla_storage::kernel::StorageKernel::commit crates/medulla-storage/src/kernel.rs:812
0x2a5b8c medulla_transaction::kernel::MutationKernel::admit crates/medulla-transaction/src/kernel.rs:204
```

### Refusals

| Code | Meaning |
|---|---|
| `SYMBOL_ARTIFACT_MISSING` | The tag has no symbol artifact for this platform and build-id, or its `.sha256` file is absent. |
| `SYMBOL_ARTIFACT_DIGEST_MISMATCH` | The archive does not match its `.sha256` file. |
| `SYMBOL_FILE_MISSING` | The archive holds no `.debug` symbol file; only the Linux symbol file is read by `addr2line`. |
| `BUILD_ID_MISMATCH` | The symbol file carries a different build-id than the server printed. |

The command writes only a temporary directory, removed on exit. It resolves a stack in well
under 30 seconds on a 100 Mbit link.
