# MEDULLA-BRANCH-MERGE — Data model

This file describes the entities of the branch registry, the commit chain and the conflict record.

## Entities

### Branch

- Fields: `tenant`, `graph`, `name`, `base_version: u64`, `head_commit: String`, `created_by`, `created_at`, `deleted: bool`.
- Owner: `crates/medulla-types/src/branch.rs`; stored in `BranchRegistry` (`crates/medulla-storage/src/owner/branch_registry.rs`).
- Rule: the name matches `[a-z0-9][a-z0-9._/-]{0,127}`; `main` and the prefix `eg/` are reserved.

### BranchRef

- Fields: `tenant`, `graph`, `name`. The key of a branch.

### BranchCommit

- Fields: `id`, `branch`, `parents`, `engine_version`, `manifest_digest`, `actor`, `created_at`, `generation`.
- Relation: every commit has the prior head as parent; a merge commit has two parents. `main` and every branch are named heads over one commit DAG.

### MergeOutcome

- Variants: `up_to_date`, `fast_forward`, `merged`, `conflict`. The set is closed.

### ConflictRecord

- Fields: `entity`, `candidate_a`, `candidate_b`, `state` (open or resolved), `policy_digest`.
- Relation: each candidate carries `valid_from`, `valid_until`, `tx_from`, `tx_until`, source reliability, confidence and evidence refs. A resolved record keeps a contradiction link.

### KgTransaction, KgCommit, KgDiff

- Owner: `crates/medulla-types/src/versioning.rs`. `KgDiff` lists added, updated and deleted entities as (entity, operation, image ref).
