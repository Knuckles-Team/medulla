# MEDULLA-OBJECT-STORAGE-BACKEND test-spec

Proof levels follow the integration proof standard of `specs/decision-engine/test-spec.md`: L1 unit, L2 property and golden vector, L3 served Rust, L4 served client, L6 nightly. A test marked planned does not exist yet; no delivery is claimed.

## Positive

- `tests/test_object_storage_backend_spec.py::test_backend_spec_roundtrip_and_refusals`. Both backend
  variants round-trip. A missing secret reference refuses. An unknown variant refuses. An
  unknown layout refuses. Each refusal is typed. *(MEDULLA-OBJECT-STORAGE-BACKEND-R001)*
- `crates/medulla-storage/tests/object_reads.rs::versioned_reads_and_corruption_refusal`. Reads at three
  versions each equal that version's write set. A corrupted segment refuses with a typed error
  that names the version. Earlier versions stay readable. *(MEDULLA-OBJECT-STORAGE-BACKEND-R002)*
- `crates/medulla-persistence/src/object_store_backend/tests.rs::commit_before_ack_and_partial_publish`. A commit acks
  only after the manifest publish. An interrupted publish reopens the graph at the prior whole
  version. A concurrent CAS loss yields exactly one head.
  *(MEDULLA-OBJECT-STORAGE-BACKEND-R003)*
- `tests/test_object_storage_shared_root.py::test_multi_node_cold_open_and_rejoin`. Two nodes open the
  same root. They serve identical state. A node that drops local state rejoins within the
  bounded re-fetch. *(MEDULLA-OBJECT-STORAGE-BACKEND-R004)*
- `tests/test_comparative_cost_gate.py::test_regression_guard_and_claim_eligibility`. The stamped
  JSONL records all four dimensions for a clean run. An injected fixed-cost regression fails the
  release guard. *(MEDULLA-OBJECT-STORAGE-BACKEND-R005)*

- `crates/medulla-storage/tests/object_archive.rs::overlap_and_digest_refuse`. An overlapping range refuses with `ARCHIVE_RANGE_OVERLAP`. A digest mismatch refuses with `ARCHIVE_DIGEST_MISMATCH`. *(MEDULLA-OBJECT-STORAGE-BACKEND-R006)*
- `tests/test_graph_copy_served.py::test_digest_mismatch_leaves_destination_empty`. A corrupted batch refuses and leaves the destination empty; a clean copy reads from the root. *(MEDULLA-OBJECT-STORAGE-BACKEND-R003.2)*
- `tests/test_object_storage_reads.py::test_cold_open_from_root`. A second engine on the same root reads all 100 nodes. *(MEDULLA-OBJECT-STORAGE-BACKEND-R002.1)*
- `tests/test_object_storage_commits.py::test_ack_follows_head_update`. A kill between segment upload and `HEAD` update keeps acked writes and drops the unacked write. *(MEDULLA-OBJECT-STORAGE-BACKEND-R003.1)*

## Negative

- An object-store spec without a declared durability class refuses at open.
- A manifest that names a missing segment refuses the read with a typed error. The graph's
  earlier versions stay readable.
- The engine never acks a client before the manifest publish is durable.
- A node without a placement grant for the tenant refuses before reads.
- A stale local cache behind a newer manifest discards. It never serves.
- A comparative run missing its environment stamp is claim-ineligible. A run missing its pinned
  revision is claim-ineligible. Neither enters the gate's evidence.

## Integration

- Full lifecycle on object storage: open, write, read at the prior version, second node reopens,
  commit from the second node, both nodes read the new version.
- Crash injection lands between segment write and manifest publish. The engine restarts. State
  equals the prior version exactly.
- Two nodes race the CAS on the same version publish. Exactly one publisher wins. The loser
  refuses as stale. It reconciles without a second head.

## Quality and release proof

- Clippy, test, jscpd, CCCC, dupehound, KISS gates pass on the new binding and harness.
- The no-duplicate-authority check confirms the object-store binding shares the store-authority
  chokepoint.
- Release proof follows MEDULLA-CONTRACT. Exact-head CI receipts record in this directory.

## Test bindings

Each row's acceptance test binds its ID where the test lands. The lines below record the homes. The spec-trailers gate resolves the spec PR's trailer.

- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R001` — `crates/medulla-storage/src/backend_spec.rs::tests::both_variants_round_trip_over_the_wire` and `::refusals_are_typed_and_pure` (existing, L1); `tests/test_object_storage_backend_spec.py::test_backend_spec_roundtrip_and_refusals` (planned, L4)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002` — `crates/medulla-storage/tests/object_reads.rs::versioned_reads_and_corruption_refusal` (planned, L2)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002.1` — `tests/test_object_storage_reads.py::test_cold_open_from_root` (planned, L4)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003` — `crates/medulla-persistence/src/object_store_backend/tests.rs::commit_before_ack_and_partial_publish` (planned, L3)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003.1` — `tests/test_object_storage_commits.py::test_ack_follows_head_update` (planned, L4)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003.2` — `tests/test_graph_copy_served.py::test_digest_mismatch_leaves_destination_empty` (planned, L4); `benches/graph_copy.rs` (planned, L6)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R004` — `crates/medulla-persistence/src/object_store_backend/tests.rs::two_nodes_read_identical_state` (planned, L3); `tests/test_object_storage_shared_root.py::test_multi_node_cold_open_and_rejoin` (planned, L4)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R005` — `tests/test_comparative_cost_gate.py::test_regression_guard_and_claim_eligibility` (planned, L4)
- `# spec: MEDULLA-OBJECT-STORAGE-BACKEND-R006` — `crates/medulla-storage/tests/object_archive.rs::overlap_and_digest_refuse` (planned, L2)
