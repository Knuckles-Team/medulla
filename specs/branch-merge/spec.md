# MEDULLA-BRANCH-MERGE — Agent branching, three-way merge and epistemic conflict resolution

Component: Branching and merge

## Outcome

An agent or a person opens a named branch. The branch starts from the current engine state of a graph. The agent writes to the branch in isolation. The branch merges back with a graph-wide atomic merge. Every branch diffs against any other version and reads at any point in its own commit history. A merge is entity-level three-way. A merge conflict is an epistemic conflict: both candidates keep their bitemporal windows and their provenance. Resolution uses evidence weights under an owner-pinned policy. An unresolved conflict stays a typed, queryable state and is never a silent last-write-wins overwrite.

This gives long-horizon agents a reviewable write path. Each agent or task gets its own branch, and merge happens on review. Time travel spans the whole graph. The merge model is evidence-weighted and validity-aware, and its conflict receipts are first-class epistemic records.

## Scope

Owner: `medulla` (native Rust), sole owner. The code lands in `crates/medulla-types` (branch types), `crates/medulla-storage` (registry layout), `crates/medulla-h-graph` (handlers in `graph_ops/branch.rs`), `crates/medulla-compute` (line merge), and the protocol and router crates for the new methods. Non-goals: the engine never reads a principal from a request body, never merges silently, and never keeps a second commit chain. Arbora owns admission and the served door. Source changesets (`MEDULLA-SCM`) use the branch commit chain; this spec owns the chain.

Why a spec: no other spec owns the branch registry, the commit DAG, the merge outcomes or the conflict record.

## User Scenarios & Testing

### User Story 1 - Open a branch and write to it in isolation (Priority: P1)

The user is an agent or a person who needs a private write path. The outcome is a branch whose writes stay invisible to `main` and to sibling branches until merge.

**Acceptance scenarios**

1. **Given** a tenant holds a graph, **When** the caller creates branch `feature/a` through `CreateBranch`, **Then** one registry row exists and no graph data is copied.
2. **Given** the branch exists, **When** the caller writes an entity to it, **Then** the branch shows the entity, `main` does not, and a replay of the same idempotency key returns the original receipt.
3. **Given** a name is empty, taken, reserved or the tenant is foreign, **When** the caller creates the branch, **Then** the engine refuses with the typed code and writes no row.

Independent test: the bound tests of `MEDULLA-BRANCH-MERGE-R001` and `MEDULLA-BRANCH-MERGE-R002`.

### User Story 2 - Diff a branch and read it at any version (Priority: P2)

The user is a reviewer. The outcome is a typed change feed and an as-of read for any version on the branch chain.

**Acceptance scenarios**

1. **Given** a branch with three versions, **When** the reviewer reads each version, **Then** each read equals that version's write set.
2. **Given** two versions, **When** the reviewer asks for the diff, **Then** it lists the added, updated and deleted entities and equals the intervening commits.
3. **Given** a version not on the chain, **When** the reviewer reads it, **Then** the engine refuses with `BRANCH_VERSION_NOT_ON_CHAIN`.

Independent test: the bound test of `MEDULLA-BRANCH-MERGE-R006`.

### User Story 3 - Merge a branch atomically (Priority: P1)

The user is a reviewer who accepts a branch. The outcome is one of four closed outcomes, published atomically with a two-parent receipt.

**Acceptance scenarios**

1. **Given** the source head descends from the target head, **When** the reviewer merges, **Then** the target moves to the source head with no entity scan.
2. **Given** both sides changed different entities, **When** the reviewer merges, **Then** both changes publish in one atomic commit and an unchanged entity is not copied.
3. **Given** the target moved after the merge was planned, **When** the merge publishes, **Then** it refuses with `MERGE_TARGET_STALE` and changes nothing.
4. **Given** a required gate has no passing verdict, **When** the merge targets a protected branch, **Then** it refuses with `LANDING_GATE_BLOCKED`.

Independent test: the bound tests of `MEDULLA-BRANCH-MERGE-R003`, `MEDULLA-BRANCH-MERGE-R004`, `MEDULLA-BRANCH-MERGE-R007` and `MEDULLA-BRANCH-MERGE-R009`.

### User Story 4 - Resolve a conflict from evidence, or keep it open (Priority: P1)

The user is an owner who pins a conflict policy. The outcome is a winner chosen from evidence, or an open conflict that stays queryable.

**Acceptance scenarios**

1. **Given** both sides changed one entity and a policy is pinned, **When** the merge runs, **Then** the winner follows the policy and the resolved row keeps the contradiction link and both evidence refs.
2. **Given** no policy is pinned, **When** the merge runs, **Then** the entity stays unmerged and the open conflict lists both candidates through `WITH KNOWLEDGE`.

Independent test: the bound test of `MEDULLA-BRANCH-MERGE-R005`.

### User Story 5 - Merge source files by line (Priority: P2)

The user is an agent that submits a changeset. The outcome is a line-level merge of file blobs, or a typed conflict.

**Acceptance scenarios**

1. **Given** two edits touch disjoint lines of one file, **When** the changeset lands, **Then** the merged blob holds both edits.
2. **Given** two edits touch one line, or the blob is binary, **When** the changeset lands, **Then** the engine refuses with `SOURCE_CONFLICT` naming the path.

Independent test: the bound tests of `MEDULLA-BRANCH-MERGE-R008` and `MEDULLA-BRANCH-MERGE-R010`.

### Edge Cases

- A branch whose tenant differs from the verified tenant refuses before any state change.
- Two branches with no common ancestor refuse with `BRANCH_NO_COMMON_ANCESTOR`; the engine never invents a base.
- An adopted entity that fails schema, constraint or ABox validation refuses the whole merge and publishes nothing.
- Stale evidence, a missing policy and a nonfinite confidence each abstain and leave the entity unmerged.
- A deleted branch refuses writes with `BRANCH_DELETED`.

### Assumptions

- The commit chain, the idempotency rules and the raft path of `MEDULLA-DURABLE-KERNEL` are the only commit machinery; this spec adds no store and no consensus layer.
- The conflict policy is pinned by the owner; the engine ships no default winner rule beyond `MEDULLA-DECISION-ENGINE-R828`.
- Source changesets and gate verdicts are owned by `MEDULLA-SCM`; this spec consumes them by row ID.

## Requirements

- **FR-001**: A branch is a registry row and copies no graph data. Rows: `MEDULLA-BRANCH-MERGE-R001`.
- **FR-002**: Branch writes stay on one commit chain and stay invisible until merge. Rows: `MEDULLA-BRANCH-MERGE-R002`.
- **FR-003**: The merge outcome comes from a closed set. Rows: `MEDULLA-BRANCH-MERGE-R003`.
- **FR-004**: A three-way merge classifies each entity and validates every adoption. Rows: `MEDULLA-BRANCH-MERGE-R004`.
- **FR-005**: A conflict resolves only from evidence under a pinned policy, or stays open. Rows: `MEDULLA-BRANCH-MERGE-R005`.
- **FR-006**: Every version on a chain reads and diffs. Rows: `MEDULLA-BRANCH-MERGE-R006`.
- **FR-007**: A merge publishes atomically with a two-parent receipt. Rows: `MEDULLA-BRANCH-MERGE-R007`, `MEDULLA-BRANCH-MERGE-R009`.
- **FR-008**: File blobs merge by line; binary blobs conflict. Rows: `MEDULLA-BRANCH-MERGE-R008`, `MEDULLA-BRANCH-MERGE-R010`.

## Success Criteria

- **SC-001**: Creating a branch writes exactly one registry row and zero graph rows. Rows: `MEDULLA-BRANCH-MERGE-R001`.
- **SC-002**: A branch write is absent from `main` in 100 percent of reads before merge. Rows: `MEDULLA-BRANCH-MERGE-R002`.
- **SC-003**: Every merge returns one of the four declared outcomes. Rows: `MEDULLA-BRANCH-MERGE-R003`.
- **SC-004**: A failed or stale merge leaves the target head unchanged in 100 percent of cases. Rows: `MEDULLA-BRANCH-MERGE-R007`.
- **SC-005**: No conflict resolves by last-write-wins. Rows: `MEDULLA-BRANCH-MERGE-R005`.

## Requirements and acceptance

Every requirement ID this spec owns lives in [requirements.md](requirements.md): `MEDULLA-BRANCH-MERGE-R001` to `R010`. Architecture and live wiring live in [plan.md](plan.md). The entities are in [data-model.md](data-model.md) and the shortest served proof is in [quickstart.md](quickstart.md). The spec accepts when every row reaches `VERIFIED`. A planned or tested item is not a landed item.

## Traceability

See [test-spec.md](test-spec.md).
