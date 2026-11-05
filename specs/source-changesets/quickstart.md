# MEDULLA-SCM — Quickstart

The shortest served proof submits a changeset, reads the tree and the blob back, and reads the history of the changed path.

## Scenario

1. Start the served harness of `crates/medulla-h-ingest` with one tenant, one principal and a path grant on the repository prefix.
2. Send `SubmitChangeset` with two changed files, then `ReadTree` at the new head and a blob read by digest.
3. Send `ReadPathHistory` and `ReadBlame` for one changed path and check that each answer names the changeset.
4. Send two `SubmitChangeset` requests on one path from the same base and check that one lands and one returns `SOURCE_CONFLICT`.

## Commands

Run the served proofs of the family:

```text
uv run pytest tests/test_scm_blob.py tests/test_scm_changeset.py tests/test_scm_accept.py tests/test_scm_tree.py tests/test_scm_history.py
```

Run the symbol edit proofs and the existing unit proofs:

```text
uv run pytest tests/test_scm_symbol_edit_served.py
cargo test -p medulla-types source_changeset
cargo test -p medulla-compute --test source_symbol_edit
```

Run the seeded simulation:

```text
SIM_SEEDS=16 cargo test --test source_changesets
```
