# MEDULLA-OBJECT-STORAGE-BACKEND — Quickstart

The shortest served proof opens a graph from an object root after the local data is gone.

## Scenario

1. Start a loopback S3-compatible store and an engine whose backend spec is `object_store`.
2. Write 100 nodes, stop the engine and delete its local data directory.
3. Start a second engine on the same root and read the 100 nodes.

## Commands

Run the backend-spec unit proof of `MEDULLA-OBJECT-STORAGE-BACKEND-R001`:

```text
cargo test -p medulla-storage backend_spec
```

Run the cold-open proof of `MEDULLA-OBJECT-STORAGE-BACKEND-R002.1` (planned):

```text
uv run pytest tests/test_object_storage_reads.py::test_cold_open_from_root
```
