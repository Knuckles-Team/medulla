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

Negative cases that must exist: an origin-approved rank-1 declaration with a connected, approved rank-2 alternate must abstain when rank 1 is unconnected or its mapping is unapproved and `allowFallback` is false; the same fixtures may select rank 2 only when rank 1 explicitly enables fallback, with `FallbackFrom` recorded. Also test a caller who cannot see the registered source; a connector self-claim; an equal-rank tie; a copy served for a virtual row.

### MEDULLA-SRC-AUTH-R010 tests

- `MEDULLA-SRC-AUTH-R010.1`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::source_role_round_trips`.
- `MEDULLA-SRC-AUTH-R010.2`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::live_truth_beats_historical_on_conflict`.
- `MEDULLA-SRC-AUTH-R010.3`: `crates/medulla-virtual-graph/src/source_selection/live.rs::tests::roles_read_from_schema_sources` (planned, L2): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R010.4`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::inspiration_only_abstains_role_insufficient`.
- `MEDULLA-SRC-AUTH-R010.5`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::role_premise_names_both_sources`.
- `MEDULLA-SRC-AUTH-R010.6`: `tests/federated/test_source_role_served.py::test_live_truth_wins_conflict` (planned, L3): the file does not exist yet; the owning change adds it at this exact path.

### MEDULLA-SRC-AUTH-R011 tests

- `MEDULLA-SRC-AUTH-R011.1`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::trust_mark_round_trips`.
- `MEDULLA-SRC-AUTH-R011.2`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::derived_content_inherits_untrusted`.
- `MEDULLA-SRC-AUTH-R011.3`: `crates/medulla-h-graph/src/graph_ops/gateway_graph.rs::tests::add_node_stamps_trust_mark` (planned, L2): the test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-SRC-AUTH-R011.4`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::downgrade_without_approval_refused`.
- `MEDULLA-SRC-AUTH-R011.5`: `crates/medulla-virtual-graph/src/source_selection/tests.rs::mark_carries_rule_and_generation`.
- `MEDULLA-SRC-AUTH-R011.6`: `tests/federated/test_trust_mark_served.py::test_tool_result_is_untrusted_and_sticky` (planned, L3): the file does not exist yet; the owning change adds it at this exact path.

### Planned served and integration tests of the other rows

Each entry below is planned. The test file or function does not exist in the tree yet; the row that binds it adds it at the stated path.

- `MEDULLA-SRC-AUTH-R003.3`: `crates/medulla-h-admin/src/graph_schema/approved/tests.rs::authority_candidate_needs_matching_action_and_digest` (planned, L2) and `tests/federated/test_source_authority_approval_served.py::test_authority_attach_needs_distinct_approver` (planned, L3).
- `MEDULLA-SRC-AUTH-R004.6`: `crates/medulla-server-served-tests/src/tests/foreign_tenancy.rs::connected_sources_exclude_another_owners_source` (planned, L2) and `tests/federated/test_source_coverage_served.py::test_other_principals_source_is_unconnected` (planned, L3).
- `MEDULLA-SRC-AUTH-R004.6.1`: `tests/federated/test_source_coverage_served.py::test_rank_one_wins_and_alternates_listed` (planned, L3).
- `MEDULLA-SRC-AUTH-R007.4`: `tests/federated/test_source_coverage_served.py::test_report_sources_used_equal_audit` (planned, L3) and `crates/medulla-virtual-graph/src/supply_chain_impact.rs::tests::sources_used_equals_audit` (planned, L2).
- `MEDULLA-SRC-AUTH-R009.4`: `tests/federated/test_source_coverage_served.py::test_swapping_authority_rank_changes_sources_used` (planned, L3).
- `MEDULLA-SRC-AUTH-R801`: `tests/policy_source_served.rs::stale_review_leaves_policy_unchanged` (planned, L3).
