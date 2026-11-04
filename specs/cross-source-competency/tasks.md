# MEDULLA-CQ-XSRC — Implementation tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md). Producers first; each child is one 5-minute unit.

- [ ] T001 MEDULLA-CQ-XSRC-R001 Concept-to-source authority is declared in the ontology (rollup).
- [ ] T002 MEDULLA-CQ-XSRC-R001.1 Add `vg:SourceAuthority`, `vg:authoritativeSource`, `vg:authorityFor` and `vg:authorityRank` to the virtual-graph vocabulary.
- [ ] T003 MEDULLA-CQ-XSRC-R001.2 SHACL shape: a `vg:SourceAuthority` names exactly one connected source, one class and one rank.
- [ ] T004 MEDULLA-CQ-XSRC-R001.3 Authority applies through subclass entailment.
- [ ] T005 MEDULLA-CQ-XSRC-R001.4 Authority records the approval of its mapping.
- [ ] T006 MEDULLA-CQ-XSRC-R002 Supply-chain concepts and the sources that are authoritative for them (rollup).
- [ ] T007 MEDULLA-CQ-XSRC-R002.1 Add `:Supplier` as a subclass of `:Vendor`, and `:SupplyChainItem` with `:suppliesItem`; preserve source-owned `:suppliedBy` declarations.
- [ ] T008 MEDULLA-CQ-XSRC-R002.2 Add `:SoftwareComponent`, `:ComponentVersion` and `:publishedBy`.
- [ ] T009 MEDULLA-CQ-XSRC-R002.3 Add `:Vulnerability` and `:affectsComponent`.
- [ ] T010 MEDULLA-CQ-XSRC-R002.4 SHACL shape for supplier and component.
- [ ] T011 MEDULLA-CQ-XSRC-R002.5 Source-authority mapping: suppliers and purchase items.
- [ ] T012 MEDULLA-CQ-XSRC-R002.6 Source-authority mapping: software components and vulnerabilities.
- [ ] T013 MEDULLA-CQ-XSRC-R002.7 Virtual mapping fixture: erpnext supplier table to `:Supplier`.
- [ ] T014 MEDULLA-CQ-XSRC-R003 Service concepts and the sources that are authoritative for them (rollup).
- [ ] T015 MEDULLA-CQ-XSRC-R003.1 Source-authority mapping: `:Service` health.
- [ ] T016 MEDULLA-CQ-XSRC-R003.2 Source-authority mapping: deployed workloads and images.
- [ ] T017 MEDULLA-CQ-XSRC-R003.3 Source-authority mapping: business capabilities.
- [ ] T018 MEDULLA-CQ-XSRC-R003.4 Virtual mapping fixture: uptime monitor to `:Service`.
- [ ] T019 MEDULLA-CQ-XSRC-R003.5 Virtual mapping fixture: container-manager workload to `:Service` and `:ContainerImage`.
- [ ] T020 MEDULLA-CQ-XSRC-R004 Linking relation between a supply-chain entity and a service, with entity resolution across sources (rollup).
- [ ] T021 MEDULLA-CQ-XSRC-R004.1 Add `:usesComponent` (service to component version) and `:suppliesService`.
- [ ] T022 MEDULLA-CQ-XSRC-R004.2 SHACL shape: `:usesComponent` domain and range.
- [ ] T023 MEDULLA-CQ-XSRC-R004.3 Resolve a vendor in erpnext, leanix and dockerhub to one `:Supplier`.
- [ ] T024 MEDULLA-CQ-XSRC-R004.4 Resolve a service in uptime, container-manager and the CMDB to one `:Service`.
- [ ] T025 MEDULLA-CQ-XSRC-R004.5 Query-time link: a join may only cross an approved `owl:sameAs`.
- [ ] T026 MEDULLA-CQ-XSRC-R005 The federated query is planned from the ontology's source declarations (rollup).
- [ ] T027 MEDULLA-CQ-XSRC-R005.1 Source selection orders candidate mappings by the declared authority rank.
- [ ] T028 MEDULLA-CQ-XSRC-R005.2 Query fixture: the supply-chain impact query over classes only.
- [ ] T029 MEDULLA-CQ-XSRC-R005.3 The plan is derived, not authored: planned sources equal the declared authorities.
- [ ] T030 MEDULLA-CQ-XSRC-R005.4 An unconnected authoritative source yields a typed abstention, not a silent fallback.
- [ ] T031 MEDULLA-CQ-XSRC-R005.5 Explain output shows the authority premise for each planned source.
- [ ] T032 MEDULLA-CQ-XSRC-R006 The unified report shape (rollup).
- [ ] T033 MEDULLA-CQ-XSRC-R006.1 Add the typed `SupplyChainImpactReport` model.
- [ ] T034 MEDULLA-CQ-XSRC-R006.2 Report carries freshness per source.
- [ ] T035 MEDULLA-CQ-XSRC-R006.3 Report abstention variants.
- [ ] T036 MEDULLA-CQ-XSRC-R006.4 Assemble the report from the federated rows.
- [ ] T037 MEDULLA-CQ-XSRC-R006.5 Serve the report through Arbora.
- [ ] T038 MEDULLA-CQ-XSRC-R007 End-to-end acceptance from real connected data (rollup).
- [ ] T039 MEDULLA-CQ-XSRC-R007.1 Acceptance on recorded connector responses.
- [ ] T040 MEDULLA-CQ-XSRC-R007.2 Acceptance against the live connected sources.
- [ ] T041 MEDULLA-CQ-XSRC-R007.3 Acceptance with the procurement source not connected.
- [ ] T042 MEDULLA-CQ-XSRC-R007.4 Decisive test: changing only the ontology's authority declaration changes the sources used.
- [ ] T043 MEDULLA-CQ-XSRC-R008 Generalization: three more cross-source questions of the same shape (rollup).
- [ ] T044 MEDULLA-CQ-XSRC-R008.1 Which services are exposed by a vulnerability in an image we deploy? (rollup)
- [ ] T045 MEDULLA-CQ-XSRC-R008.1.1 Query fixture and authority rows: vulnerable deployed image to exposed service.
- [ ] T046 MEDULLA-CQ-XSRC-R008.1.2 Acceptance: exposed services listed with image, vulnerability and sources.
- [ ] T047 MEDULLA-CQ-XSRC-R008.2 Which business capabilities depend on a host that is over its temperature or capacity limit? (rollup)
- [ ] T048 MEDULLA-CQ-XSRC-R008.2.1 Add a host limit-breach term to the ontology.
- [ ] T049 MEDULLA-CQ-XSRC-R008.2.2 SHACL shape: a limit-bearing host carries both reading and limit.
- [ ] T050 MEDULLA-CQ-XSRC-R008.2.3 Authority rows and query fixture: capability to service to host over a limit.
- [ ] T051 MEDULLA-CQ-XSRC-R008.2.4 Acceptance: capabilities at risk listed with the host, reading and limit.
- [ ] T052 MEDULLA-CQ-XSRC-R008.3 Which open incidents or changes touch a service a customer contract depends on? (rollup)
- [ ] T053 MEDULLA-CQ-XSRC-R008.3.1 Add `:CustomerContract` and `:coversService`; reuse canonical `kg:Change` (the `:BusinessProcess` change), with no commercial `:ChangeRequest`.
- [ ] T054 MEDULLA-CQ-XSRC-R008.3.2 SHACL shape: a contract names a customer and at least one service.
- [ ] T055 MEDULLA-CQ-XSRC-R008.3.3 Authority rows and query fixture for `:Incident` and canonical `:Change` to service to contract.
- [ ] T056 MEDULLA-CQ-XSRC-R008.3.4 Acceptance: open incidents and changes listed with the service and contract.

## Agent-memory substrate claims (2026-10-22)

- [ ] T057 MEDULLA-CQ-XSRC-R009 Acceptance: high-confidence agent claims against a new external report (rollup).
- [ ] T058 MEDULLA-CQ-XSRC-R009.1 Query fixture: claims that touch the report.
- [ ] T059 MEDULLA-CQ-XSRC-R009.2 Evidence clustering.
- [ ] T060 MEDULLA-CQ-XSRC-R009.3 Contradiction decay analysis.
- [ ] T061 MEDULLA-CQ-XSRC-R009.4 Served acceptance through arbora.
- [ ] T062 MEDULLA-CQ-XSRC-R009.5 Refusal.

## Served method and final-phase scenario (2026-11-04)

- [ ] T063 MEDULLA-CQ-XSRC-R008.4 Serve the three generalization questions as one engine method.
- [ ] T064 MEDULLA-CQ-XSRC-R011 Serve the supply-chain impact report as a Medulla method.
- [ ] T065 MEDULLA-CQ-XSRC-R012 Served proof of cross-connector answers for the final phase scenario (rollup).
- [ ] T066 MEDULLA-CQ-XSRC-R012.1 Cross-connector join with a provenance record per fact.
- [ ] T067 MEDULLA-CQ-XSRC-R012.2 An unreadable source is excluded and named in coverage.
