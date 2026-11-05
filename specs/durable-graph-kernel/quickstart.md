# MEDULLA-DURABLE-KERNEL — Quickstart

The shortest served proof drives a commit, a restart and a refusal through the production dispatch.

## Scenario

1. Start the served harness of `crates/medulla-server-served-tests`, which builds a tenant, a principal and a data directory.
2. Submit one MutationBatch with graph and table members and read it back.
3. Restart the engine over the same directory and read the same batch again.
4. Submit a batch for a foreign tenant and check the stable refusal.

## Commands

Run the lazy-open proof for `MEDULLA-DURABLE-KERNEL-R006`:

```text
cargo test -p medulla-server-served-tests boot_serves_unrelated_operations_when_the_agent_library_open_fails
```

Run the served Python tests of the kernel:

```text
uv run pytest tests -k "task_queue or bitemporal or change_feed"
```
