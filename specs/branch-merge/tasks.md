# MEDULLA-BRANCH-MERGE — Tasks

One task per leaf row of `requirements.md`, producer first: the branch model (US1) precedes the writes, reads and merge rows that use it. Each task names its row; the row holds the files, the caller chain and the proof. Rows that edit `crates/medulla-h-graph/src/graph_ops/branch.rs` run in series.

## Tasks

- [ ] T001 [US1] MEDULLA-BRANCH-MERGE-R001 Typed branch model and CreateBranch
- [x] T0021 [US1] MEDULLA-BRANCH-MERGE-R002.1 Typed branch-slice model for branch-scoped mutations
- [x] T002 [US1] MEDULLA-BRANCH-MERGE-R002 Branch-scoped mutations on one commit chain
- [ ] T003 [US2] MEDULLA-BRANCH-MERGE-R006 Branch diff and time-travel reads
- [ ] T004 [US3] MEDULLA-BRANCH-MERGE-R003 Merge base and fast-forward
- [ ] T005 [P] [US3] MEDULLA-BRANCH-MERGE-R008 Line-level three-way merge for file blobs
- [ ] T006 [US3] MEDULLA-BRANCH-MERGE-R004 Entity-level three-way merge
- [ ] T007 [US4] MEDULLA-BRANCH-MERGE-R005 Epistemic conflict resolution
- [ ] T008 [US3] MEDULLA-BRANCH-MERGE-R007 Atomic merge publication
- [ ] T009 [US3] MEDULLA-BRANCH-MERGE-R009 Landing policy check at merge publish
- [ ] T010 [US3] MEDULLA-BRANCH-MERGE-R010 Line-granularity file merge for source changesets
