# MEDULLA-OBJECT-STORAGE-BACKEND — Tasks

One task per row of `requirements.md`, producer first: the backend spec (R001) precedes the read path, the commit path, shared-root access, the archive and the cost gate. Each task names its row; the row holds the files, the caller chain and the proof. Rows that edit `crates/medulla-persistence/src/object_store_backend.rs` run in series.

## Tasks

- [ ] T001 [US1] MEDULLA-OBJECT-STORAGE-BACKEND-R001 Typed object-store backend specification
- [ ] T002 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002 Object-store read path
- [ ] T003 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002.1 Hot-path proof of the cold open
- [ ] T004 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003 Object-store commit path (commit before ack)
- [ ] T005 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003.1 Hot-path proof of the commit
- [ ] T006 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003.2 Graph copy from the redb store with a verified digest
- [ ] T007 [P] [US5] MEDULLA-OBJECT-STORAGE-BACKEND-R006 Archive segment put and get by sequence range
- [ ] T008 [US4] MEDULLA-OBJECT-STORAGE-BACKEND-R004 Shared-root multi-node access
- [ ] T009 [US6] MEDULLA-OBJECT-STORAGE-BACKEND-R005 Comparative cost gate across releases
