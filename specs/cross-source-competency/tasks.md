# MEDULLA-CQ-XSRC — Tasks

One task per leaf row of `requirements.md`, producers first: vocabulary and seed rows, then linking, planning, report, acceptance and the generalization questions. Each task names its row; the row holds the caller chain, the shape and the test. Retired rows have no task.

## Tasks

- [ ] T001 [US1] MEDULLA-CQ-XSRC-R002.1 Add `:Supplier` as a subclass of `:Vendor`, and `:SupplyChainItem` with `:suppliesItem`
- [ ] T002 [US1] MEDULLA-CQ-XSRC-R002.2 Add `:SoftwareComponent`, `:ComponentVersion` and `:publishedBy`
- [ ] T003 [US1] MEDULLA-CQ-XSRC-R002.3 Add `:Vulnerability` and `:affectsComponent`
- [ ] T004 [US1] MEDULLA-CQ-XSRC-R002.5 Source-authority seed: suppliers, purchase items and third-party risk
- [ ] T005 [US1] MEDULLA-CQ-XSRC-R002.6 Source-authority seed: software components and vulnerabilities
- [ ] T006 [US1] MEDULLA-CQ-XSRC-R002.7 Virtual mapping fixture: erpnext supplier table to `:Supplier`
- [ ] T007 [US1] MEDULLA-CQ-XSRC-R003.1 Source-authority seed: `:Service` health and identity
- [ ] T008 [US1] MEDULLA-CQ-XSRC-R003.2 Source-authority seed: deployed workloads, images and hosts
- [ ] T009 [US1] MEDULLA-CQ-XSRC-R003.3 Source-authority seed: business capabilities
- [ ] T010 [US1] MEDULLA-CQ-XSRC-R003.4 Virtual mapping fixture: uptime monitor to `:Service`
- [ ] T011 [US1] MEDULLA-CQ-XSRC-R003.5 Virtual mapping fixture: container-manager deployment to `:Workload` and its image
- [ ] T012 [US4] MEDULLA-CQ-XSRC-R004.1 Add `:usesComponent` (service to component version) and `:suppliesService`
- [ ] T013 [US4] MEDULLA-CQ-XSRC-R004.3 Resolve a vendor in erpnext, leanix and dockerhub to one `:Supplier`
- [ ] T014 [US4] MEDULLA-CQ-XSRC-R004.4 Resolve a service in uptime, container-manager and the CMDB to one `:Service`
- [ ] T015 [US4] MEDULLA-CQ-XSRC-R004.5 Query-time link: a join may only cross an approved `owl:sameAs`
- [ ] T016 [US1] MEDULLA-CQ-XSRC-R005.1 Row: `select_sources` orders candidate mappings by `vg:authorityRank`
- [ ] T017 [US1] MEDULLA-CQ-XSRC-R005.2 Query fixture: the supply-chain impact query over classes only
- [ ] T018 [US1] MEDULLA-CQ-XSRC-R005.3 The plan is derived, not authored: planned sources equal the declared authorities
- [ ] T019 [US1] MEDULLA-CQ-XSRC-R005.4 An unconnected authoritative source yields a typed abstention, not a silent fallback
- [ ] T047 [US1] MEDULLA-CQ-XSRC-R005.6 Fragment trace carries authority detail
- [ ] T020 [US1] MEDULLA-CQ-XSRC-R005.5 Row: `EXPLAIN` shows the authority premise for each planned source
- [ ] T021 [US1] MEDULLA-CQ-XSRC-R006.1 Add the typed `SupplyChainImpactReport` model
- [ ] T022 [US1] MEDULLA-CQ-XSRC-R006.2 Report carries freshness per source
- [ ] T023 [US1] MEDULLA-CQ-XSRC-R006.3 Report abstentions map the source-authority reasons, with no second list
- [ ] T024 [US1] MEDULLA-CQ-XSRC-R006.4 Assemble the report from the federated rows
- [ ] T025 [US1] MEDULLA-CQ-XSRC-R006.5 Serve the report through Arbora
- [ ] T026 [US1] MEDULLA-CQ-XSRC-R007.1 Acceptance on recorded connector responses
- [ ] T027 [US1] MEDULLA-CQ-XSRC-R007.2 Acceptance against the live connected sources
- [ ] T028 [US1] MEDULLA-CQ-XSRC-R007.3 Acceptance with the procurement source not connected
- [ ] T029 [US2] MEDULLA-CQ-XSRC-R007.4 Decisive test: changing only the authority declaration changes the sources used
- [ ] T030 [US3] MEDULLA-CQ-XSRC-R008.1.1 Query fixture and authority rows: vulnerable deployed image to exposed service
- [ ] T031 [US3] MEDULLA-CQ-XSRC-R008.1.2 Acceptance: exposed services listed with image, vulnerability and sources
- [ ] T032 [US3] MEDULLA-CQ-XSRC-R008.2.1 Declare the host limit-breach terms in the ontology
- [ ] T033 [US3] MEDULLA-CQ-XSRC-R008.2.3 Authority rows and query fixture: capability to service to host over a limit
- [ ] T034 [US3] MEDULLA-CQ-XSRC-R008.2.4 Acceptance: capabilities at risk listed with the host, reading and limit
- [ ] T035 [US3] MEDULLA-CQ-XSRC-R008.3.1 Add `:CustomerContract` and `:coversService`; reuse canonical `kg:Change`
- [ ] T036 [US3] MEDULLA-CQ-XSRC-R008.3.3 Authority rows and query fixture: open incident or change to service to contract
- [ ] T037 [US3] MEDULLA-CQ-XSRC-R008.3.4 Acceptance: open incidents and changes listed with the service and contract
- [ ] T038 [US3] MEDULLA-CQ-XSRC-R008.4 Serve the three generalization questions as Method::CrossSourceQuestion
- [ ] T039 [US5] MEDULLA-CQ-XSRC-R009.1 Query fixture: claims that touch the report
- [ ] T048 [US5] MEDULLA-CQ-XSRC-R009.6 Per-input-row EVIDENCE FOR
- [ ] T040 [US5] MEDULLA-CQ-XSRC-R009.2 Evidence clustering
- [ ] T041 [US5] MEDULLA-CQ-XSRC-R009.3 Contradiction decay analysis
- [ ] T042 [US5] MEDULLA-CQ-XSRC-R009.4 Served acceptance through arbora
- [ ] T043 [US5] MEDULLA-CQ-XSRC-R009.5 Refusal
- [ ] T044 [US6] MEDULLA-CQ-XSRC-R011 Serve the supply-chain impact report as a Medulla method
- [ ] T045 [US6] MEDULLA-CQ-XSRC-R012.1 Cross-connector join with a provenance record per fact
- [ ] T046 [US6] MEDULLA-CQ-XSRC-R012.2 An unreadable source is excluded and named in coverage
