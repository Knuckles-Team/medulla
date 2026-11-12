# MEDULLA-BRANCH-MERGE test-spec

Proof levels follow the integration proof standard of `specs/decision-engine/test-spec.md`: L2 property and golden vector, L3 served Rust, L4 served client. A row's acceptance test enters through the production dispatch with a tenant and principal context. No delivery is claimed; every test below is planned.

## Positive

- `tests/test_branch_create.py::test_branch_create_and_refusals` (planned, L4). Branch creation round-trips the
  identity. Each refusal (empty name, duplicate, reserved namespace, foreign tenant, unresolvable
  base) returns a typed error. Each leaves no registry entry. *(MEDULLA-BRANCH-MERGE-R001)*
- `tests/test_branch_scoped_mutation.py::test_branch_write_invisible_until_merge` (L4). A write on a branch
  shows on that branch. It stays invisible on main and on sibling branches. It is idempotent
  under a tenant-scoped branch idempotency key. *(MEDULLA-BRANCH-MERGE-R002)*
- `crates/medulla-types/src/branch.rs::tests::branch_commit_appends_one_parent_and_round_trips` (L2). A plain
  branch commit appends exactly one parent, the prior head, and round-trips through serde.
  `refusal_set_and_default_branch_are_closed` proves the typed refusal set is closed and the
  default trunk name is reserved. *(MEDULLA-BRANCH-MERGE-R002.1)*
- `tests/test_branch_merge_base.py::test_merge_outcomes_up_to_date_and_fast_forward` (planned, L4). Four fixture
  topologies (contained, descendant, diverged, disjoint) return the declared outcomes from the
  closed set. Fast-forward produces a single-parent receipt. Up-to-date publishes nothing.
  *(MEDULLA-BRANCH-MERGE-R003)*
- `tests/test_branch_three_way_merge.py::test_entity_level_outcomes`. Disjoint, one-sided, and
  both-sides fixtures each produce the expected published target state. Unchanged entities adopt
  without row copy. *(MEDULLA-BRANCH-MERGE-R004)*
- `tests/test_branch_epistemic_conflict.py::test_evidence_weighted_resolution_and_open_conflict`. Under
  a pinned epistemic policy, the winner selects per the policy. The resolved row keeps the
  contradiction link. It keeps both evidence refs. Without a policy, the conflict stays open with
  both candidates queryable. *(MEDULLA-BRANCH-MERGE-R005)*
- `tests/test_branch_time_travel.py::test_as_of_reads_and_diff`. As-of reads at three versions each
  equal that version's write set. The diff between versions equals the intervening commits.
  *(MEDULLA-BRANCH-MERGE-R006)*
- `tests/test_branch_merge_publish.py::test_atomic_two_parent_publication_and_replay`. The two-parent
  receipt reads back byte-identically. Replay against the same inputs returns the original
  receipt. *(MEDULLA-BRANCH-MERGE-R007)*

- `crates/medulla-compute/tests/line_merge.rs::clean_and_conflicting_hunks`. Disjoint hunks merge clean. Overlapping hunks and any binary overlap return `Conflict`. *(MEDULLA-BRANCH-MERGE-R008)*
- `tests/test_branch_landing_gate.py::test_failing_gate_blocks_merge_and_target_unchanged`. A merge into a protected branch with one failing required gate refuses with `LANDING_GATE_BLOCKED` and leaves the target head unchanged. *(MEDULLA-BRANCH-MERGE-R009)*
- `tests/test_branch_line_merge.py::test_disjoint_hunks_merge_and_overlap_conflicts`. Two edits to disjoint lines merge. One shared line conflicts with `SOURCE_CONFLICT`. *(MEDULLA-BRANCH-MERGE-R010)*

## Negative

- Foreign-tenant branch creation and reads refuse before any state change.
- A mutation naming an unknown branch refuses. A deleted branch refuses. A foreign-tenant branch
  refuses. The graph stays unchanged.
- A merge with no common ancestor refuses with a typed error. It never invents an implicit base.
- An adopted entity that fails constraint or ABox validation refuses the whole merge. The refusal
  is atomic. No partial merge publishes.
- A missing epistemic policy refuses resolution. Stale evidence refs refuse. Nonfinite confidence
  refuses. The engine leaves the entity unmerged. It never guesses.
- An as-of read at a version not on the branch chain refuses. A cross-tenant diff refuses. Both
  return typed errors.
- A target that advanced past its recorded head refuses as stale. The merge never rebases
  silently. It never claims a rollback.

## Integration

- Full agent loop: create branch, write, diff, merge (fast-forward and three-way), read the merged
  state on main with provenance intact.
- Two concurrent branches write disjoint entities. The merge is clean. The same two branches
  write the same entity. The pinned policy produces a typed conflict.
- Merge publication under the cluster feature replicates through raft. A follower catches up. It
  serves the merged state.

## Quality and release proof

- Clippy, test, jscpd, CCCC, dupehound, KISS gates pass on the merge pass.
- The no-duplicate-authority check confirms the merge reuses the existing commit and validation
  authorities. It adds no parallel authority.
- Release proof follows MEDULLA-CONTRACT. Exact-head CI receipts record in this directory.

## Test bindings

Each row's acceptance test binds its ID where the test lands. The lines below record the planned homes. The spec-trailers gate resolves the spec PR's trailer.

- `# spec: MEDULLA-BRANCH-MERGE-R001` — `tests/test_branch_create.py::test_branch_create_and_refusals` (planned, L4)
- `# spec: MEDULLA-BRANCH-MERGE-R002` — `tests/test_branch_scoped_mutation.py::test_branch_write_invisible_until_merge` (L4)
- `# spec: MEDULLA-BRANCH-MERGE-R003` — `tests/test_branch_merge_base.py::test_merge_outcomes_up_to_date_and_fast_forward` (planned, L4)
- `# spec: MEDULLA-BRANCH-MERGE-R004` — `tests/test_branch_three_way_merge.py::test_entity_level_outcomes` (planned, L4)
- `# spec: MEDULLA-BRANCH-MERGE-R005` — `tests/test_branch_epistemic_conflict.py::test_evidence_weighted_resolution_and_open_conflict` (planned, L4)
- `# spec: MEDULLA-BRANCH-MERGE-R006` — `tests/test_branch_time_travel.py::test_as_of_reads_and_diff` (planned, L4)
- `# spec: MEDULLA-BRANCH-MERGE-R007` — `tests/test_branch_merge_publish.py::test_atomic_two_parent_publication_and_replay` (planned, L4)
- `# spec: MEDULLA-BRANCH-MERGE-R008` — `crates/medulla-compute/tests/line_merge.rs::clean_and_conflicting_hunks` (planned, L2)
- `# spec: MEDULLA-BRANCH-MERGE-R009` — `tests/test_branch_landing_gate.py::test_failing_gate_blocks_merge_and_target_unchanged` (planned, L3)
- `# spec: MEDULLA-BRANCH-MERGE-R010` — `tests/test_branch_line_merge.py::test_disjoint_hunks_merge_and_overlap_conflicts` (planned, L4)
