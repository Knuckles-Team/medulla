# MEDULLA-SCM — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine holds a repository's source as a log of changesets over a content-addressed blob store. This plan adds the changeset record and its accept path, trees by query, derived history indexes, path grants, gate verdict records, bisect, git import and export, run provenance and graph-native symbol edits. It reuses the blob store, the branch commit chain, the version-checked write path and the typed verdict record; it adds no store and no second writer authority.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `medulla-server-blob`, `medulla-transaction`, `medulla-compute`, `medulla-enrich`, `medulla-types`, `medulla-server-auth`, `medulla-h-ingest`.
- **Storage**: Blob store for file bytes; changeset, checkpoint, index and verdict records in the engine store.
- **Testing**: Rust unit tests per crate, a seeded simulation under `tests/simulation/`, served Python tests under `tests/`, a nightly bench.
- **Target Platform**: Linux x86_64 and aarch64 hosts.
- **Project Type**: Library crates and a server handler crate.
- **Performance Goals**: A subtree read costs O(subtree); a changeset costs O(change); `SubmitSymbolEdit` p99 at most 30 ms (`MEDULLA-SCM-R013.7`).
- **Constraints**: Commit before ack; every refusal leaves the chain unchanged; cross-tenant reads use the request-boundary refusal.
- **Scale/Scope**: 22 leaf rows over 5 user stories.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Source lives in the engine blob store and chain.
- **R-II. Every wait has a deadline**: pass. Accept is a bounded version-checked write.
- **R-III. Every row sets budgets and names a benchmark**: pass. Limits are knobs of `SchedulerConfig`; `MEDULLA-SCM-R013.7` holds the bench.
- **R-IV. Crates are small, layered and cut over hard**: pass. Pure logic sits in `medulla-compute`; the handlers sit in `medulla-h-ingest` and call only lower crates.
- **Organization standards**: pass. Rows follow the spec standard and ecosystem boundaries.

## Project Structure

```text
crates/medulla-types/src/source_changeset.rs          existing: Changeset, Trailers, ChangedPath
crates/medulla-types/src/source_symbol_edit.rs        existing: SymbolEdit
crates/medulla-types/src/source_path_grant.rs         new: PathGrant
crates/medulla-types/src/scheduler_config.rs          existing: knobs added
crates/medulla-compute/src/source_symbol_edit.rs      existing: projection
crates/medulla-compute/src/source_tree.rs             new: tree_at
crates/medulla-compute/src/source_history.rs          new: history indexes
crates/medulla-compute/src/source_bisect.rs           new: next_step
crates/medulla-compute/src/source_git_objects.rs      new: git object build and parse
crates/medulla-enrich/src/symbol_edit_verify.rs       new: verify_roundtrip
crates/medulla-server-auth/src/access/path_grant.rs   new: check
crates/medulla-h-ingest/src/source_changeset.rs       new: accept, import, export
crates/medulla-h-ingest/src/source_tree.rs            new: read_tree
crates/medulla-h-ingest/src/source_history.rs         new: history handlers
crates/medulla-h-ingest/src/gate_verdict.rs           new: record
crates/medulla-h-ingest/src/source_bisect.rs          new: bisect handler
crates/medulla-h-ingest/src/source_symbol_edit.rs     new: handle_submit_symbol_edit
crates/medulla-h-ingest/src/repository_index.rs       existing: reindex_changeset added
tests/simulation/source_changesets.rs                 new: seeded simulation
benches/source_symbol_edit.rs                         new: nightly bench
```

## Phase 0: Research

none

## Phase 1: Design

### Architecture

A repository's source has exactly two homes in the engine. Both are existing surfaces.

1. **Blob store** — the existing content-addressed chunk store (`crates/medulla-server-blob/src/blob/store.rs`). Each
   file version stores once, keyed by its sha256 digest. The bounded streaming facade
   (`crates/medulla-server-blob/src/blob/stream.rs`) moves the bytes without buffering the whole file.
2. **Changeset chain** — the append-only branch chain of MEDULLA-BRANCH-MERGE. Each changeset names
   its target branch, its parents, and the paths it changed against the first parent. The chain
   is the source of truth for the repository files.

Everything else derives from the chain. Trees answer by replaying changesets from a snapshot
checkpoint. History indexes update incrementally. The code graph re-indexes the structurally
changed files. Gate verdicts and bisect decisions record against changeset ids.

### Reuse (inventory before new components)

| Need | Existing owner |
|---|---|
| Content-addressed store, dedup, refcount GC, S3 backend | blob CAS (`crates/medulla-server-blob/src/blob/store.rs`) |
| Bounded streaming of a blob to and from the CAS | streaming facade (`crates/medulla-server-blob/src/blob/stream.rs`) |
| Sealed create-only record guard | sealed record classes (MEDULLA-DURABLE-KERNEL-R057) |
| Append-only branch chain, commit-before-ack | branch commit path (MEDULLA-BRANCH-MERGE-R002) |
| File and line granularity three-way merge | merge pass (MEDULLA-BRANCH-MERGE-R004) |
| Version-checked concurrent writes, typed conflict | write conflict path (MEDULLA-DURABLE-KERNEL-R084) |
| Incremental derived series with lineage | derived series (MEDULLA-REPO-INGEST-R004) |
| Commit and change-coupling graph | history enrichment (MEDULLA-REPO-INGEST-R601) |
| Structural fingerprints, symbols, tests, Leiden communities | enrichment and community detection (MEDULLA-REPO-INGEST-R600/R603/R011) |
| Incremental indexing boundary with base-match refusal | incremental indexing (MEDULLA-REPO-INGEST-R006.4) |
| Universal read-RLS gate | read-RLS gate (MEDULLA-DURABLE-KERNEL-R029) |
| Typed verdict model | step verdict (MEDULLA-DURABLE-KERNEL-R089.1) |
| Replayable decision records | decision replay (MEDULLA-DECISION-ENGINE-R007) |
| Off-lock snapshot reads | snapshot path (MEDULLA-DURABLE-KERNEL-R023) |

No new store. No new consensus layer. No second writer authority.

### Interfaces

Wire surface at the request boundary; each method variant goes into `crates/medulla-protocol-types/src/protocol/method/method_10.rs` beside `SourceIngestStatus`, and each route into `crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs`.

- `SubmitChangeset { repo, branch, base_head, message, paths, trailers, run_trace_id }` returns an accept, a rebase receipt, or the typed conflict (R002, R003).
- `ReadTree { repo, rev, prefix, depth }` returns `{path, blob_digest, mode}` entries (R004).
- `ReadPathHistory { repo, path, rev, limit }` and `ReadBlame { repo, rev, path }` return the history and line-origin answers (R005).
- `RecordGateVerdict { changeset, gate_id, input_digest, verdict, evidence_digest, duration_ms }` returns the sealed verdict record (R008).
- `BisectStep { repo, branch, bad, good }` returns the next midpoint or the culprit (R009).
- `ImportGitHistory`, `ExportGitHistory` and `GitExportPack` move changesets to and from git (R010, R014).
- `SubmitSymbolEdit { request }` projects a symbol edit to one changeset (R013).

### Decisions

- **Changesets are the source of truth, not git objects.** The engine stores file bytes by
  digest and file changes as chain records. Git objects appear at the export boundary only.
- **One chain per repository branch.** The changeset chain is the branch commit chain of
  MEDULLA-BRANCH-MERGE. No parallel log.
- **Tree reads replay from checkpoints.** Full history replay stays available. The checkpoint
  bounds the normal cost.
- **Conflicts are typed, never silent.** Disjoint paths land in parallel. Overlapping paths
  rebase on the server or refuse with `SOURCE_CONFLICT{paths}`.
- **Provenance is mandatory for agent principals.** An agent changeset names its run trace. A
  human changeset may omit it.

### Graph-native edits (R013) and structured trailers (R002.1)

An agent edits a code symbol, not a file. The engine owns the projection from symbol to bytes.

```text
agent -> arbora source.edit_symbol (ARBORA-OPS-R848.4)
      -> Medulla SubmitSymbolEdit
         1. resolve symbol_id at base_rev -> (path, blob_digest, byte span)   code graph, MEDULLA-REPO-INGEST-R600
         2. check anchor_span_digest == digest(span bytes)                     else EDIT_BASE_STALE
         3. splice ops per file, descending span order                         bytes outside spans unchanged
         4. parse_file (crates/medulla-compute/src/parser/tree_sitter.rs:249) re-parse each changed file
         5. compare symbols: edited resolves, untouched keep structure hash    else EDIT_ROUNDTRIP_MISMATCH
         6. write blobs (R001), build one changeset (R002), accept (R003)
         7. re-index (R006), derive lifecycle edges (MEDULLA-SDLC-R001)
      <- accept, server rebase, or typed refusal
```

The file projection is the authority for bytes. The code graph is the authority for structure.
The re-parse ties both: a changeset lands only when the new bytes parse back to the declared
symbol change. A plain file changeset through `SubmitChangeset` stays available for files with
no parser.

Trailers become typed fields on the changeset. The git bridge of R010 maps them to and from the
commit message trailers. MEDULLA-SDLC reads the typed fields, never the message text.

## Phase 2: Task approach

`tasks.md` has one task per leaf row. Order: blob store and record (R001, R002), acceptance (R003), tree and history (R004, R005), re-index and access (R006, R007), verdicts and bisect (R008, R009), git (R010, R014), provenance (R012), symbol edits (R013), then the simulation (R011). Rows on different files run in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- The changeset record and tree query pass the no-duplicate-authority check against the existing commit and blob modules.
- `medulla-h-ingest` adds `medulla-transaction` and `medulla-enrich` dependencies only; it calls no sibling handler crate.
- A public item in a compute crate has a non-test caller under a handler or server crate (`MEDULLA-CONTRACT-R055`).
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- `Method::SubmitChangeset` → `crates/medulla-server-router/src/dispatch/router/graph_lifecycle.rs` → `submit_changeset.rs::handle_submit_changeset` → `crates/medulla-h-ingest/src/source_changeset.rs::verify_submission` and `::accept` → `stream_blob_put` (`crates/medulla-server-blob/src/blob/stream.rs:35`), `medulla_transaction::occ::disjoint` (`crates/medulla-transaction/src/occ.rs:79`), the branch commit path, then `reindex_changeset` and the history indexes.
- `Method::ReadTree` and the history methods → the same route file → `source_tree.rs::read_tree` and `source_history.rs` → `crates/medulla-compute/src/source_tree.rs::tree_at` and `source_history.rs`.
- `Method::SubmitSymbolEdit` → the same route file → `source_symbol_edit.rs::handle_submit_symbol_edit` → `project_symbol_edit` (`crates/medulla-compute/src/source_symbol_edit.rs:145`) → `verify_roundtrip` → `accept`.
- Obsolete path to delete: the planned `gate_verdicts` field of `Changeset`.

## Cross-repo seams

- Symbol edit serving: `MEDULLA-SCM-R013.4` provides, `ARBORA-OPS-R848.4` consumes; `MERISTEM-SCM-R002` applies the answer. Contract: [arbora-symbol-edit.md](contracts/arbora-symbol-edit.md).
- Git bridge export: `MEDULLA-SCM-R014` provides, `ARBORA-OPS-R851` consumes. Contract: [arbora-git-export.md](contracts/arbora-git-export.md).
- Run provenance: `MEDULLA-SCM-R012` provides, `MERISTEM-SCM-R001` consumes. Contract: [harness-run-provenance.md](contracts/harness-run-provenance.md).
