# MEDULLA-CQ-XSRC — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Shape | Authority seed fixtures: valid, no source, no class, no rank, equal-rank tie, unapproved mapping | Valid accepted. Each fault refused. Unapproved mapping reports uncovered | PENDING |
| T-002 | FR-002 | Contract | Load `supply_chain-v1.ttl` and the approved mapping fixtures | Terms present; each mapping passes the virtual-graph shapes | PENDING |
| T-003 | FR-003 | Integration | Three vendor records and three service records across sources | One node after approval, separate before. Join empty before approval | PENDING |
| T-004 | FR-004 | Integration | Class-only SPARQL fixtures | Planned sources equal the authority-derived set; no connector name in the query | PENDING |
| T-005 | FR-005 | Unit | Recorded rows, each abstention kind | Report shape, freshness and abstentions as specified | PENDING |
| T-006 | FR-006 | End to end | Recorded responses. Then live. Then procurement absent. Then authority swap | Non-empty provenance-carrying answer. Typed abstention. Sources differ with equal query text | PENDING |
| T-007 | FR-007 | End to end | Three generalization fixtures | Each non-empty with provenance, and abstention when its source is absent | PENDING |
| T-008 | FR-008 | End to end | One report and agent claims ingested through the served surface | One plan; claims, clusters and decayed weights as fixed; second tenant sees no rows; unknown report refuses | PENDING |
| T-009 | FR-009 | End to end | Two fixture connectors and an approved link; a role that cannot read one source | Joined answer with provenance per fact; the unreadable source is named in coverage | PENDING |

## Negative and boundary cases

Equal-rank authority. Authority on an unapproved mapping. Unconnected rank-1 source with and without `vg:allowFallback`. Unapproved `owl:sameAs`. A report item with no source. A copy with no watermark. A connector name in the query text. An unknown question id. An unknown report id.

## Quality and release proof

Focused `cargo test` for `medulla-rdf`, `medulla-source-types`, `medulla-virtual-graph`, `medulla-types` and `medulla-core` ontology tests; `spec-standard`, `plan-conformance` and `spec-decomposition`. Live acceptance runs through Arbora and records revision, connected sources and output. A source test alone is not served acceptance.

## Bound tests

Each leaf row binds the tests below. A test marked `(planned)` does not exist yet; the row names its exact path.

| Case | Requirement | Proving test |
|---|---|---|
| Add `:Supplier` as a subclass of `:Vendor`, and `:SupplyChainItem` with `:suppliesItem` | `MEDULLA-CQ-XSRC-R002.1` | `crates/medulla-rdf/tests/supply_chain_declarations.rs::supplier_and_item_terms_are_listed`; `crates/medulla-rdf/tests/supply_chain_reasoning.rs::supplier_entails_vendor_under_the_native_reasoner` |
| Add `:SoftwareComponent`, `:ComponentVersion` and `:publishedBy` | `MEDULLA-CQ-XSRC-R002.2` | `crates/medulla-rdf/tests/supply_chain_declarations.rs::software_component_terms_declared`; `crates/medulla-rdf/tests/supply_chain_reasoning.rs::container_image_entails_software_component_under_the_native_reasoner` |
| Add `:Vulnerability` and `:affectsComponent` | `MEDULLA-CQ-XSRC-R002.3` | `crates/medulla-rdf/tests/supply_chain_declarations.rs::vulnerability_and_affects_component_terms_are_listed` |
| Source-authority seed: suppliers, purchase items and third-party risk | `MEDULLA-CQ-XSRC-R002.5` | `crates/medulla-rdf/tests/source_authority_fixture.rs::supplier_seed_passes_shape` (planned); `tests/federated/test_supply_chain_authority_served.py::supplier_selects` (planned) |
| Source-authority seed: software components and vulnerabilities | `MEDULLA-CQ-XSRC-R002.6` | `crates/medulla-rdf/tests/source_authority_fixture.rs::component_seed_passes_shape` (planned); `tests/federated/test_supply_chain_authority_served.py::component_selects` (planned) |
| Virtual mapping fixture: erpnext supplier table to `:Supplier` | `MEDULLA-CQ-XSRC-R002.7` | `crates/medulla-core/tests/supply_chain_vocab.rs::erpnext_supplier_mapping_fixture_passes_the_virtual_shapes`; `tests/federated/test_xsrc_mappings_served.py::test_erpnext_mapping_reads_recorded_rows` |
| Source-authority seed: `:Service` health and identity | `MEDULLA-CQ-XSRC-R003.1` | `crates/medulla-rdf/tests/source_authority_fixture.rs::service_seed_passes_shape` (planned); `tests/federated/test_supply_chain_authority_served.py::service_selects` (planned) |
| Source-authority seed: deployed workloads, images and hosts | `MEDULLA-CQ-XSRC-R003.2` | `crates/medulla-rdf/tests/source_authority_fixture.rs::workload_seed_passes_shape` (planned); `tests/federated/test_supply_chain_authority_served.py::workload_selects` (planned) |
| Source-authority seed: business capabilities | `MEDULLA-CQ-XSRC-R003.3` | `crates/medulla-rdf/tests/source_authority_fixture.rs::capability_seed_passes_shape` (planned); `tests/federated/test_supply_chain_authority_served.py::capability_selects` (planned) |
| Virtual mapping fixture: uptime monitor to `:Service` | `MEDULLA-CQ-XSRC-R003.4` | `crates/medulla-core/tests/supply_chain_vocab.rs::uptime_monitor_resolves_to_service_by_url_host_key`; `tests/federated/test_xsrc_mappings_served.py::test_uptime_mapping_reads_recorded_rows` |
| Virtual mapping fixture: container-manager deployment to `:Workload` and its image | `MEDULLA-CQ-XSRC-R003.5` | `crates/medulla-core/tests/supply_chain_vocab.rs::container_manager_workload_and_image_map_deployed_as_and_uses_image`; `tests/federated/test_xsrc_mappings_served.py::test_container_manager_mapping_reads_recorded_rows` |
| Add `:usesComponent` (service to component version) and `:suppliesService` | `MEDULLA-CQ-XSRC-R004.1` | `crates/medulla-rdf/tests/supply_chain_declarations.rs::supplier_service_carries_the_software_chain_and_inverts_supplies_service`; `crates/medulla-rdf/tests/supply_chain_reasoning.rs::supplies_service_infers_supplier_to_service_from_the_software_path` |
| Resolve a vendor in erpnext, leanix and dockerhub to one `:Supplier` | `MEDULLA-CQ-XSRC-R004.3` | `crates/medulla-types/src/entity_link_proposal.rs::tests::true_supplier_match_is_not_asserted_before_approval` |
| Resolve a service in uptime, container-manager and the CMDB to one `:Service` | `MEDULLA-CQ-XSRC-R004.4` | `crates/medulla-types/src/entity_link_proposal.rs::tests::service_kinds_refuse_unapproved`; `tests/data_plane/test_service_resolution_served.py::test_three_records_resolve_after_approval` |
| Query-time link: a join may only cross an approved `owl:sameAs` | `MEDULLA-CQ-XSRC-R004.5` | `tests/federated/test_supply_chain_join_served.py::test_join_needs_approved_same_as` (planned) |
| `select_sources` orders candidate mappings by `vg:authorityRank` | `MEDULLA-CQ-XSRC-R005.1` | `crates/medulla-virtual-graph/src/source_selection/tests.rs::authority_rank_selects_winner_and_lists_lower_ranked_alternate` |
| Query fixture: the supply-chain impact query over classes only | `MEDULLA-CQ-XSRC-R005.2` | `crates/medulla-rdf/tests/xsrc_supply_chain_query.rs::the_supply_chain_question_names_classes_only_and_parses` |
| The plan is derived, not authored: planned sources equal the declared authorities | `MEDULLA-CQ-XSRC-R005.3` | `crates/medulla-rdf/tests/xsrc_supply_chain_query.rs::the_planned_sources_equal_the_authority_derived_set` |
| An unconnected authoritative source yields a typed abstention, not a silent fallback | `MEDULLA-CQ-XSRC-R005.4` | `crates/medulla-rdf/tests/virtual_graph_authority.rs::allow_fallback_is_declared_on_the_authority_vocabulary`; `crates/medulla-rdf/tests/xsrc_supply_chain_query.rs::an_unconnected_rank_one_source_abstains_by_name` (planned) |
| `EXPLAIN` shows the authority premise for each planned source | `MEDULLA-CQ-XSRC-R005.5` | `tests/federated/test_supply_chain_authority_served.py::test_explain_lists_authority_premises_without_urls` (planned) |
| Add the typed `SupplyChainImpactReport` model | `MEDULLA-CQ-XSRC-R006.1` | `crates/medulla-source-types/src/supply_chain_report.rs::tests::a_valid_report_builds_and_round_trips_the_wire_encoding`; `crates/medulla-source-types/src/supply_chain_report.rs::tests::an_item_without_a_source_is_refused` |
| Report carries freshness per source | `MEDULLA-CQ-XSRC-R006.2` | `crates/medulla-source-types/src/supply_chain_report.rs::tests::freshness_modes` (planned) |
| Report abstentions map the source-authority reasons, with no second list | `MEDULLA-CQ-XSRC-R006.3` | `crates/medulla-virtual-graph/src/supply_chain_impact.rs::tests::one_report_per_abstention_kind` (planned) |
| Assemble the report from the federated rows | `MEDULLA-CQ-XSRC-R006.4` | `crates/medulla-source-types/src/supply_chain_report.rs::tests::assembly_is_byte_for_byte_deterministic_over_recorded_rows` |
| Serve the report through Arbora | `MEDULLA-CQ-XSRC-R006.5` | `crates/medulla-virtual-graph/src/supply_chain_impact.rs::tests::the_method_body_returns_a_report_matching_the_typed_model` |
| Acceptance on recorded connector responses | `MEDULLA-CQ-XSRC-R007.1` | `crates/medulla-rdf/tests/xsrc_supply_chain_acceptance.rs::the_recorded_responses_answer_the_impact_question_with_provenance` |
| Acceptance against the live connected sources | `MEDULLA-CQ-XSRC-R007.2` | `tests/federated/test_supply_chain_impact_served.py::test_report_from_ingested_connector_rows` (planned) |
| Acceptance with the procurement source not connected | `MEDULLA-CQ-XSRC-R007.3` | `tests/federated/test_supply_chain_impact_served.py::test_erpnext_absent_abstains_and_software_rows_unchanged` (planned) |
| Decisive test: changing only the authority declaration changes the sources used | `MEDULLA-CQ-XSRC-R007.4` | `tests/federated/test_supply_chain_impact_served.py::test_authority_swap_changes_sources_not_query_text` (planned) |
| Query fixture and authority rows: vulnerable deployed image to exposed service | `MEDULLA-CQ-XSRC-R008.1.1` | `crates/medulla-rdf/tests/xsrc_supply_chain_query.rs::the_vulnerable_image_question_plans_to_exactly_the_named_sources` |
| Acceptance: exposed services listed with image, vulnerability and sources | `MEDULLA-CQ-XSRC-R008.1.2` | `tests/federated/test_cross_source_questions_served.py::test_question_served[vulnerable_image_services]` (planned) |
| Declare the host limit-breach terms in the ontology | `MEDULLA-CQ-XSRC-R008.2.1` | `crates/medulla-rdf/tests/infrastructure_host_limits.rs::host_limit_breach_terms_listed_with_host_domain`; `crates/medulla-rdf/tests/infrastructure_host_limits.rs::host_limit_entity_terms_listed_and_no_overlimit_class` |
| Authority rows and query fixture: capability to service to host over a limit | `MEDULLA-CQ-XSRC-R008.2.3` | `crates/medulla-rdf/tests/xsrc_supply_chain_query.rs::the_capability_host_limit_question_plans_to_exactly_the_named_sources` |
| Acceptance: capabilities at risk listed with the host, reading and limit | `MEDULLA-CQ-XSRC-R008.2.4` | `tests/federated/test_cross_source_questions_served.py::test_question_served[host_limit_capabilities]` (planned) |
| Add `:CustomerContract` and `:coversService`; reuse canonical `kg:Change` | `MEDULLA-CQ-XSRC-R008.3.1` | `crates/medulla-rdf/tests/commercial_declarations.rs::customer_contract_and_covers_service_declared` |
| Authority rows and query fixture: open incident or change to service to contract | `MEDULLA-CQ-XSRC-R008.3.3` | `crates/medulla-rdf/tests/xsrc_supply_chain_query.rs::the_incident_contract_question_plans_to_exactly_the_named_sources` |
| Acceptance: open incidents and changes listed with the service and contract | `MEDULLA-CQ-XSRC-R008.3.4` | `tests/federated/test_cross_source_questions_served.py::test_question_served[contract_incidents]` (planned) |
| Serve the three generalization questions as Method::CrossSourceQuestion | `MEDULLA-CQ-XSRC-R008.4` | `tests/federated/test_cross_source_questions_served.py::test_unknown_question_refuses` (planned); `crates/medulla-source-types/src/cross_source_report.rs::tests::report_round_trips` (planned) |
| Query fixture: claims that touch the report | `MEDULLA-CQ-XSRC-R009.1` | `tests/test_cq_xsrc_r009.py::test_fixture_compiles_to_one_plan` (planned) |
| Evidence clustering | `MEDULLA-CQ-XSRC-R009.2` | `tests/test_cq_xsrc_r009.py::test_agent_claims_against_report_one_plan` (planned) |
| Contradiction decay analysis | `MEDULLA-CQ-XSRC-R009.3` | `tests/test_cq_xsrc_r009.py::test_agent_claims_against_report_one_plan` (planned) |
| Served acceptance through arbora | `MEDULLA-CQ-XSRC-R009.4` | `tests/test_cq_xsrc_r009.py::test_agent_claims_against_report_one_plan` (planned) |
| Refusal | `MEDULLA-CQ-XSRC-R009.5` | `tests/test_cq_xsrc_r009.py::test_unknown_report_refuses` (planned); `tests/test_cq_xsrc_r009.py::test_empty_window_is_empty_result` (planned) |
| Serve the supply-chain impact report as a Medulla method | `MEDULLA-CQ-XSRC-R011` | `tests/federated/test_supply_chain_impact_served.py::test_method_returns_typed_report` (planned) |
| Cross-connector join with a provenance record per fact | `MEDULLA-CQ-XSRC-R012.1` | `tests/federated/test_cross_connector_served.py::test_join_across_two_virtual_graphs_carries_provenance` (planned) |
| An unreadable source is excluded and named in coverage | `MEDULLA-CQ-XSRC-R012.2` | `tests/federated/test_cross_connector_served.py::test_unreadable_source_is_excluded_and_named_in_coverage` (planned) |

- **MEDULLA-CQ-XSRC-R005.6:** `crates/medulla-federation/tests/fragment_trace.rs::trace_carries_authority_rank_generation_and_pushed_fields` (planned). Proves the row at its entry point; no receipt is asserted.
- **MEDULLA-CQ-XSRC-R009.6:** `crates/medulla-uql/tests/evidence_for_rows.rs::evidence_for_runs_once_per_input_row` (planned). Proves the row at its entry point; no receipt is asserted.
