# MEDULLA-SCM test-spec

## Positive

- `tests/test_scm_blob.py::test_blob_stored_once_and_read_by_digest` (planned). Identical bytes store once. The
  digest readback equals the original bytes. *(MEDULLA-SCM-R001)*
- `tests/test_scm_changeset.py::test_changeset_stores_only_changed_paths`. A two-path changeset stores
  exactly two path entries. The chain readback returns the record unchanged. *(MEDULLA-SCM-R002)*
- `tests/test_scm_accept.py::test_disjoint_writers_land_and_overlap_rebases_or_conflicts` (planned). Two disjoint
  writers both land. Two overlapping writers land one and conflict the other with
  `SOURCE_CONFLICT{paths}`. *(MEDULLA-SCM-R003)*
- `tests/test_scm_tree.py::test_tree_at_revision_matches_full_replay` (planned). A subtree read at each revision
  equals the full replay. The checkpoint path returns the same entries. *(MEDULLA-SCM-R004)*
- `tests/test_scm_history.py::test_incremental_indexes_equal_rebuild`. Incremental index rows equal a
  from-scratch rebuild byte-for-byte. A stale index head refuses `HISTORY_INDEX_STALE`. *(MEDULLA-SCM-R005)*
- `tests/test_scm_history.py::test_path_history_lists_touching_changesets`. Path history lists touching
  changesets newest first up to `rev`. A `limit` over 1000 refuses `CHANGESET_INVALID`. *(MEDULLA-SCM-R005.1)*
- `tests/test_scm_history.py::test_blame_attributes_lines_across_rename`. Blame keeps line origins across a
  modify and a rename. A binary file refuses `BLAME_BINARY`. *(MEDULLA-SCM-R005.2)*
- `tests/test_scm_code_graph.py::test_one_file_change_updates_only_its_symbols`. A one-file changeset
  updates only that file symbols. A structural no-op updates nothing. *(MEDULLA-SCM-R006)*
- `tests/test_scm_path_access.py::test_prefix_grant_filters_reads_and_refuses_writes`. A prefix grant
  filters reads to the prefix. A longer grant with both bits false is a stored explicit deny and wins, so
  `src/secret` reads refuse `PATH_READ_DENIED` and writes refuse `PATH_WRITE_DENIED` under an allowed `src`.
  A path with no grant is denied. *(MEDULLA-SCM-R007)*
- `tests/test_scm_gate_verdict.py::test_verdict_records_are_sealed_and_read_back` (planned). Pass and fail
  verdicts read back byte-identically. An overwrite attempt is refused. *(MEDULLA-SCM-R008)*
- `tests/test_scm_bisect.py::test_bisect_finds_culprit_and_replays_decision`. A seeded bisect names the
  seeded culprit. The recorded decision replays byte-identically. *(MEDULLA-SCM-R009)*
- `crates/medulla-h-ingest/src/source_changeset/git.rs::tests::export_import_preserves_commit_and_tree_ids` (planned).
  Import of a generated linear history then `export_pack` preserves every commit id and tree id for both object
  formats, including one merge commit of two parents. `crates/medulla-compute-assemble/src/source_git_objects.rs::tests::pack_round_trips_and_ids_match_git_vectors`
  (planned) pins the object ids and the delta reads. `crates/medulla-h-ingest/src/source_changeset/git.rs::tests::history_and_blame_follow_first_parent_through_a_merge` (planned) reads history and blame through a merge on the first-parent line. *(MEDULLA-SCM-R010)*
- `tests/simulation/source_changesets.rs::simulate_concurrent_accept_crash_and_replay`. Concurrent accepts with
  crash and replay hold every chain invariant. *(MEDULLA-SCM-R011)*
- `tests/test_scm_provenance.py::test_changeset_links_to_run_trace` (planned). A delegated agent changeset reads
  back with `agent_identity` and `run_trace_id`. A missing id refuses `AGENT_RUN_TRACE_MISSING`. An
  invisible or wrong-kind node refuses `RUN_TRACE_UNKNOWN`. *(MEDULLA-SCM-R012)*

## Negative

- A write that overwrites an existing blob digest is refused. The store keeps the original bytes.
- A changeset with an empty path list is refused. A parent not on the chain is refused. The chain
  stays unchanged.
- A changeset whose base left the branch chain is refused as stale.
- An unclean overlap refuses with `SOURCE_CONFLICT{paths}`. The conflict lists every conflicting
  path. No partial change lands.
- A tree read at a revision not on the repository chain is refused. A cross-tenant tree read is
  refused.
- An index read whose head row is absent or of another version refuses `HISTORY_INDEX_STALE`.
- A read under a prefix with no grant refuses `PATH_READ_DENIED`. A write without a write grant
  refuses `PATH_WRITE_DENIED`. A bad `SetPathGrant` refuses `PATH_GRANT_INVALID`.
- A verdict whose input digest mismatches the changeset tree is refused. A sealed verdict cannot
  be overwritten.
- A bisect without verdict coverage is refused. A revert that later changesets depend on is
  refused.
- An export with a failing mid-stream tree read is refused. A malformed import graph is refused.
- A fuzz seed that fails to seed is refused. It records no state.
- An agent changeset with no run trace id refuses `AGENT_RUN_TRACE_MISSING`. An unknown run node
  refuses `RUN_TRACE_UNKNOWN`.

## Integration

- Full agent loop: submit a changeset, read the tree at the new head, export the changed files,
  read the history and blame for the changed paths.
- Two concurrent writers on disjoint paths both land. The same two writers on one path land one
  and conflict the other.
- A crashed accept mid-write replays to the same head. The chain stays consistent.
- Export a chain, import it into a fresh repository, and compare the commit and tree ids.
- A path-scoped principal reads its prefix, is refused outside it, and is refused on a write.

## Quality and release proof

- Clippy, test, jscpd, CCCC, dupehound, KISS gates pass on the changeset and tree code.
- The no-duplicate-authority check confirms the changeset path reuses the existing commit and
  blob authorities. It adds no parallel authority.
- The simulation harness runs in the spec gate. It gates a regression in chain invariants.
- Release proof follows MEDULLA-CONTRACT. Exact-head CI receipts record in this directory.

## Proving tests per row

Each entry marked planned names a test or file that does not exist in the tree yet; the owning change adds it at this exact path. The two entries marked existing run today.

| Row | Proving test | Level | State |
|---|---|---|---|
| `MEDULLA-SCM-R001` | `tests/test_scm_blob.py::test_blob_stored_once_and_read_by_digest` | L3 | planned |
| `MEDULLA-SCM-R002` | `tests/test_scm_changeset.py::test_changeset_stores_only_changed_paths` | L3 | existing |
| `MEDULLA-SCM-R002` | `crates/medulla-h-ingest/src/source_changeset.rs::tests::accept_stores_only_changed_paths_and_fills_old_blobs_from_the_parent` | L2 | existing |
| `MEDULLA-SCM-R002` | `crates/medulla-h-ingest/src/source_changeset.rs::tests::accept_refuses_a_base_off_the_branch_and_an_overlapping_stale_base` | L2 | existing |
| `MEDULLA-SCM-R002` | `crates/medulla-h-ingest/src/source_changeset.rs::tests::verify_submission_refuses_empty_invalid_and_missing_blobs` | L2 | existing |
| `MEDULLA-SCM-R002.1` | `crates/medulla-source-types/src/source_changeset.rs::tests::trailers_parse_and_render_round_trip` | L2 | existing |
| `MEDULLA-SCM-R002.1` | `tests/test_scm_changeset.py::test_trailers_round_trip_through_git` | L3 | planned |
| `MEDULLA-SCM-R003` | `tests/test_scm_accept.py::test_disjoint_writers_land_and_overlap_rebases_or_conflicts` | L3 | planned |
| `MEDULLA-SCM-R003.1` | `tests/test_scm_disjoint_served.py::test_disjoint_changesets_land_in_parallel` | L4 | planned |
| `MEDULLA-SCM-R004` | `tests/test_scm_tree.py::test_tree_at_revision_matches_full_replay` | L3 | existing |
| `MEDULLA-SCM-R004` | `crates/medulla-compute-assemble/src/source_tree.rs::tests::tree_at_revision_matches_full_replay` | L2 | existing |
| `MEDULLA-SCM-R005` | `crates/medulla-compute-assemble/src/source_history.rs::tests::incremental_rows_equal_rebuild` | L2 | planned |
| `MEDULLA-SCM-R005` | `tests/test_scm_history.py::test_incremental_indexes_equal_rebuild` | L3 | planned |
| `MEDULLA-SCM-R005.1` | `crates/medulla-compute-assemble/src/source_history.rs::tests::path_history_lists_newest_first_up_to_rev` | L2 | planned |
| `MEDULLA-SCM-R005.1` | `tests/test_scm_history.py::test_path_history_lists_touching_changesets` | L3 | planned |
| `MEDULLA-SCM-R005.2` | `crates/medulla-compute-assemble/src/source_history.rs::tests::blame_keeps_origins_across_modify_and_rename` | L2 | planned |
| `MEDULLA-SCM-R005.2` | `tests/test_scm_history.py::test_blame_attributes_lines_across_rename` | L3 | planned |
| `MEDULLA-SCM-R006` | `tests/test_scm_code_graph.py::test_one_file_change_updates_only_its_symbols` | L3 | planned |
| `MEDULLA-SCM-R007` | `crates/medulla-source-types/src/source_path_grant.rs::tests::longest_segment_prefix_wins_and_no_match_denies` | L2 | planned |
| `MEDULLA-SCM-R007` | `tests/test_scm_path_access.py::test_prefix_grant_filters_reads_and_refuses_writes` | L3 | planned |
| `MEDULLA-SCM-R007` | `crates/medulla-persistence/src/branch_registry_store.rs::tests::both_false_grant_is_stored_as_deny` | L2 | planned |
| `MEDULLA-SCM-R008` | `tests/test_scm_gate_verdict.py::test_verdict_records_are_sealed_and_read_back` | L3 | planned |
| `MEDULLA-SCM-R009` | `tests/test_scm_bisect.py::test_bisect_finds_culprit_and_replays_decision` | L3 | planned |
| `MEDULLA-SCM-R010` | `crates/medulla-h-ingest/src/source_changeset/git.rs::tests::export_import_preserves_commit_and_tree_ids`; `crates/medulla-compute-assemble/src/source_git_objects.rs::tests::pack_round_trips_and_ids_match_git_vectors` | L3 | planned |
| `MEDULLA-SCM-R011` | `tests/simulation/source_changesets.rs::simulate_concurrent_accept_crash_and_replay` | L3 | planned |
| `MEDULLA-SCM-R012` | `crates/medulla-h-ingest/src/source_changeset/provenance.rs::tests::agent_needs_a_visible_run_trace` | L2 | planned |
| `MEDULLA-SCM-R012` | `tests/test_scm_provenance.py::test_changeset_links_to_run_trace` | L3 | planned |
| `MEDULLA-SCM-R013.3` | `crates/medulla-enrich/tests/symbol_edit_verify.rs::reparse_confirms_edit_and_untouched_symbols` | L2 | planned |
| `MEDULLA-SCM-R013.4` | `tests/test_scm_symbol_edit_served.py::test_symbol_edit_lands_and_reindexes` | L4 | planned |
| `MEDULLA-SCM-R013.5` | `tests/test_scm_symbol_edit_served.py::test_symbol_edit_refusals` | L4 | planned |
| `MEDULLA-SCM-R013.6` | `tests/test_scm_symbol_edit_served.py::test_symbol_edit_evidence_reads_back` | L4 | planned |
| `MEDULLA-SCM-R013.7` | `benches/source_symbol_edit.rs::edit_one_symbol_in_5000_lines` | L6 | bound |
| `MEDULLA-SCM-R013.1` | `crates/medulla-types/tests/source_symbol_edit.rs::edit_round_trips_and_kinds_are_closed` | L2 | existing |
| `MEDULLA-SCM-R013.2` | `crates/medulla-compute/tests/source_symbol_edit.rs::bytes_outside_spans_are_unchanged` | L2 | existing |
| `MEDULLA-SCM-R013.4` | `tests/scm_symbol_edit_served.rs::accept_path_runs_after_projection` | L3 | planned |
| `MEDULLA-SCM-R014` | `tests/test_scm_git_export_pack.py::test_clone_matches_tree_hash_and_denied_path_refuses`; `crates/medulla-server-router/src/dispatch/router/git_export_pack.rs::tests::receipt_carries_a_blob_ref_not_hex`; `crates/medulla-source-types/src/source_changeset.rs::tests::gitlink_mode_valid_only_on_import` | L3 | planned |
| `MEDULLA-SCM-R851` | `tests/test_scm_git_refs_list.py::test_refs_list_names_each_head_and_a_denied_path_refuses`; `crates/medulla-persistence/src/branch_registry_store.rs::tests::live_branches_of_one_graph_exclude_deleted_and_foreign_rows` | L3 | planned |
