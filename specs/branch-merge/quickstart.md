# MEDULLA-BRANCH-MERGE — Quickstart

The shortest served proof creates a branch, writes to it, and merges it back.

## Scenario

1. Start a loopback engine with one tenant and one graph.
2. Send `CreateBranch` for `feature/a`, write one node to the branch, and read `main`.
3. Send `BranchMerge` from `feature/a` into `main` and read the node on `main`.

## Commands

Run the proof for `MEDULLA-BRANCH-MERGE-R001` and `MEDULLA-BRANCH-MERGE-R002` (planned):

```text
uv run pytest tests/test_branch_create.py tests/test_branch_scoped_mutation.py
```

Run the merge proof of `MEDULLA-BRANCH-MERGE-R007` (planned):

```text
uv run pytest tests/test_branch_merge_publish.py
```
