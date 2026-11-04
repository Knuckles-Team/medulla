# MEDULLA-CQ-XSRC — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Shape | Authority fixtures: valid, no source, no class, no rank, equal-rank tie, unapproved mapping | Valid accepted. Each fault refused. Unapproved mapping reports uncovered | PENDING |
| T-002 | FR-002 | Contract | Load `supply_chain-v1.ttl`, seed authority rows | Terms present; `select_sources` names the declared source per concept | PENDING |
| T-003 | FR-003 | Integration | Three vendor records and three service records across sources | One node after approval, separate before. Join empty before approval | PENDING |
| T-004 | FR-004 | Integration | Class-only SPARQL fixture | Planned sources equal authority-derived set; no connector name in query | PENDING |
| T-005 | FR-005 | Unit | Recorded rows, each abstention kind | Report shape, freshness and abstentions as specified | PENDING |
| T-006 | FR-006 | End to end | Recorded responses. Then live. Then procurement absent. Then authority swap | Non-empty provenance-carrying answer. Typed abstention. Sources differ with equal query text | PENDING |
| T-007 | FR-007 | End to end | Three generalization fixtures | Each non-empty with provenance, and abstention when its source is absent | PENDING |

## Negative and boundary cases

Equal-rank authority. Authority on an unapproved mapping. Unconnected rank-1 source with and without `vg:allowFallback`. Unapproved `owl:sameAs`. A report item with no source. A copy with no watermark. A connector name in the query text.

## Quality and release proof

Focused `cargo test` for `medulla-rdf`, `medulla-types` and `medulla-core` ontology tests; `pipelines-hook spec-status` and `spec-decomposition`. Live acceptance runs through Arbora and records revision, connected sources and output. A source test alone is not served acceptance.

## Agent-memory substrate claims (2026-10-22)

| Case | Requirement | Proving test |
|---|---|---|
| Acceptance: high-confidence agent claims against a new external report (rollup) | `MEDULLA-CQ-XSRC-R009` | Rollup: satisfied when the test of every child passes. |
| Query fixture: claims that touch the report | `MEDULLA-CQ-XSRC-R009.1` | `tests/test_cq_xsrc_r009.py::test_fixture_compiles_to_one_plan` |
| Evidence clustering | `MEDULLA-CQ-XSRC-R009.2` | `tests/test_cq_xsrc_r009.py::test_agent_claims_against_report_one_plan` |
| Contradiction decay analysis | `MEDULLA-CQ-XSRC-R009.3` | `tests/test_cq_xsrc_r009.py::test_agent_claims_against_report_one_plan` |
| Served acceptance through arbora | `MEDULLA-CQ-XSRC-R009.4` | `tests/test_cq_xsrc_r009.py::test_agent_claims_against_report_one_plan` |
| Refusal | `MEDULLA-CQ-XSRC-R009.5` | `tests/test_cq_xsrc_r009.py::test_unknown_report_refuses` |

## Virtual mapping fixtures (2026-11-03)

| Case | Requirement | Proving test |
|---|---|---|
| Virtual mapping fixture: erpnext supplier table to `:Supplier` | `MEDULLA-CQ-XSRC-R002.7` | `crates/medulla-core/tests/xsrc_mappings.rs::erpnext_mapping_passes_r037_shapes` |
| Virtual mapping fixture: container-manager workload to `:Service` and `:ContainerImage` | `MEDULLA-CQ-XSRC-R003.5` | `crates/medulla-core/tests/xsrc_mappings.rs::container_manager_mapping_passes_r037_shapes` |

## Bound tests for the seed, planning, report and acceptance rows (2026-11-04)

| Case | Requirement | Proving test |
|---|---|---|
| Source-authority seed: suppliers, purchase items and third-party risk | `MEDULLA-CQ-XSRC-R002.5` | `crates/medulla-rdf/tests/source_authority_fixture.rs::supplier_seed_passes_shape` |
| Source-authority seed: software components and vulnerabilities | `MEDULLA-CQ-XSRC-R002.6` | `crates/medulla-rdf/tests/source_authority_fixture.rs::component_seed_passes_shape` |
| Source-authority seed: `:Service` health and identity | `MEDULLA-CQ-XSRC-R003.1` | `crates/medulla-rdf/tests/source_authority_fixture.rs::service_seed_passes_shape` |
| Source-authority seed: deployed workloads, images and hosts | `MEDULLA-CQ-XSRC-R003.2` | `crates/medulla-rdf/tests/source_authority_fixture.rs::workload_seed_passes_shape` |
| Source-authority seed: business capabilities | `MEDULLA-CQ-XSRC-R003.3` | `crates/medulla-rdf/tests/source_authority_fixture.rs::capability_seed_passes_shape` |
| Virtual mapping fixture: uptime monitor to `:Service` | `MEDULLA-CQ-XSRC-R003.4` | `crates/medulla-core/tests/xsrc_mappings.rs::uptime_mapping_passes_r037_shapes` |
| Resolve a service in uptime, container-manager and the CMDB to one `:Service` | `MEDULLA-CQ-XSRC-R004.4` | `crates/medulla-types/src/entity_link_proposal.rs::tests::service_kinds_refuse_unapproved` |
| Query-time link: a join may only cross an approved `owl:sameAs` | `MEDULLA-CQ-XSRC-R004.5` | `tests/federated/test_supply_chain_join_served.py::test_join_needs_approved_same_as` |
| Explain lists the authority premise for each planned source | `MEDULLA-CQ-XSRC-R005.5` | `tests/federated/test_supply_chain_authority_served.py::test_explain_lists_authority_premises_without_urls` |
| Report carries freshness per source | `MEDULLA-CQ-XSRC-R006.2` | `crates/medulla-source-types/src/supply_chain_report.rs::tests::freshness_modes` |
| Report abstentions map the source-authority reasons | `MEDULLA-CQ-XSRC-R006.3` | `crates/medulla-virtual-graph/src/supply_chain_impact.rs::tests::one_report_per_abstention_kind` |
| Acceptance against the live connected sources | `MEDULLA-CQ-XSRC-R007.2` | `tests/federated/test_supply_chain_impact_served.py::test_report_from_ingested_connector_rows` |
| Acceptance with the procurement source not connected | `MEDULLA-CQ-XSRC-R007.3` | `tests/federated/test_supply_chain_impact_served.py::test_erpnext_absent_abstains_and_software_rows_unchanged` |
| Decisive test: changing only the authority declaration changes the sources used | `MEDULLA-CQ-XSRC-R007.4` | `tests/federated/test_supply_chain_impact_served.py::test_authority_swap_changes_sources_not_query_text` |
| Acceptance: exposed services listed with image, vulnerability and sources | `MEDULLA-CQ-XSRC-R008.1.2` | `tests/federated/test_cross_source_questions_served.py::test_question_served[vulnerable_image_services]` |
| SHACL shape: a limit-bearing host carries both reading and limit | `MEDULLA-CQ-XSRC-R008.2.2` | `crates/medulla-core/tests/supply_chain_vocab.rs::host_limit_pair_shape` |
| Acceptance: capabilities at risk listed with the host, reading and limit | `MEDULLA-CQ-XSRC-R008.2.4` | `tests/federated/test_cross_source_questions_served.py::test_question_served[host_limit_capabilities]` |
| SHACL shape: a contract names a customer and at least one service | `MEDULLA-CQ-XSRC-R008.3.2` | `crates/medulla-core/tests/supply_chain_vocab.rs::customer_contract_shape` |
| Acceptance: open incidents and changes listed with the service and contract | `MEDULLA-CQ-XSRC-R008.3.4` | `tests/federated/test_cross_source_questions_served.py::test_question_served[contract_incidents]` |
| Serve the three generalization questions as one engine method | `MEDULLA-CQ-XSRC-R008.4` | `crates/medulla-types/src/cross_source_report.rs::tests::report_round_trips` |
| Serve the supply-chain impact report as a Medulla method | `MEDULLA-CQ-XSRC-R011` | `tests/federated/test_supply_chain_impact_served.py::test_method_returns_typed_report` |
| Cross-connector join with a provenance record per fact | `MEDULLA-CQ-XSRC-R012.1` | `tests/federated/test_cross_connector_served.py::test_join_across_two_virtual_graphs_carries_provenance` |
| An unreadable source is excluded and named in coverage | `MEDULLA-CQ-XSRC-R012.2` | `tests/federated/test_cross_connector_served.py::test_unreadable_source_is_excluded_and_named_in_coverage` |
