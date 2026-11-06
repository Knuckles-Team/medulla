# MEDULLA-IDENTITY — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Integration | Network-free signed JWTs for issuers A and B with separate audiences and kinds; rotate the key of A | Both valid issuers bind exact principal and tenant; unknown, ambiguous or disallowed-kind issuers fail before dispatch; the overlap key works only in its window | PENDING |
| T-002 | FR-002 | Integration | Register and replace complete roles, remove an admin role, force a failure between the role write and the audit append, tamper with one audit record | Grants reflect removal before success; one transaction rolls back both writes; verification detects the altered record; state persists after restart | PENDING |
| T-003 | FR-003 | End to end | Every allowed mode transition with two admins, local credentials and an enabled provider; stale epoch; no surviving admin | Mode and epoch advance once; old sessions refuse; refusals leave mode and epoch unchanged | PENDING |
| T-004 | FR-004 | End to end | Anonymous mode over loopback and a private socket; remote listener; store reuse; oversize metadata | Fixed anonymous authority; each refusal is `ACCESS_DENIED`; the decoder stays within its budget | PENDING |
| T-005 | FR-005 | Contract | Generate the contract and install the wheel in a clean environment; a human grant of a service-only scope; an unknown name | Every scope has one class and the digest matches the server; generation or runtime refuses each fault | PENDING |
| T-006 | FR-006 | Unit | One verified principal across every queue class; tenant role reads `__commons__` | One principal attributed throughout; read allowed, write refused, other tenants unreadable | PENDING |
| T-007 | FR-007 | End to end | A stored policy node, a wider plan scope in the request, a candidate beyond the bound | The stored bound wins; `SCOPE_EXCEEDS_POLICY` with one audit entry; another tenant sees the empty bound | PENDING |
| T-008 | FR-008 | End to end | Private and org-shared units as principals A, B and C | Only the org-shared current version reaches B; C sees neither; hidden and absent point reads return one code | PENDING |
| T-009 | FR-009 | End to end | All three metadata variants through every write route, restart, owner and other principal reads | Admission runs on each route; refusals leave no durable effect; restart keeps the revision chain | PENDING |
| T-010 | FR-010 | End to end | Share to `org`, revoke, SIGTERM and restart | A second principal does not see the node after restart; refusals change nothing; one audit event per change | PENDING |
| T-011 | FR-011 | End to end | Three-level tenant chain, unscopable query, group-restricted record | Visibility order and marking filter hold; the unscopable query is refused; one audit line per read | PENDING |
| T-012 | FR-012 | End to end | Plan and apply ownership then roll back; delegation record; policy bundle; resource access; client session | Roles restore exactly; refusals change nothing; each served method returns its typed result | PENDING |
| T-013 | FR-013 | End to end | Resolve, liveness, renewal, approval-key lifecycle and credential observation | Replies match the closed sets; no raw token on the wire; stale revision refuses without mutation | PENDING |

## Negative and boundary cases

Unknown, ambiguous and disallowed-kind issuers. Stale epoch and no surviving admin. A mode transition into or out of `none`. A missing policy node. A hidden unit versus an absent unit. A full audit queue. A plan that removes the last admin. A reference outside the caller's deployment tenant. A delegated broker that tries to attest a key.

## Open acceptance gap: replicated tenant provisioning

**OPEN — clustered tenant provisioning and identity-store ordering.** The
inherited replicated `CreateGraph` path receives a principal fingerprint rather
than the original principal ID (`crates/medulla-server-api/src/authority_context.rs`,
`VerifiedRequestContext::replicated_mutation`). The request boundary copies that
verified identity into the request, and lifecycle provisioning looks it up as an
identity ID. Local tenant provisioning tests do not establish correct clustered
provisioning; this work does not claim that inherited mismatch is fixed.

A narrow fail-closed guard withholds managed auto-binding during replicated
creation when the fingerprint matches a managed ID. It skips the entire
provisioning call, including the unmanaged fallback; graph creation still
succeeds. Raw fingerprint-shaped self IDs are also withheld because the carrier
cannot distinguish them from another caller's fingerprint. Separate authorized
Identity grants are required. The serialization/application regression source
covers caller `U` plus managed alias `H(U)`, raw self-ID `H`, local creation, and
policy persistence/reprojection using temporary Redb. These regressions are
unexecuted pending exact-source native qualification; they do not establish
cross-group ordering or close this acceptance gap.

Managed tenant provisioning also mutates the identity store.
It must preserve route ownership and ordering against Identity mutations. `CreateGraph` follows the named graph route, while Identity mutations
use `__commons__` (`crates/medulla-server-consensus/src/dispatch/consensus/routing.rs`). The local store
transaction and focused Identity routing regression do not establish ordering
between these routes.

Before clearing this acceptance gap, require a focused replicated
CreateGraph/Identity test or equivalent exact-source contract evidence proving
correct creator binding, durable grants, and ordering with identity revocation
across replica application and replay. Keep this gap OPEN until that evidence
exists; passing local mapping, persistence, or routing tests alone is insufficient.
This note does not authorize principal reverse lookup or a cross-group authority
design change, and does not promote the related acceptance criteria to passed.


## Reproducible gates

Use `cargo run -q --locked -p medulla-capabilities --features contract --bin gen_contract -- --check` and focused Rust tests for OIDC, isolation identity admin, RBAC persistence and audit. Run `pre-commit run --config .config/pre-commit.yaml --all-files` on the exact PR head. Hosted CI generates JWTs and attestations locally, uses disposable redb files and needs no live identity provider or network.

## Bound tests

Each leaf row binds the tests below. A test marked `(planned)` does not exist yet; the row names its exact path.

| Case | Requirement | Proving test |
|---|---|---|
| Multi-issuer OIDC trust list with per-issuer kind limits | `MEDULLA-IDENTITY-R001` | `crates/medulla-server-api/src/oidc.rs::tests::unknown_ambiguous_or_disallowed_kind_issuer_fails_before_dispatch` (planned); `crates/medulla-server-auth/src/auth.rs::tests::two_signers_valid_simultaneously_during_an_overlap_window_then_old_is_retired`; `crates/medulla-server-api/src/oidc.rs::tests::rotated_jwks_key_is_accepted_only_in_its_overlap_window` (planned) |
| Identity and RBAC writes recompute roles and audit atomically | `MEDULLA-IDENTITY-R002` | `crates/medulla-isolation/src/isolation/identity_store_tests.rs::a_failed_write_rolls_back_the_store_and_the_projection_together`; `crates/medulla-isolation/src/isolation/identity_store_tests/tenant_and_audit.rs::failed_no_op_grant_audit_save_rolls_back_the_audit_entry`; `crates/medulla-kernel/src/audit.rs::tests::audit_path_detects_a_tampered_leaf` |
| Explicit isolated anonymous store authority (native TCP qualification) | `MEDULLA-IDENTITY-R003.1` | `tests/test_native_no_auth_served.py::test_anonymous_mode_native_tcp_cases` (planned) |
| Measured anonymous admission budget | `MEDULLA-IDENTITY-R003.2` | `tests/test_native_no_auth_served.py::test_oversize_authority_metadata_refuses` (planned) |
| Isolated-mode runtime qualification over a private socket | `MEDULLA-IDENTITY-R003.3` | `tests/test_native_no_auth_uds_served.py::test_anonymous_mode_over_private_socket` (planned); `tests/test_native_no_auth_uds_served.py::test_anonymous_mode_with_ast_enrichment` (planned) |
| Model: identity mode and epoch CAS on the existing types | `MEDULLA-IDENTITY-R003.4` | `crates/medulla-identity-types/src/identity/tests/modes.rs::stale_epoch_refuses_and_changes_nothing` (planned); `crates/medulla-identity-types/src/identity/tests/modes.rs::transition_to_or_from_none_is_forbidden` (planned); `tests/identity/test_identity_mode_served.py::test_local_external_round_trip` (planned) |
| Behavior: transition preconditions | `MEDULLA-IDENTITY-R003.5` | `crates/medulla-identity-types/src/identity/tests/modes.rs::unmet_precondition_refuses_typed` (planned) |
| Wiring: session invalidation and issuer rotation after the commit | `MEDULLA-IDENTITY-R003.6` | `crates/medulla-identity-types/src/identity/tests/modes.rs::sessions_older_than_epoch_refuse_after_commit_only` (planned); `crates/medulla-server-auth/src/auth.rs::tests::crash_between_commit_and_rotation_is_finished_at_boot` (planned) |
| Wiring: served transition method and offline conversion | `MEDULLA-IDENTITY-R003.7` | `tests/operator_command_identity_mode_cli.rs::none_store_converts_to_local_keeping_data` (planned); `tests/identity/test_identity_mode_served.py::test_non_admin_cannot_transition` (planned) |
| Hot-path proof: mode round trip over the wire | `MEDULLA-IDENTITY-R003.8` | `tests/identity/test_identity_mode_served.py::test_local_external_round_trip` (planned) |
| Generated scope registry classifies every capability scope | `MEDULLA-IDENTITY-R004` | `crates/medulla-capabilities/src/scopes.rs::tests::registry_digest_matches_every_declared_scope` (planned); `crates/medulla-capabilities/src/scopes.rs::tests::unclassified_or_wildcard_scope_fails_generation` (planned) |
| Semantic workers use one identity across queue classes | `MEDULLA-IDENTITY-R005` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_query_dispatch_pipeline_tests.rs::dispatch_attributes_every_queue_class_to_the_one_verified_principal`; `crates/medulla-h-retrieve/src/semantic_index_runtime.rs::semantic_owner_registry_is_scoped_to_persistence_root` |
| Tenant roles read the shared commons graph | `MEDULLA-IDENTITY-R006` | `crates/medulla-isolation/src/isolation.rs::tests::a_tenant_role_reads_but_never_writes_the_shared_commons`; `crates/medulla-isolation/src/isolation/identity_store_tests/tenant_and_audit.rs::managed_tenant_binding_survives_projection_and_is_in_the_saved_store` |
| Scopes `identity:admin`, `loops:read` and `loops:control` are registered in the engine contract | `MEDULLA-IDENTITY-R007` | `crates/medulla-capabilities/src/scopes.rs::tests::registry_digest_matches_every_declared_scope` (planned); `crates/medulla-capabilities/src/scopes.rs::tests::unclassified_or_wildcard_scope_fails_generation` (planned) |
| Attach point: tool-set admission in AgentAssemble | `MEDULLA-IDENTITY-R008.1` | `crates/medulla-h-decide/src/decide/stat_tests/scope_bound_tests.rs::attach_point_is_reached` (planned); `tests/identity/test_scope_bound_served.py::test_plan_scope_is_bounded_by_policy` (planned) |
| Typed `ScopeBound` | `MEDULLA-IDENTITY-R008.2` | `crates/medulla-isolation/src/isolation/scope_bound.rs::tests::empty_policy_iri_is_refused` |
| Pure subset check | `MEDULLA-IDENTITY-R008.3` | `crates/medulla-isolation/src/isolation/scope_bound.rs::tests::first_excess_tool_is_named` |
| Bound is read from the stored policy node | `MEDULLA-IDENTITY-R008.4` | `crates/medulla-isolation/tests/scope_bound.rs::plan_scope_field_is_ignored` (planned); `crates/medulla-isolation/tests/scope_bound.rs::missing_policy_node_is_empty_bound` (planned) |
| Typed refusal ScopeExceedsPolicy with an audit record | `MEDULLA-IDENTITY-R008.5` | `crates/medulla-isolation/tests/scope_bound.rs::excess_plan_is_refused_and_audited` (planned); audit event `subject_iri` and `policy_iri` bounded at 2048 bytes |
| End-to-end proof over the wire | `MEDULLA-IDENTITY-R008.6` | `tests/identity/test_scope_bound_served.py::test_plan_scope_is_bounded_by_policy` (planned) |
| Wiring: visible-unit filter on the served read path | `MEDULLA-IDENTITY-R023.3` | `tests/identity/test_segment_retrieval_served.py::test_retrieve_filters_by_principal` (planned) |
| Refusal: point read outside scope | `MEDULLA-IDENTITY-R023.4` | `tests/identity/test_segment_retrieval_served.py::test_out_of_scope_read_names_id_only` (planned) |
| Evidence: read audit with hidden count | `MEDULLA-IDENTITY-R023.5` | `tests/identity/test_segment_retrieval_served.py::test_read_audit_counts_hidden` (planned) |
| Hot-path journey: shared unit visible only in scope | `MEDULLA-IDENTITY-R023.6` | `tests/identity/test_segment_retrieval_served.py::test_shared_unit_visible_only_in_scope` (planned) |
| Model: typed principal metadata | `MEDULLA-IDENTITY-R023.7` | `crates/medulla-identity-types/src/identity/metadata.rs::tests::typed_metadata_variants_round_trip` (planned); `tests/identity/test_typed_metadata_served.py::test_typed_metadata_served_journey` (planned) |
| Behavior: persist metadata as versioned graph nodes | `MEDULLA-IDENTITY-R023.8` | `tests/identity/test_typed_metadata_served.py::test_typed_metadata_persists_without_file_authority` (planned) |
| Wiring: metadata admission before mutation compilation | `MEDULLA-IDENTITY-R023.9` | `tests/identity/test_typed_metadata_served.py::test_all_metadata_write_routes_reach_admission` (planned) |
| Refusal: invalid metadata writes | `MEDULLA-IDENTITY-R023.10` | `tests/identity/test_typed_metadata_served.py::test_invalid_metadata_has_no_durable_effect` (planned) |
| Evidence: metadata import provenance | `MEDULLA-IDENTITY-R023.11` | `tests/identity/test_typed_metadata_served.py::test_metadata_import_provenance_survives_restart` (planned) |
| Hot-path journey: typed metadata over the wire | `MEDULLA-IDENTITY-R023.12` | `tests/identity/test_typed_metadata_served.py::test_typed_metadata_served_journey` (planned) |
| Model: share operations | `MEDULLA-IDENTITY-R023.13.1` | `crates/medulla-types/src/acl.rs::tests::share_ops_round_trip` (planned); `tests/identity/test_share_restart_served.py::test_org_share_revocation_persists_across_restart` (planned) |
| Behavior: share transition function | `MEDULLA-IDENTITY-R023.13.2` | `crates/medulla-isolation/src/isolation/share.rs::tests::revoke_clears_grants` (planned) |
| Wiring: RbacAdmin arm calls the transition | `MEDULLA-IDENTITY-R023.13.3` | `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/share_grant.rs::share_grant_commits_through_gateway` (planned) |
| Refusal: share operations | `MEDULLA-IDENTITY-R023.13.4` | `tests/identity/test_share_restart_served.py::test_share_refusals_change_nothing` (planned) |
| Evidence: share audit event | `MEDULLA-IDENTITY-R023.13.5` | `tests/identity/test_share_restart_served.py::test_share_audit_event_readable` (planned) |
| Hot-path journey: org share revoke survives restart | `MEDULLA-IDENTITY-R023.13.6` | `tests/identity/test_share_restart_served.py::test_org_share_revocation_persists_across_restart` (planned) |
| Model | `MEDULLA-IDENTITY-R610.1` | `crates/medulla-identity-types/src/tenancy.rs::tests::types_round_trip_and_reject_invalid_identifiers` |
| Behavior | `MEDULLA-IDENTITY-R610.2` | `crates/medulla-identity-types/src/tenancy.rs::tests::graph_precedence_marking_filter_claims_and_scope` |
| Wiring: tenant predicate in the UQL executor | `MEDULLA-IDENTITY-R610.3` | `tests/identity/test_tenant_scope_served.py::test_scope_predicate_injected_on_served_read` (planned) |
| Refusal: unscopable read query | `MEDULLA-IDENTITY-R610.4` | `tests/identity/test_tenant_scope_served.py::test_unscopable_query_is_refused` (planned) |
| Evidence: read and claim audit sink | `MEDULLA-IDENTITY-R610.5` | `tests/identity/test_tenant_scope_served.py::test_read_and_claim_are_audited` (planned) |
| Hot-path journey: scope injection and marking filter | `MEDULLA-IDENTITY-R610.6` | `tests/identity/test_tenant_scope_served.py::test_chain_visibility_and_unscopable_refusal` (planned) |
| Model: ownership signals and grant plan | `MEDULLA-IDENTITY-R700.1` | `crates/medulla-identity-types/src/identity/ownership.rs::tests::ownership_types_round_trip` (planned) |
| Behavior: deterministic plan, transactional apply, exact rollback | `MEDULLA-IDENTITY-R700.2` | `crates/medulla-isolation/src/isolation/ownership.rs::tests::plan_is_deterministic_and_rollback_exact` (planned) |
| Wiring: plan method and RbacAdmin apply | `MEDULLA-IDENTITY-R700.3` | `tests/identity/test_graph_ownership_served.py::test_plan_and_apply_reach_wire_methods` (planned) |
| Refusal: unsafe ownership plans | `MEDULLA-IDENTITY-R700.4` | `tests/identity/test_graph_ownership_served.py::test_unsafe_plans_are_refused` (planned) |
| Evidence: stored plan and rollback record | `MEDULLA-IDENTITY-R700.5` | `tests/identity/test_graph_ownership_served.py::test_stored_record_reproduces_plan` (planned) |
| Hot-path journey: apply then rollback | `MEDULLA-IDENTITY-R700.6` | `tests/identity/test_graph_ownership_served.py::test_apply_then_rollback_restores_prior_roles` (planned) |
| Delegation record read for background work | `MEDULLA-IDENTITY-R701` | `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/delegation_record.rs::write_read_revoke_and_foreign_tenant_read` (planned) |
| Policy rule bundle: compile, bind and enforce in the engine | `MEDULLA-IDENTITY-R702` | `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/policy_bundle.rs::bundle_compiles_enforces_and_refuses_both_codes` (planned); `tests/identity/test_policy_bundle_served.py::test_bundle_binds_and_enforces` (planned) |
| Epistemic policy predicates | `MEDULLA-IDENTITY-R703` | `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/policy_bundle.rs::epistemic_condition_admits_expired_and_missing_refuse` (planned) |
| Resource-scoped access decision method | `MEDULLA-IDENTITY-R704` | `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/resource_access.rs::check_resource_access_default_denies` (planned) |
| ClientSession record vocabulary | `MEDULLA-IDENTITY-R705` | `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/client_session.rs::client_session_node_round_trip_and_principal_mismatch_refused` (planned) |
| Read gate expands source groups and honors source access properties | `MEDULLA-IDENTITY-R706` | `tests/ingest/test_source_permission_sync_served.py::test_group_change_applies_without_rewrite` (planned) |
| Identity domain operations with Argon2id and redacted views | `MEDULLA-IDENTITY-R707` | `crates/medulla-identity-types/src/identity/tests/auth.rs::a_matched_password_opens_a_session_and_a_wrong_one_does_not`; `crates/medulla-identity-types/src/identity/tests/sql_dump.rs::dump_round_trips_and_redacts_secrets` |
| Durable session record projection on Resolve | `MEDULLA-IDENTITY-R708` | `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::projection_survives_persistence_and_reports_post_touch_facts`; `tests/identity/test_session_record_served.py::test_resolve_returns_session_record` (planned) |
| Served session-liveness check by opaque session reference | `MEDULLA-IDENTITY-R709` | `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::liveness_reports_live_revoked_expired_and_not_session_bound` (planned); `tests/identity/test_session_liveness_served.py::test_liveness_unknown_and_cross_tenant_refuse` (planned) |
| Idle session renewal uses global authority compare-and-set | `MEDULLA-IDENTITY-R710` | `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::renewal_refuses_invalid_authority_without_mutation`; `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/resolution_context.rs::reference_renewal_rechecks_boundary_and_global_revision` |
| Registered human approval keys | `MEDULLA-IDENTITY-R711` | `crates/medulla-identity-types/src/identity/tests/approval_keys.rs::lifecycle_persists_public_facts_and_enforces_rotation_and_revocation_cas`; `tests/identity/test_approval_key_served.py::test_enrollment_requires_broker_proof` (planned) |
| Credential resolution observes without refreshing | `MEDULLA-IDENTITY-R712` | `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::credential_observation_preserves_state_and_existing_refusals`; `tests/identity/test_credential_observe_served.py::test_refresh_false_does_not_touch` (planned) |
| Original session reference inspection | `MEDULLA-IDENTITY-R713` | `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::original_reference_inspection_is_owner_bound_and_does_not_touch`; `tests/identity/test_session_reference_served.py::test_inspect_reference_owner_and_tenant` (planned) |
| Governed change adopts the elevation edge table | `MEDULLA-IDENTITY-R720` | `crates/medulla-mutation-types/src/governed_change.rs::tests::every_transition_and_refusal_is_covered` (planned) |
| User status is one explicit state machine (rollup) | `MEDULLA-IDENTITY-R721` | `crates/medulla-identity-types/src/identity/tests/user_status_machine.rs::every_transition_and_refusal_is_covered` (planned) |
| Every user status write goes through transition_status | `MEDULLA-IDENTITY-R721.1` | `crates/medulla-identity-types/src/identity/tests/user_status_machine.rs::no_write_bypasses_transition_status`; `crates/medulla-identity-types/src/identity/tests/user_status_machine.rs::every_illegal_pair_is_refused` (planned) |
| Derive active and revoked, and describe the machine | `MEDULLA-IDENTITY-R721.2` | `crates/medulla-identity-types/src/identity/tests/user_status_machine.rs::active_and_revoked_are_derived`; `crates/medulla-rdf/tests/state_machine_vocabulary.rs::user_status_machine_matches_edge_table` (planned) |
| Describe the RBAC elevation machine in the ontology | `MEDULLA-IDENTITY-R722` | `crates/medulla-rdf/tests/state_machine_vocabulary.rs::rbac_elevation_machine_matches_legal_table` (planned) |
