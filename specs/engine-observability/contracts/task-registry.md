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
| Register and run | `medulla_observe::spawn(TaskSpec { name, owner, kind }, future)` | Registers, enters the trace span, runs, finishes the handle with the future's outcome |
| Register only | `TaskRegistry::register(TaskSpec) -> TaskHandle` | For a job the consumer spawns itself |
| Report progress | `TaskHandle::progress(cursor, units_done)` | Call at least once per pass or per unit of work; records the caller location |
| Finish | `TaskHandle::finish(Outcome)` | `Outcome` is `Completed`, `Failed(code)` or `Cancelled` |
| Cursor | `ProgressCursor::{Keyset, Counter(u64), Opaque(String)}` | `Opaque` holds at most 256 bytes and never a payload or a secret |

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
