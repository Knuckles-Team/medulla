# MEDULLA-OBJECT-STORAGE-BACKEND — Tasks

One task per row of `requirements.md`, producer first: the backend spec (R001) precedes the read path, the commit path, shared-root access, the archive and the cost gate. Each task names its row; the row holds the files, the caller chain and the proof. Rows that edit `crates/medulla-persistence/src/object_store_backend.rs` run in series.

## Tasks

- [ ] T001 [US1] MEDULLA-OBJECT-STORAGE-BACKEND-R001 Typed object-store backend specification
- [ ] T002 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002 Object-store read path
- [x] T010 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002.2 Build the object-store client from the spec secret references (resolve_object_store_client)
- [x] T011 [P] [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002.3 Loopback object-store fixture tests/_loopback_object_store.py and the store-upgrade layouts verb
- [ ] T015 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002.4 Server reads MEDULLA_STORAGE_BACKEND at startup and opens through the backend factory
- [ ] T016 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002.5 Cold open restores the root into the local store
- [ ] T003 [US2] MEDULLA-OBJECT-STORAGE-BACKEND-R002.1 Served cold open and corrupt-segment refusal over the loopback store
- [ ] T004 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003 Object-store commit path (commit before ack)
- [ ] T005 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003.1 Hot-path proof of the commit
- [ ] T006 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003.2 Graph copy from the redb store with a verified digest
- [ ] T017 [US3] MEDULLA-OBJECT-STORAGE-BACKEND-R003.3 Redb commit publishes to the object store before the ack, with first-commit bootstrap
- [ ] T007 [P] [US5] MEDULLA-OBJECT-STORAGE-BACKEND-R006 Archive segment put and get by sequence range
- [ ] T008 [US4] MEDULLA-OBJECT-STORAGE-BACKEND-R004 Shared-root multi-node access
- [ ] T012 [US4] MEDULLA-OBJECT-STORAGE-BACKEND-R004.1 Served two-engine cold open and rejoin over the loopback store
- [ ] T009 [US6] MEDULLA-OBJECT-STORAGE-BACKEND-R005 Comparative cost gate across releases
