# MEDULLA-REPO-INGEST — Quickstart

The shortest served proof indexes a small repository fixture through the production dispatch and reads the result back.

## Scenario

1. Start the served harness of `crates/medulla-server-served-tests`, which builds a tenant, a principal and a graph.
2. Send one `Method::IndexRepository` batch for a fixture repository with two refs that share one blob.
3. Read the committed nodes and edges and check one parse per unique blob and one edge per relation triple.
4. Send a batch above the commit budget and check the stable refusal and zero writes.

## Commands

Run the proof for `MEDULLA-REPO-INGEST-R008`:

```text
cargo test -p medulla-h-ingest oversized_batch_is_refused_with_the_stable_retry_code_and_commits_nothing
```

Run the served tests of the repository index:

```text
cargo test -p medulla-server-served-tests medulla_h_ingest_repository_index_tests
```
