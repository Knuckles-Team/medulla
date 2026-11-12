# MEDULLA-BRANCH-MERGE — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

Branch state has two homes, both existing surfaces: a registry row in the owner-store lineage, and an append-only commit chain with parent pointers. A branch head is an engine version; `main` and every branch are named heads over one commit DAG, so creating a branch copies no rows. The merge pass computes over `(base_head, source_head, target_head)`, classifies each entity as unchanged, adopted or conflicted, validates adoptions through the existing authorities, resolves epistemic conflicts through the decision engine, and publishes one atomic change through the existing cross-modal transaction and raft path.

```text
caller (arbora)
  -> Method::CreateBranch / BranchMerge / BranchDiff / BranchReadAt   crates/medulla-protocol-types/src/protocol/method/method_11.rs
  -> dispatch_graph_lifecycle_methods   crates/medulla-server-router/src/dispatch/router/graph_lifecycle.rs
  -> graph_ops::branch                  crates/medulla-h-graph/src/graph_ops/branch.rs
  -> PersistenceBackend::commit_mutation_batch_crossmodal   crates/medulla-server-api/src/api/persistence.rs
```

No new store, no new consensus layer and no second writer authority.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `crates/medulla-types`, `crates/medulla-storage`, `crates/medulla-server-api`, `crates/medulla-server-mutation`, `crates/medulla-raft`, `crates/medulla-h-graph`, `crates/medulla-compute`. No new third-party dependency.
- **Storage**: one new owner layout (`BranchRegistry`) under the owner-store lineage of `MEDULLA-DURABLE-KERNEL-R003`; commits live in the existing mutation-batch tables.
- **Testing**: served Python tests under `tests/test_branch_*.py` (L4) and one compute test `crates/medulla-compute/tests/line_merge.rs` (L2).
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Handler, protocol and storage additions to the existing engine crates.
- **Performance Goals**: Creating a branch is one registry write. A fast-forward merge scans no entity. Diff output is bounded by `branch.diff_page_size`.
- **Constraints**: A merge never rebases silently and never claims a rollback. Every refusal precedes any write. Commit-before-ack.
- **Scale/Scope**: 10 rows over 5 user stories, 8 `FR-` labels and 5 `SC-` labels.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Branches and merges run in the engine.
- **R-II. Every wait has a deadline**: pass. Merge publication reuses the commit path deadline.
- **R-III. Every row sets budgets and names a benchmark**: pass. `branch.diff_page_size` bounds the diff; the fast-forward path scans nothing.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates; the conflict decision crosses to `medulla-h-decide` through a `ConflictDecider` trait in `medulla-server-api`, so no handler calls a sibling handler.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-types/src/branch.rs                     new: Branch, BranchRef, BranchCommit, MergeOutcome, ConflictRecord
crates/medulla-types/src/versioning.rs                 new: KgTransaction, KgCommit, KgDiff
crates/medulla-storage/src/owner/branch_registry.rs    new: BranchRegistry owner layout
crates/medulla-storage/src/owner/lineage.rs            existing: ALL_LAYOUTS gains the layout
crates/medulla-server-api/src/api/conflict_decider.rs  new: ConflictDecider trait
crates/medulla-h-graph/src/graph_ops/branch.rs         new: handlers and merge_conflicts
crates/medulla-h-graph/src/graph_ops/branch/           new: merge.rs, conflict.rs, publish.rs, line_merge.rs
crates/medulla-compute/src/line_merge.rs               new: merge_lines
crates/medulla-protocol-types/src/protocol/method/method_11.rs  existing: four new Method variants
tests/test_branch_*.py                                 new: served tests
```

## Phase 0: Research

No unknowns that block planning; no research.md.

## Phase 1: Design

1. **View semantics, not copy-on-branch.** Branch creation costs one registry row and no graph data.
2. **Entity identity reuses existing authorities.** A node uses `(tenant, type, key)` and an edge uses `(source, target, relation_type)`.
3. **Conflicts are epistemic records, not heuristics.** The engine never guesses. An owner-pinned policy decides, or the conflict stays open and queryable.
4. **Closed outcome vocabulary.** Clients branch on `MergeOutcome`, never on prose.
5. **One commit chain.** A source changeset is the payload of a branch commit; no second chain exists.
6. **One line merge.** `merge_lines` in `medulla-compute` is the only diff3; `merge_file` adapts it.

Data entities are in [data-model.md](data-model.md); the shortest served proof is in [quickstart.md](quickstart.md); the seam has a document in `contracts/`.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row, producer first. The model and registry row precede writes, reads and merge. Tasks that edit `branch.rs` run in series; the line merge (T005) runs in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- The merge pass reuses the existing commit and validation authorities; the no-duplicate-authority check finds no parallel one.
- A public item in a handler crate has a non-test caller in `crates/medulla-server-router`. `MEDULLA-CONTRACT-R055` gates this.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- Release proof follows `MEDULLA-CONTRACT`.

## Production caller chain

- `Method::CreateBranch`, `Method::BranchMerge`, `Method::BranchDiff` and `Method::BranchReadAt` arms in `dispatch_graph_lifecycle_methods` (`crates/medulla-server-router/src/dispatch/router/graph_lifecycle.rs:7`, chained at `router.rs:145`) to the handlers in `crates/medulla-h-graph/src/graph_ops/branch.rs`.
- `handle_branch_merge` to `merge_entities` (`branch/merge.rs`), `resolve` (`branch/conflict.rs`) and `publish_merge` (`branch/publish.rs`) to `commit_mutation_batch_crossmodal` (`crates/medulla-server-api/src/api/persistence.rs:314`).
- `Method::BranchWrite` in `dispatch_graph_lifecycle_methods` to `handle_branch_write` (`crates/medulla-server-router/src/dispatch/router/branch_write.rs`), which resolves the target with `resolve_target`, commits the mutation to the branch target graph through `dispatch_graph_op`, then appends the commit with `BranchRegistry::append_commit`.
- Obsolete paths to delete: none; every path above is new.

## Cross-repo seams

- Served branch operations: `MEDULLA-BRANCH-MERGE-R001` provides, `ARBORA-OPS-R006` consumes. Contract: [arbora-branch-operations.md](contracts/arbora-branch-operations.md).
