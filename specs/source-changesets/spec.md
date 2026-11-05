# MEDULLA-SCM — Source changesets as the source of truth

Owner: Medulla (`EG`). Sole owner.
Cross-repo IDs: `ARBORA-OPS-R848.4` (symbol edit serving), `ARBORA-OPS-R851` (git bridge), `MERISTEM-SCM-R001` and `MERISTEM-SCM-R002` (harness side). The served `source.*` operations, the client transport, the gate split and the git mirror sync belong to their owners.

## User outcome

The engine holds a repository's source as a log of changesets. Each changeset stores
only the paths changed against its first parent. File bytes live once in the
content-addressed blob store. Trees, history and line attribution derive from the
changeset log.

A coding agent reads a tree at any revision without a clone. It submits a changeset.
It receives an accept, a server rebase, or a typed conflict. Concurrent writers on
disjoint paths all land with no global lock. A subtree read at any revision costs
time in proportion to the subtree, never the history length. Gate verdicts record
against each changeset. A failed post-landing gate yields a culprit and a replayable
revert-or-fix-forward decision. A repository exports to git and imports back with
identical commit and tree object ids.

This gives agents a write path over source with no local clone and no push step.
Each repository moves to engine authority on its own wave. A git mirror stays in
sync. A rollback returns authority to the mirror.

## User Scenarios & Testing

### User Story 1 - Submit a changeset and read it back by digest (Priority: P1)

The user is a coding agent. The outcome is a write path over source with no local clone: file bytes store once by digest and a changeset stores only the changed paths.

**Acceptance scenarios**

1. **Given** two identical file versions, **When** the agent submits them through `SubmitChangeset`, **Then** one blob is stored and a read by digest returns the bytes.
2. **Given** a three-file repository with two files changed, **When** the agent submits the change, **Then** the changeset holds two path entries against the first parent.
3. **Given** two writers on disjoint paths and two on one path, **When** all submit, **Then** the disjoint writers both land, and the overlapping pair lands one and gets a typed `SOURCE_CONFLICT`.

Independent test: the bound tests of `MEDULLA-SCM-R002` and `MEDULLA-SCM-R003` run this story through the served method.

### User Story 2 - Read trees, history and blame at any revision (Priority: P1)

The user is an agent or reviewer. The outcome is a subtree, a path history or a blame answer whose cost follows the subtree, not the history length.

**Acceptance scenarios**

1. **Given** a ten-revision chain, **When** the user reads a subtree at each revision, **Then** each answer equals a full replay.
2. **Given** a renamed file, **When** the user reads blame, **Then** each line maps to its last changeset across the rename.

Independent test: the bound tests of `MEDULLA-SCM-R004` and `MEDULLA-SCM-R005` run this story through served methods.

### User Story 3 - Gate verdicts, bisect and revert decisions (Priority: P2)

The user is a release owner. The outcome is a recorded verdict per changeset, a culprit after a failed post-landing gate and a replayable revert-or-fix-forward decision.

**Acceptance scenarios**

1. **Given** a recorded pass and a recorded fail verdict, **When** the user reads both, **Then** each reads back sealed and an overwrite is refused.
2. **Given** a failing change seeded mid-chain, **When** the user bisects, **Then** the culprit is named and the decision replays byte for byte.

Independent test: the bound tests of `MEDULLA-SCM-R008` and `MEDULLA-SCM-R009` run this story through served methods.

### User Story 4 - Export to git and import back with identical ids (Priority: P2)

The user is an operator of a git mirror. The outcome is that a repository exports to git and imports back with the same commit and tree object ids, and that run provenance and path grants hold on the way.

**Acceptance scenarios**

1. **Given** twenty changesets, **When** the operator exports and imports them, **Then** commit and tree ids match for both object formats.
2. **Given** an agent principal, **When** it submits a changeset with no run trace, **Then** the submission is refused with `AGENT_RUN_TRACE_MISSING`.
3. **Given** a principal with a prefix grant, **When** it writes outside the prefix, **Then** the write is refused with `PATH_WRITE_DENIED`.

Independent test: the bound tests of `MEDULLA-SCM-R010`, `MEDULLA-SCM-R007` and `MEDULLA-SCM-R012` run this story through served methods.

### User Story 5 - Edit a code symbol instead of a file (Priority: P2)

The user is a coding agent. The outcome is that naming a symbol and its new text yields one changeset whose bytes parse back to the declared change.

**Acceptance scenarios**

1. **Given** a symbol at a base revision, **When** the agent submits a `replace_body` op, **Then** one changeset lands, bytes outside the span are unchanged and the new `code_context` answer shows the edit.
2. **Given** an op whose span digest differs from the symbol, **When** the agent submits it, **Then** `EDIT_BASE_STALE` is returned and nothing lands.

Independent test: the bound tests of `MEDULLA-SCM-R013.4` run this story through the served method.

### Edge Cases

- A changeset with an empty path list, or a parent not on the branch chain, is refused and the chain stays unchanged.
- A blob whose declared digest differs from the computed digest is refused.
- A base that left the branch chain refuses with `CHANGESET_BASE_STALE`; the engine never rebases silently.
- A tree read at a revision not on the chain, or across tenants, is refused.
- A binary blob refuses blame with `BLAME_BINARY`.
- A git export with a failing mid-stream tree read returns no partial pack.
- A path with no grant is denied.

## Requirements

- **FR-001**: Content-addressed blob store with refusals. Rows: `MEDULLA-SCM-R001`.
- **FR-002**: Changeset record with first-parent paths and structured trailers. Rows: `MEDULLA-SCM-R002`, `MEDULLA-SCM-R002.1`.
- **FR-003**: Path-set acceptance, shared disjoint check and server rebase. Rows: `MEDULLA-SCM-R003`, `MEDULLA-SCM-R003.1`.
- **FR-004**: Tree materialization by query. Rows: `MEDULLA-SCM-R004`.
- **FR-005**: Derived history indexes. Rows: `MEDULLA-SCM-R005`, `MEDULLA-SCM-R005.1`, `MEDULLA-SCM-R005.2`, `MEDULLA-SCM-R005.3`.
- **FR-006**: Code graph stays live with changesets. Rows: `MEDULLA-SCM-R006`.
- **FR-007**: Path-scoped access control. Rows: `MEDULLA-SCM-R007`.
- **FR-008**: Gate verdicts are records. Rows: `MEDULLA-SCM-R008`.
- **FR-009**: Bisect and revert decisions. Rows: `MEDULLA-SCM-R009`.
- **FR-010**: Git import and export fidelity and served export. Rows: `MEDULLA-SCM-R010`, `MEDULLA-SCM-R014`.
- **FR-011**: Simulation harness over accept, rebase and replay. Rows: `MEDULLA-SCM-R011`.
- **FR-012**: Agent principals and run provenance. Rows: `MEDULLA-SCM-R012`.
- **FR-013**: Graph-native code edits. Rows: `MEDULLA-SCM-R013`.

## Success Criteria

- **SC-001**: A changeset costs storage in proportion to its changed paths, never the repository size. Rows: `MEDULLA-SCM-R001`, `MEDULLA-SCM-R002`, `MEDULLA-SCM-R003`.
- **SC-002**: A subtree read at any revision costs time in proportion to the subtree. Rows: `MEDULLA-SCM-R004`, `MEDULLA-SCM-R005`.
- **SC-003**: A repository exports to git and imports back with identical commit and tree ids. Rows: `MEDULLA-SCM-R010`.
- **SC-004**: Concurrent writers on disjoint paths all land with no global lock. Rows: `MEDULLA-SCM-R003`, `MEDULLA-SCM-R011`.
- **SC-005**: A symbol edit lands only when the new bytes parse back to the declared change. Rows: `MEDULLA-SCM-R013`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- The branch commit chain of `MEDULLA-BRANCH-MERGE-R002` is the only chain; this spec adds no second log.
- Each repository moves to engine authority on its own wave and a git mirror stays in sync.

## Why a new spec

MEDULLA-BRANCH-MERGE owns entity branches and merges, not file changesets. MEDULLA-REPO-INGEST indexes external revisions and does not hold the source of truth. No other spec owns a file changeset log, trees by query, or per-repository source authority. This spec owns those parts; each linked part stays with its owner.
