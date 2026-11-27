# Seam contract: task registry

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-OBSERVE-R003
Consumer: medulla clustering kernel `MEDULLA-CLUSTER` training-run progress rows, bound through MEDULLA-OBSERVE-R003.1; any later long-running job binds the same way

## Provider

Repository: medulla. Rows: `MEDULLA-OBSERVE-R003`, `MEDULLA-OBSERVE-R003.1`, `MEDULLA-OBSERVE-R003.2`, `MEDULLA-OBSERVE-R004.1` for the state machine, `MEDULLA-OBSERVE-R005.2` for the stall watchdog and `MEDULLA-OBSERVE-R006.1` for the served `tasks` section.

## Consumer

A consumer owns a long-running job and wants it visible, supervised and explained. It adds a row in its own spec that names `MEDULLA-OBSERVE-R003` in `Depends-on`. It binds the job to the registry through the API below and adds no state machine of its own for the same facts.

## API

| Item | Shape | Rule |
|---|---|---|
| Register and run | `medulla_observe::spawn::spawn(TaskSpec { name, owner, kind }, future)` | Registers, runs the future inside the span current at the call, finishes the handle with the future's outcome (`IntoOutcome`, with `FailureCode` for an error), and records the spawning request as `origin_request` (`SpawnContext::capture`, `TaskHandle::set_origin_request`) |
| Register only | `TaskRegistry::register(TaskSpec) -> TaskHandle` | `TaskSpec` names the job and its `TaskKind` (`Materialization`, `Index`, `Migration`, `Maintenance`, `Projection`, `Replication`, `Listener` or `External`). For a job the consumer spawns itself; the process-wide registry is `medulla_observe::global()` |
| Report progress | `TaskHandle::progress(cursor, units_done)` | Call at least once per pass or per unit of work; records the caller location. `TaskHandle::reporter` returns a cloneable `TaskProgress` for the same call from another thread |
| Report from inside a job | `spawn::progress(cursor, units_done)` | Reaches the current job without its handle; `spawn::current_reporter()` returns a `TaskProgress` a blocking thread can use, and `spawn::current_task_name()` names the job |
| Finish | `TaskHandle::finish(Outcome)` | `Outcome` is `Completed`, `Failed(code)` or `Cancelled` |
| Cursor | `ProgressCursor::{Keyset, Counter(u64), Opaque(String)}` | `Opaque` holds at most 256 bytes and never a payload or a secret; `ProgressCursor::opaque` cuts at a character boundary |
| Read | `TaskRegistry::get`, `all`, `live`, `sequence` and `TaskHandle::row` return `TaskRow`; `file_line` renders a recorded location | A row holds name, owner, kind, state, cursor, units, last-progress time and location, and the origin request |
| Guard a loop | `ProgressGuard::new(name)` or `with_limit`, then `check(&cursor)` | After `observe.progress_idle_limit` equal cursors in a row `check` returns `NoProgress` naming the loop and its `file:line`; `set_progress_idle_limit` and `progress_idle_limit` read and set the limit |
| Capacity | `TaskRegistry::set_capacity`, `dropped_total` | A job that does not fit still runs unlisted and is counted |
| Supervise | `TaskRegistry::detect_stalls(now, stall_after_ms)` | The stall watchdog's scan; the clock is an argument |

A training run reports `Counter(step)` as the cursor and its step count as units. The task name is `train:<run id>`. The owner is `clustering`.

## Behavior the consumer receives

- State: Registered, Running, Progressing, Stalled, then Completed, Failed or Cancelled (`TaskMachine`, edges T01 to T17).
- A task with no progress for `observe.task_stall_after_ms` becomes Stalled and logs its registration and last-progress location at ERROR.
- A dropped handle records `Failed(TASK_DROPPED)`. A panic records `Failed(TASK_PANICKED)`.
- `Introspect tasks` lists the task. The `TaskDump` profile includes it.

## Wire shape

The registry has no wire shape of its own. A registered task is read through the served `Introspect` section `tasks` (`MEDULLA-OBSERVE-R006.1`). Each row holds name, owner, kind, state, cursor, units done, last-progress time and location, and the origin request.

## Refusal codes

`NO_PROGRESS` (a guarded loop of the task made no progress), `TRANSITION_REFUSED` (an event the `TaskMachine` does not accept). A failed task records its typed code in `Failed(code)`.

## Version and compatibility

The API is internal to the workspace. A change to a signature updates every consumer in the same change (hard cutover).
