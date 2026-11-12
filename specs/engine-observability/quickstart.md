# MEDULLA-OBSERVE — Quickstart

Governing spec: [spec.md](spec.md).

## Why is the server not ready?

1. Call `Health`. The `readiness` field lists each blocking graph, its state, the reason code and the task that blocks it.
2. Or probe `GET /readyz` on the observability listener: 200 when ready, 503 with the same blockers otherwise.
3. Read `Introspect { section: "startup" }` for the duration and outcome of each post-upgrade step, also from the previous boot.

Proof command: `cargo nextest run -p medulla-server-served-tests health_names_the_blocking_graph_and_reason`.

## What is the engine doing now?

```text
Introspect { section: "tasks" }      background jobs with state, cursor, last progress
Introspect { section: "requests" }   in-flight requests with phase and age
Introspect { section: "locks" }      lock holders and waiters with hold time
Introspect { section: "runtime" }    per-worker busy ratio; spinning workers are flagged
Introspect { section: "events" }     the last state transitions
Introspect { section: "build" }      version, git SHA, build-id, features
```

The caller needs the scope `observe:read`.

## Follow one request

Send `trace_id` (a W3C `traceparent`) in the request. Filter logs with `RUST_LOG=medulla::request=debug`. Every line of the request and of its tasks shows `request_id` and `trace_id`.

## Symbolize a production stack

1. Read the build-id from `Introspect { section: "build" }` or the startup log line.
2. Run `scripts/symbolize.sh <tag> <platform> <offset>...`. It downloads the symbol artifact of that tag and resolves each offset.

## Profile a busy worker

Call `Profile { kind: "Stacks", seconds: 10 }` with the scope `observe:profile`. Frames carry build-id and offset. Call `Profile { kind: "TaskDump", seconds: 1 }` for one consistent snapshot.

Proof command: `cargo nextest run -p medulla-server-served-tests chaos_stalled_task_and_spinning_loop_are_named`.
