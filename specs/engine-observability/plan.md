# MEDULLA-OBSERVE — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

A production incident took hours because the engine did not say what it was doing. One thread spun at 100 percent. The binary had no symbols. A typed refusal reached the client as `INTERNAL`. No call listed requests, background tasks or locks. A paging loop had no progress guard and re-found its cursor by a quadratic skip scan. Log lines dropped module targets. Readiness hung instead of explaining.

This plan adds one small crate, `medulla-observe`, that holds the task registry, the request and lock registries, three explicit state machines, the progress guard, the watchdog, the startup report and the profiler. It wires them into the existing dispatch, boot and paging paths. It adds two served methods (`Introspect`, `Profile`), extends `Health`, adds `/readyz` and a trace id, and fixes the refusal collapse. Release symbols extend MEDULLA-CONTRACT. The paging guard and seek cursor extend MEDULLA-DURABLE-KERNEL.

## Technical Context

- **Language/Version**: Rust, toolchain of `rust-toolchain.toml`; Python 3.14 for the client and docs tests.
- **Primary Dependencies**: `tokio` (stable runtime metrics), `tracing`, `tracing-subscriber`, `parking_lot`, `serde`, `redb` (startup report table), a signal-based sampler behind the `profiler` feature.
- **Storage**: one control-store table `OBSERVE_STARTUP_REPORT` (R008.2). All other state is in memory and bounded.
- **Testing**: unit (L1), integration (L2), served dispatch tests (L3), a micro benchmark for the registries.
- **Target Platform**: the medulla server on Linux x86_64 and arm64; macOS and Windows build without the sampler.
- **Project Type**: service and library crates in one workspace.
- **Performance Goals**: register or progress at most 2 microseconds p99; one `Introspect` section at most 50 ms p99; `Health` within 2 s even under a held lock.
- **Constraints**: every wait bounded, every ring bounded, no payload or secret in any row, no unstable tokio flag, no second store.
- **Scale/Scope**: 60 rows in this spec, 3 in MEDULLA-DURABLE-KERNEL, 4 in MEDULLA-CONTRACT, 1 in arbora.

## Constitution Check

- I. One owner per concept: pass. This spec owns runtime state; R873 and R060 extend their owners as child rows.
- II. Extend before you create: pass. The search is in spec.md "Why a new spec"; research.md D1 records the placement.
- III. Specs describe the current state only: pass.
- IV. Requirements are neutral and controlled: pass. No product names.
- V. Hard cutover, no legacy path: pass. The fixed `unclassified engine refusal` text and the DEBUG-only line are replaced in the same release (R001.2). The wire additions are optional fields.
- VI. Names carry no version suffix: pass.
- VII. One engine, no second database, no sync: pass. One control-store table; the rest is memory.
- VIII. Ask the graph, not a bespoke operation: pass with a stated reason. Runtime state is process state, not graph facts, so it has its own admin read (research.md D2). The machines are described in the ontology (R004.5).
- IX. Connectors depend on the SDK: not applicable.
- X. Every ontology class rolls up to the core: pass. R004.5 classes extend the core state-machine classes.
- XI. Code stays small: pass. New logic goes into a new crate; every file stays under 900 lines.
- XII. No ratchets and no suppressions: pass. `clippy.toml` disallows bare `tokio::spawn` in the listed crates; no baseline.
- XIII. Zero flakes: pass. Chaos tests use fixed knobs and bounded deadlines.
- XIV. Compile first: pass. Each row names files that compile in order.
- XV. Python 3.14 standard: pass.
- XVI. Install and deploy from releases: not applicable.
- XVII. Publish the gate's tool first: pass. The symbolize script ships before any gate uses it.
- XVIII. Landed means reachable: pass. Each rollup has a wiring row and a served proof.
- XIX. Tag push publishes: pass. R060.2 adds artifacts to the existing tag workflow.
- XX. Secrets are never traced: pass. R006.5 forbids tokens and payloads in rows and logs; principals are keyed references.
- XXI. Public files stay portable: pass. No host, private path or tool name.
- XXII. Names and placement: pass.
- XXIII. Versioning and release: pass. Wire changes ship with the Python client update in the same release.
- XXIV. Complete specifications: pass. Every row names caller chain, files, types, codes, knobs, budgets and test.
- XXV. Small, modular units: pass. `medulla-observe` sits above `medulla-types` and below `medulla-kernel`, `medulla-persistence` and the server crates.
- XXVI. Defects proved over the real protocol: pass. Every rollup has a served proof.
- XXXI. Explicit state machines: pass. Three machines with closed state sets, one transition function each and `TRANSITION_REFUSED`.
- R-I. Native Rust, only fact store: pass.
- R-II. Every wait has a deadline: pass. `Health` uses a bounded read (R007.3); the lock wrappers record waits and time out as the callers already do.
- R-III. Budgets and a benchmark per row: pass. Each row states budgets; registries use `benches/registry.rs`.
- R-IV. Crates are small, layered: pass. The crate is new and below the size limit.

## Project Structure

```text
crates/medulla-observe/Cargo.toml                           new (R003.1)
crates/medulla-observe/src/lib.rs                           new (R003.1)
crates/medulla-observe/src/machine/task.rs                  new (R004.1)
crates/medulla-observe/src/machine/readiness.rs             new (R004.2)
crates/medulla-observe/src/machine/request.rs               new (R004.3)
crates/medulla-observe/src/events.rs                        new (R004.4)
crates/medulla-observe/src/registry/tasks.rs                new (R003.1)
crates/medulla-observe/src/registry/requests.rs             new (R006.2)
crates/medulla-observe/src/registry/locks.rs                new (R006.4)
crates/medulla-observe/src/registry/readiness.rs            new (R007.1)
crates/medulla-observe/src/spawn.rs                         new (R003.2, R002.4)
crates/medulla-observe/src/trace.rs                         new (R002.2, R002.3)
crates/medulla-observe/src/progress.rs                      new (R005.1)
crates/medulla-observe/src/watchdog.rs                      new (R005.2, R005.3)
crates/medulla-observe/src/runtime_stats.rs                 new (R006.3)
crates/medulla-observe/src/startup.rs                       new (R008.1, R008.2)
crates/medulla-observe/src/profile.rs                       new (R010.1, R010.2)
crates/medulla-observe/src/chaos.rs                         new (R012)
crates/medulla-observe/tests/*.rs                           new (unit and integration proofs)
crates/medulla-observe/benches/registry.rs                  new (budgets)
crates/medulla-types/src/introspect.rs                      new (R006.1)
crates/medulla-types/src/contract/server_error_code.rs      existing, new codes
crates/medulla-protocol-types/src/protocol.rs               existing (R002.2)
crates/medulla-protocol-types/src/protocol/support_04.rs    existing (R001.2)
crates/medulla-protocol-types/src/protocol/method/method_11.rs existing (R006.1, R010.1)
crates/medulla-protocol-types/src/result_contract/cluster/lifecycle.rs existing (R007.2)
crates/medulla-capabilities/src/domains/security.rs         existing (R006.1)
crates/medulla-capabilities/src/scopes.rs                   existing (R006.1, R010.1)
crates/medulla-server-graph/src/dispatch/graph_pipeline/graph_access.rs existing (R001.1)
crates/medulla-server-router/src/dispatch/request_boundary.rs existing (R002.3, R006.2)
crates/medulla-server-router/src/dispatch/router/service_control.rs existing (R007.2, R007.3)
crates/medulla-h-admin/src/admin/introspect.rs              new (R006.1, R006.5, R008.4)
crates/medulla-h-admin/src/admin/profile.rs                 new (R010.1, R010.3)
crates/medulla-h-admin/src/namespace_migration.rs           existing (R003.5, R008.3)
crates/medulla-h-admin/src/boot_maintenance.rs              existing (R003.6)
crates/medulla-persistence/src/cold_offload.rs              existing (R003.3, MEDULLA-DURABLE-KERNEL-R873.1)
crates/medulla-store-apply/src/redb_store/dump.rs           existing (MEDULLA-DURABLE-KERNEL-R873.2)
crates/medulla-kernel/src/otel.rs                           existing (R002.1)
crates/medulla-server-obs/src/obs/http/mod.rs               existing (R007.4)
crates/medulla-server-api/src/knob_config.rs                existing (R011)
crates/medulla-version/src/build_info.rs                    new (R009.1)
crates/medulla-pyengine/src/errors.rs                       existing (R001.4)
crates/medulla-core/ontology/state-machines-v1.ttl          existing (R004.5)
crates/medulla-server-served-tests/src/split_tests/observability.rs new (served proofs)
scripts/split_release_symbols.sh                            new (MEDULLA-CONTRACT-R060.1)
scripts/symbolize.sh                                        new (MEDULLA-CONTRACT-R060.3)
.github/workflows/release.yml                               existing (MEDULLA-CONTRACT-R060.2)
medulla/client.py                                           existing (R001.4, R006.6, R007.2)
docs/interfaces/engine-observability.md                     new (R013)
tests/docs/test_engine_observability_doc.py                 new (R013)
```

## Phase 0: Research

Six unknowns were resolved in [research.md](research.md): where the registry lives, why runtime state is not a graph read, how a global handle reaches deep crates, how the sampler symbolizes a stripped binary, how a seek replaces the skip scan, and how the trace id rides the signed envelope.

## Phase 1: Design

### Refusals

`Response::err` accepts only `CODE: detail` text with a declared code. `check_materialization_valid` returned a bare JSON object whose `code` field the parser never read, so the wire showed `INTERNAL` (R001.1). R001.2 keeps the rule that uncoded text never reaches the wire, and adds a cause reference that ties the wire `INTERNAL` to one WARN line. R001.3 counts every uncoded text and fails a served test that produced one. R001.4 carries the reference to Python callers.

### Logs and trace

`init_fmt_only` and `init_with_otel` print the module target (R002.1). `Request.trace_id` carries a W3C `traceparent` (R002.2). `dispatch_with_context` opens one `request` span with `request_id`, `trace_id`, `op` and `graph` (R002.3). `observe::spawn` carries the span into each task (R002.4). The span is the single place that sets these fields.

### State machines

Three machines (R004) use `medulla_types::transition_refused::transition`. Edge tables are in [data-model.md](data-model.md). Each transition becomes a `TransitionEvent` in a bounded ring and a log line (R004.4). The machines are described in `state-machines-v1.ttl` (R004.5). A source scan proves no writer sets state outside `apply` (R004.6).

### Task registry

`TaskRegistry` is a process-wide handle reached through `medulla_observe::global()`. Tests build isolated instances. A job registers through `observe::spawn`. A job reports `progress(cursor, units)`. The handle drives `TaskMachine`. The inventory of registered jobs is closed: lazy paging, index builds, store and namespace migration, matview reload, time-series reconcile, pack projection drain, reasoning projection, replica pull, listener accept loops and the watchdog (R003.3 to R003.6). A consumer outside the inventory binds through [contracts/task-registry.md](contracts/task-registry.md).

### Introspection

`Method::Introspect` reads eight sections (R006.1). `requests` reads `RequestRegistry`, filled at the dispatch chokepoint (R006.2). `runtime` samples stable tokio metrics and flags a worker busy for two windows (R006.3). `locks` reads the lock sites (R006.4). Rows carry no payload and no token (R006.5).

### Readiness

`ReadinessRegistry` holds one machine per graph (R007.1). `Health` returns `ReadinessReport` and blockers (R007.2) under a bounded read (R007.3). `/readyz` serves the same report to probes (R007.4). The watchdog degrades a graph whose materialization task stalls (R005.3).

### Startup report

`StartupReport::step` records duration and outcome per step (R008.1). The last reports persist across restarts (R008.2). The namespace migration records each graph and its refusal code (R008.3).

### Progress proofs

`ProgressGuard` fails a loop whose cursor repeats (R005.1). The watchdog marks a silent task Stalled (R005.2). MEDULLA-DURABLE-KERNEL-R873.1 puts the guard in `page_in_remaining`. R873.2 replaces the skip scan in `read_graph_dump_page_nodes` and `read_graph_dump_page_edges` with a bounded range seek from the exclusive cursor key to the exclusive successor of the graph scope.

### Build identity

`BUILD_INFO` holds version, git SHA, features and profile, embedded by `build.rs` without breaking reproducibility (R009.1). The build-id is read from the running executable. The startup line and `Introspect build` report both (R009.2).

### Profiling

`Method::Profile` samples threads for N seconds and returns folded stacks with module build-id and offset per frame (R010). `TaskDump` returns one consistent snapshot of the registries (R010.3).

### Release symbols

MEDULLA-CONTRACT-R060 splits debug info from each release-server binary, keeps the GNU build-id in both files, publishes `symbols-<platform>.tar.zst` with each `v*` release, and ships `scripts/symbolize.sh`.

### Chaos proof

Fault injection behind feature `chaos` stalls a task, spins a loop and freezes a paging cursor. One served test asserts that `Introspect`, the watchdog log and `Health` name the culprit within 10 s (R012).

### Untangled design

One line per answer.

- Owner: Medulla. The new crate `medulla-observe` owns the registries and machines; handler crates only call it.
- Cohesion: All runtime-state types sit in one crate, so a consumer names one dependency.
- Dependency direction: `medulla-observe` depends on `medulla-types` only. The kernel, persistence and server crates depend on it. It imports no handler.
- Interface depth: Three calls cover a job: `spawn`, `progress` and `finish`. The introspection is one method with a closed section enum.
- Special cases: None. Every background job takes the same path; the closed inventory lists them.
- State: Each machine stores one state field and changes it only through `apply`.
- Environment: The sampler needs a Unix signal; other platforms refuse `PROFILE_UNAVAILABLE`. Stable tokio metrics only.

### Failure modes

- Registry full: the job runs, the drop counts, the job is not listed.
- Handle dropped: `Failed(TASK_DROPPED)`.
- Panic in a job: `Failed(TASK_PANICKED)` with location; the process continues (`panic = "unwind"`).
- State lock held: `Health` answers `degraded` with `STATE_LOCK_BUSY`.
- Sampler unavailable: `PROFILE_UNAVAILABLE`; `TaskDump` still serves.
- Startup report write fails: the report stays in memory and logs at WARN; boot continues.

The served behavior and its commands are in [quickstart.md](quickstart.md). The seams are in [contracts/task-registry.md](contracts/task-registry.md) and [contracts/trace-correlation.md](contracts/trace-correlation.md).

## Phase 2: Task approach

`tasks.md` lists one task per row in dependency order and in six slices. Slice A fixes refusals and logs. Slice B builds the registry, machines, introspection, readiness and report. Slice C adds the paging guard and seek cursor in MEDULLA-DURABLE-KERNEL. Slice D adds build identity and release symbols. Slice E adds the watchdog and the profiler. Slice F adds the chaos proof and the guide.

## Complexity Tracking

none

## Build contract

Every change passes the hooks of `.config/pre-commit.yaml` at the pre-commit and pre-push stages, and the manual stage for the scanners below.

| Gate | Threshold | Command |
|---|---|---|
| Format | rustfmt clean, edition 2021 | `cargo fmt --all -- --check` |
| Lint | zero warnings, default features for touched crates and direct dependents | `cargo clippy -p <crate> --all-targets -- -D warnings` |
| Complexity | no new or worsened function over cyclomatic 10 or cognitive 15 | `pre-commit run complexity-staged --hook-stage manual --config .config/pre-commit.yaml` |
| Clones | dupehound similarity 0.85 at 40 tokens; jscpd 50 tokens and 5 lines, zero new pairs | `pre-commit run dupehound-changed-functions jscpd-differential --hook-stage manual --config .config/pre-commit.yaml` |
| KISS (Rust) | statements per function 35; arguments 8; returns 8; indentation 5; locals 20; calls 45; methods per type 13; lines per file 900; functions per file 40 | `pre-commit run kiss-changed-rust --hook-stage manual --config .config/pre-commit.yaml` |
| Import cycles | 0; dependency depth 4 | `pre-commit run rust-arch-lint --hook-stage manual --config .config/pre-commit.yaml` |
| Crate graph | `medulla-observe` below the kernel and persistence layers | `python3 scripts/check_crate_graph.py` |
| Durable tables | no new table outside the registries | `pre-commit run durable-table-registration --config .config/pre-commit.yaml` |
| Affected tests | nextest on each changed crate and its direct dependents | `cargo nextest run --locked -p <each changed crate>` |
| Spec | clean | `pipelines-hook spec-standard`, `pipelines-hook plan-conformance` |
| Wording | 20 words per sentence | `pipelines-hook ste-staged` |

Code shapes: new logic goes into the new files listed above. No function takes more than 8 arguments. No new `#[allow]`. Every new code is a `ServerErrorCode` variant. Every knob is a row of `KNOB_TABLE`.

## Production caller chain

- Request: transport frame reader (`crates/medulla-server/src/transport.rs:548`) → `dispatch_reserved_request` (`transport.rs:570`) → `dispatch_verified_request` → `dispatch_with_context` (`crates/medulla-server-router/src/dispatch/request_boundary.rs:43`) → request span and `RequestRegistry::enter` (R002.3, R006.2).
- Refusal: any handler → `Response::err` (`crates/medulla-protocol-types/src/protocol/support_04.rs:150`) → wire; `check_materialization_valid` (`graph_access.rs:320`) (R001).
- Lazy paging: `lazy_open` (`crates/medulla-persistence/src/cold_offload.rs:476`) → `observe::spawn` (`cold_offload.rs:526`) → `page_in_remaining` (`cold_offload.rs:541`) → `materialize_page` (`crates/medulla-core/src/registry.rs:401`) → `read_graph_dump_page_blocking` (`crates/medulla-persistence/src/redb_backend/backend_audit.rs:174`) → `read_graph_dump_page_nodes` (`crates/medulla-store-apply/src/redb_store/dump.rs:454`).
- Boot: `serve_transports` (`crates/medulla-server/src/server_startup.rs:850`) → `spawn_post_bind_maintenance` (`crates/medulla-h-admin/src/boot_maintenance.rs:35`) → `namespace_migration::spawn` (`namespace_migration.rs:259`).
- Health: `Method::Health` → `health_response` (`crates/medulla-server-router/src/dispatch/router/service_control.rs:62`) → `ReadinessRegistry::report`.
- Probe: `GET /readyz` → `control_response` (`crates/medulla-server-obs/src/obs/http/mod.rs:356`).
- Introspect: transport → `dispatch` → admin arm (`crates/medulla-h-admin/src/admin.rs:67`) → `introspect::handle`.
- Locks: `lock_graph` (`crates/medulla-server-api/src/routed_write_coalescer.rs:352`) and `timed_read` (`crates/medulla-server-graph/src/dispatch/shared.rs:223`) → `LockSite::acquire`.
- Logs: `medulla_kernel::otel::init` (`crates/medulla-kernel/src/otel.rs:52`, `:107`).
- Release: `.github/workflows/release.yml` job `server-binary` (line 149) → `scripts/split_release_symbols.sh` → release assets.

Obsolete paths to delete: the fixed `unclassified engine refusal` detail at `support_04.rs:161` and `crates/medulla-pyengine/src/errors.rs:172`; the skip-scan comment and `BeforeCursor` steps at `dump.rs:445`, `:494` and `:540` once R873.2 lands; the unbounded `timed_read` in `health_response`.

## Explicit state machines

Organization constitution Article XXXI requires a closed set of states, one transition function and a typed refusal for each machine. Rows `MEDULLA-OBSERVE-R004` apply it here. Each machine returns `TransitionRefused` (MEDULLA-ORCH-VOCAB-R811.4), stores its state in one field and is described in `crates/medulla-core/ontology/state-machines-v1.ttl`.

### Machine wiring

- `TaskMachine`: `TaskHandle` methods and the watchdog → `TaskState::apply` in `crates/medulla-observe/src/machine/task.rs`. Wiring row: R004.1. Hot-path proof row: `no_writer_sets_state_outside_apply` of R004.6.
- `ReadinessMachine`: graph open, migration, paging → `ReadinessState::apply` in `machine/readiness.rs`. Wiring row: R007.1. Hot-path proof row: `health_names_the_blocking_graph_and_reason` of R007.2.
- `RequestMachine`: `dispatch_with_context` and the lock wrappers → `RequestState::apply` in `machine/request.rs`. Wiring row: R006.2. Hot-path proof row: `in_flight_request_is_listed_with_phase_and_age` of R006.2.

## Cross-repo seams

- `ARBORA-HOST-R008.1007.1` consumes `MEDULLA-OBSERVE-R002.2`: arbora sends its correlation id as `Request.trace_id`. Contract: [contracts/trace-correlation.md](contracts/trace-correlation.md).
- The clustering kernel (`MEDULLA-CLUSTER`) consumes `MEDULLA-OBSERVE-R003` for training-run progress. Contract: [contracts/task-registry.md](contracts/task-registry.md).

`ARBORA-OPS-R018` serves `Introspect` and `Profile` as generated operations `eg.<service>.<Method>`. Arbora adds no handler for them.

## Amendments

none
