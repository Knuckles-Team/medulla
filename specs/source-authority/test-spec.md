# MEDULLA-SRC-AUTH — Test specification

Governing spec: [spec.md](spec.md). No live credential is part of the PR gate; all fixtures are disposable and recorded.

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Contract | Load the core virtual-graph source | All authority terms present | PENDING |
| T-002 | FR-002 | Shape | One fixture per fault: no source, no class, no rank, wrong contract kind, mapping of another class or source, facet not projected, accelerated without reason, equal-rank tie | Valid accepted; each fault refused with its message | PENDING |
| T-003 | FR-003 | Negative and integration | Pack source with `vg:approved true`. Operator source. Approval lease with changed digest. Proposer equal to approver | Pack refused. Operator attaches. Mismatch and same-identity approvals refused | PENDING |
| T-004 | FR-004 | Unit and integration | Two-source fixture. Superclass and subclass rows. Facet rows. Live read of attached sources | Winner, alternates and premises as specified. Live equals fixture | PENDING |
| T-005 | FR-005 | Unit | Each ordering pair; tie across two schema sources | Documented order; `Ambiguous` listing both rows, name swap changes nothing | PENDING |
| T-006 | FR-006 | Unit | Unconnected, unapproved, stale, mismatched, ambiguous. With and without `allowFallback`. No authority row | Typed verdict naming source and concept, or declared fallback recorded | PENDING |
| T-007 | FR-007 | Unit and report | Verdicts to audit; audit to report `sources_used` | Audit equals premises; none added | PENDING |
| T-008 | FR-008 | Unit | Virtual row; accelerated row with reason | Live mode; accelerated mode with reason in the audit | PENDING |
| T-009 | SC-001 | End to end | Recorded responses; swap two ranks; same query text | Different `sources_used`, equal query text, non-empty provenance | PENDING |
| T-010 | FR-009 | Unit and served | Live-truth and historical sources in conflict; inspiration-only source with a current-state question | Live source wins; `RoleInsufficient` abstention | PENDING |
| T-011 | FR-010 | Unit and served | Tool-result node; lowering write without and with approval | Stored `untrusted`; refusal; approved lowering | PENDING |
| T-012 | FR-011 | Unit and served | Three records, each changed on one or both sides | Applied, proposed and conflict counts of one | PENDING |

Negative cases that must exist: an origin-approved rank-1 declaration with a connected, approved rank-2 alternate must abstain when rank 1 is unconnected or its mapping is unapproved and `allowFallback` is false; the same fixtures may select rank 2 only when rank 1 explicitly enables fallback, with `FallbackFrom` recorded. Also test a caller who cannot see the registered source; a connector self-claim; an equal-rank tie; a copy served for a virtual row.

### MEDULLA-SRC-AUTH-R010 tests

- `MEDULLA-SRC-AUTH-R010.1`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::source_role_round_trips`.
- `MEDULLA-SRC-AUTH-R010.2`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::live_truth_beats_historical_on_conflict`.
- `MEDULLA-SRC-AUTH-R010.3`: `crates/medulla-virtual-graph/src/source_selection/live.rs::tests::roles_read_from_schema_sources` (L2); `crates/medulla-graph-core/src/graph/schema_sources.rs::tests::an_unknown_source_role_is_refused_and_the_closed_set_attaches` (L1, `SOURCE_ROLE_UNKNOWN`); `tests/federated/test_source_role_served.py::test_live_truth_wins_conflict` (L3).
- `MEDULLA-SRC-AUTH-R010.4`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::inspiration_only_abstains_role_insufficient`.
- `MEDULLA-SRC-AUTH-R010.5`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::role_premise_names_both_sources`.
- `MEDULLA-SRC-AUTH-R010.6`: `tests/federated/test_source_role_served.py::test_role_conflict_audit_names_both_sources_and_roles`, `tests/federated/test_source_role_served.py::test_inspiration_only_abstains_role_insufficient` and `tests/federated/test_source_role_served.py::test_other_principal_sees_no_role_verdict` (planned, L3): the file exists; the owning change adds these three cases to it.

### MEDULLA-SRC-AUTH-R011 tests

- `MEDULLA-SRC-AUTH-R011.1`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::trust_mark_round_trips`.
- `MEDULLA-SRC-AUTH-R011.2`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::derived_content_inherits_untrusted`.
- `MEDULLA-SRC-AUTH-R011.3`: `crates/medulla-h-graph/src/graph_ops/gateway_graph.rs::tests::add_node_stamps_trust_mark` (planned, L2) and `crates/medulla-h-graph/src/graph_ops/gateway_graph.rs::tests::cas_lowering_needs_approval_and_pins_current_mark` (planned, L2) and `crates/medulla-h-graph/src/graph_ops/gateway_graph.rs::tests::batch_writes_refuse_caller_set_trust_mark` (L2): the tests do not exist yet; the owning change adds them at this exact path. The approval id is the reserved updates-blob key `trust_mark_approval_id`.
- `MEDULLA-SRC-AUTH-R011.4`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::downgrade_without_approval_refused`.
- `MEDULLA-SRC-AUTH-R011.5`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::mark_carries_rule_and_generation`.
- `MEDULLA-SRC-AUTH-R011.6`: `tests/federated/test_trust_mark_served.py::test_tool_result_is_untrusted_and_sticky` (planned, L3): the file does not exist yet; the owning change adds it at this exact path.

### Planned served and integration tests of the other rows

Each entry below is planned. The test file or function does not exist in the tree yet; the row that binds it adds it at the stated path.

- **MEDULLA-SRC-AUTH-R003.6:** `tests/source_authority/test_governed_ledger_harness.py::test_harness_attaches_a_contract_and_reads_authority_rows` (planned). Proves the row at its entry point; no receipt is asserted.
- `MEDULLA-SRC-AUTH-R003.3`: `crates/medulla-h-admin/src/graph_schema/approved/tests.rs::authority_candidate_needs_matching_action_and_digest` (planned, L2) and `tests/federated/test_source_authority_approval_served.py::test_authority_attach_needs_distinct_approver` (planned, L3); single use and replay: `approved/tests.rs::an_approved_authority_attach_is_single_use` and `approved/tests.rs::a_retried_authority_attach_replays_its_receipt`, `approved/tests.rs::a_retry_after_the_approval_window_closed_still_replays` and tenant scope `approved/tests.rs::another_tenants_approved_change_answers_like_an_unknown_change`, `approved/tests.rs::governed_ledger_reads_are_scoped_to_the_proposing_tenant` and `approved/tests.rs::a_same_key_retry_with_a_different_body_is_refused` (planned, L2).
- `MEDULLA-SRC-AUTH-R004.6`: `crates/medulla-server-served-tests/src/tests/foreign_tenancy.rs::connected_sources_exclude_another_owners_source` (planned, L2) and `tests/federated/test_source_coverage_served.py::test_other_principals_source_is_unconnected` (planned, L3).
- `MEDULLA-SRC-AUTH-R004.6.1`: `tests/federated/test_source_coverage_served.py::test_rank_one_wins_and_alternates_listed` (planned, L3).
- `MEDULLA-SRC-AUTH-R007.4`: `tests/federated/test_source_coverage_served.py::test_report_sources_used_equal_audit` (planned, L3) and `crates/medulla-virtual-graph/src/supply_chain_impact.rs::tests::sources_used_equals_audit` (planned, L2).
- `MEDULLA-SRC-AUTH-R009.4`: `tests/federated/test_source_coverage_served.py::test_swapping_authority_rank_changes_sources_used` (planned, L3).
- `MEDULLA-SRC-AUTH-R801`: `crates/medulla-h-graph/src/graph_ops/policy_source.rs::tests::bind_refuses_in_order` (planned, L1) and `tests/policy_source_served.rs::stale_review_leaves_policy_unchanged` (planned, L3).

### MEDULLA-SRC-AUTH-R012 tests

- `MEDULLA-SRC-AUTH-R012`: `crates/medulla-h-admin/src/admin/connector_pack/tests/source_sync.rs::two_way_sync_applies_proposes_and_records_conflicts` (L3).
- `MEDULLA-SRC-AUTH-R012.1`: `crates/medulla-rdf/tests/source_sync.rs::sync_terms_resolve` (planned, L2): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.2`: `crates/medulla-rdf/tests/source_sync.rs::sync_shapes_refuse_planted_faults` (planned, L2): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.3`: `crates/medulla-virtual-graph/src/source_sync/tests.rs::reconcile_covers_every_case` (planned, L1): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.4`: `crates/medulla-virtual-graph/src/source_sync/tests.rs::state_rides_the_inbound_batch` (planned, L1): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.5`: `crates/medulla-h-admin/src/admin/connector_pack/tests/source_sync.rs::outbound_change_queues_one_proposal` (planned, L3): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.6`: `crates/medulla-h-admin/src/admin/connector_pack/tests/source_sync.rs::sync_applies_inbound_and_writes_state_once` (planned, L3): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.7`: `crates/medulla-h-admin/src/admin/connector_pack/tests/source_sync.rs::sync_refusals_leave_state_unchanged` (planned, L3): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.8`: `crates/medulla-h-admin/src/admin/connector_pack/tests/source_sync.rs::report_lists_conflicts_with_both_digests` (planned, L3): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R012.9`: `crates/medulla-h-admin/src/admin/connector_pack/tests/source_sync.rs::two_way_sync_applies_proposes_and_records_conflicts` (L3): one sync after a source-only, a graph-only and a two-sided change gives one applied, two proposed and one conflict; the graph value stands.
- `MEDULLA-SRC-AUTH-R012.10`: `benches/source_sync.rs::bench_source_sync` group `source_sync` (planned, nightly bench, never gates a PR).

### MEDULLA-SRC-AUTH-R013 tests

- `MEDULLA-SRC-AUTH-R013`: `crates/medulla-rdf/tests/source_authority_fixture.rs::data_location_yields_a_proposed_pack_row` (planned, L2): the test does not exist yet; the owning change adds it at this exact path.

## Explicit state machine rows (Article XXXI)

- `MEDULLA-SRC-AUTH-R802`: retired (no remaining lifecycle; see `MEDULLA-SRC-AUTH-R802.1`).
- `MEDULLA-SRC-AUTH-R802.1`: retired (`MappingApprovalState` is fixed at construction; no machine).
