# MEDULLA-SUPPLY-VOCAB — Tasks

One task per leaf row of `requirements.md`, producer first. Each task names its row; the row holds the files, the caller chain and the proof.

## Tasks

- [ ] T001 [US1] MEDULLA-SUPPLY-VOCAB-R001.1.1 Declare shared supplier and item terms
- [ ] T002 [US1] MEDULLA-SUPPLY-VOCAB-R001.1.2 Install shared terms in the core catalog and prove native entailment
- [ ] T003 [US1] MEDULLA-SUPPLY-VOCAB-R001.2 Add `:realizedByAsset` and `:assetHost`
- [ ] T004 [US1] MEDULLA-SUPPLY-VOCAB-R001.3 Add `:suppliesService` and the physical chain sub-property
- [ ] T005 [US1] MEDULLA-SUPPLY-VOCAB-R001.4 SHACL shapes for supplier and item
- [ ] T006 [US2] MEDULLA-SUPPLY-VOCAB-R002.1 Add `:SoftwareComponent`, `:ComponentVersion`, shared `:versionOf`, constrained `:supplyChainVersionOf` and...
- [ ] T007 [US2] MEDULLA-SUPPLY-VOCAB-R002.2 Declare `infra:ContainerImage` a `kg:SoftwareComponent`
- [ ] T008 [US2] MEDULLA-SUPPLY-VOCAB-R002.3 Add `:containsComponent` and `:publishedBy`
- [ ] T009 [US2] MEDULLA-SUPPLY-VOCAB-R002.4 Add `:usesImage` and `:deployedAs`
- [ ] T010 [US2] MEDULLA-SUPPLY-VOCAB-R002.5 Add `:usesComponent` with the runtime chain axiom
- [ ] T011 [US2] MEDULLA-SUPPLY-VOCAB-R002.6 Add `:Vulnerability` with `:cveId`, `:severity`, `:fixedInVersion`, `:affectsComponent` and `:hasVulnerabil...
- [ ] T012 [US2] MEDULLA-SUPPLY-VOCAB-R002.7 SHACL shapes for component version, vulnerability and `usesComponent`
- [ ] T013 [US3] MEDULLA-SUPPLY-VOCAB-R003.1 Add `:Customer`
- [ ] T014 [US3] MEDULLA-SUPPLY-VOCAB-R003.2 Add `:CustomerContract`, `:contractCustomer`, `:coversService`, `:contractStatus` and `:contractEnd`
- [ ] T015 [US3] MEDULLA-SUPPLY-VOCAB-R003.3 Add `:affectsService`
- [ ] T016 [US3] MEDULLA-SUPPLY-VOCAB-R003.4 Add `:supportsCapability`
- [ ] T017 [US3] MEDULLA-SUPPLY-VOCAB-R003.5 SHACL shapes for contract and capability link
- [ ] T018 [US3] MEDULLA-SUPPLY-VOCAB-R003.7 Declare party links on `kg:Organization` (no `:Party` class)
- [ ] T019 [US3] MEDULLA-SUPPLY-VOCAB-R003.8 Add `:PurchaseOrder`
- [ ] T020 [US3] MEDULLA-SUPPLY-VOCAB-R003.9 Add `:PurchaseInvoice`
- [ ] T021 [US3] MEDULLA-SUPPLY-VOCAB-R003.10 Add `:Opportunity`
- [ ] T022 [US3] MEDULLA-SUPPLY-VOCAB-R003.11 Reuse `:Person` for `:Contact` and declare the link
- [ ] T023 [US3] MEDULLA-SUPPLY-VOCAB-R003.12 Add `:customerLastActivityAt`
- [ ] T024 [US3] MEDULLA-SUPPLY-VOCAB-R003.13 Register the commercial module
- [ ] T025 [US3] MEDULLA-SUPPLY-VOCAB-R003.14 Register the commercial shapes companion
- [ ] T026 [US3] MEDULLA-SUPPLY-VOCAB-R003.15 Preserve canonical commercial namespaces
- [ ] T027 [US3] MEDULLA-SUPPLY-VOCAB-R003.16 Declare the Opportunity date property
- [ ] T028 [US3] MEDULLA-SUPPLY-VOCAB-R003.17 Restore the Opportunity date constraint
- [ ] T029 [US4] MEDULLA-SUPPLY-VOCAB-R004.1 Add `infra:HostLimit`, `:limitOf`, `:limitKind`, `:readingValue`, `:limitValue`, `:limitUnit`
- [ ] T030 [US4] MEDULLA-SUPPLY-VOCAB-R004.2 SHACL shape for `HostLimit`
- [ ] T031 [US4] MEDULLA-SUPPLY-VOCAB-R004.3 Declared-threshold fixture for capacity limits
- [ ] T032 [US5] MEDULLA-SUPPLY-VOCAB-R005.1 Mapping fixture: erpnext `Supplier` to `kg:Supplier`
- [ ] T033 [US5] MEDULLA-SUPPLY-VOCAB-R005.2 Mapping fixture: erpnext `Item` and `Purchase Order` to `kg:SupplyChainItem` and `kg:suppliesItem`
- [ ] T034 [US5] MEDULLA-SUPPLY-VOCAB-R005.4 Mapping fixture: leanix `Provider` to `kg:Supplier` (rank 2) and `supports` to `kg:supportsCapability`
- [ ] T035 [US5] MEDULLA-SUPPLY-VOCAB-R005.5 Mapping fixture: servicenow `Incident`, `Change` and CI relation to `kg:affectsService`
- [ ] T036 [US5] MEDULLA-SUPPLY-VOCAB-R005.6 Mapping fixture: salesforce `Account` to `kg:Customer` and `Contract` to `kg:CustomerContract`
- [ ] T037 [US5] MEDULLA-SUPPLY-VOCAB-R005.8 Mapping fixture: twenty `Company` to `kg:Customer` (rank 2)
- [ ] T038 [US5] MEDULLA-SUPPLY-VOCAB-R005.9 Curated contract-to-service link
- [ ] T039 [US5] MEDULLA-SUPPLY-VOCAB-R006.3 Mapping fixture: container-manager workload and image to `kg:deployedAs` and `kg:usesImage`
- [ ] T040 [US5] MEDULLA-SUPPLY-VOCAB-R006.4 Mapping fixture: uptime monitor to service health
- [ ] T041 [US5] MEDULLA-SUPPLY-VOCAB-R006.5 Mapping fixture: dockerhub image, SBOM and Scout CVEs to `containsComponent` and `Vulnerability`
- [ ] T042 [US5] MEDULLA-SUPPLY-VOCAB-R006.6 Mapping fixture: gitlab `dependencies` and github Dependabot alerts (rank 2)
- [ ] T043 [US5] MEDULLA-SUPPLY-VOCAB-R006.7 Mapping fixture: fan-manager and systems-manager to `infra:HostLimit`
- [ ] T044 [US5] MEDULLA-SUPPLY-VOCAB-R007.1 Blocking-key fixture for supplier, service, host, image and customer
- [ ] T045 [US5] MEDULLA-SUPPLY-VOCAB-R007.2 Test: a digest key is exact; a name-only key is a proposal
- [ ] T046 [US5] MEDULLA-SUPPLY-VOCAB-R008.1 Class-only impact query fixture
- [ ] T047 [US5] MEDULLA-SUPPLY-VOCAB-R008.2 Recorded-response fixture: one row per source concept
- [ ] T048 [US5] MEDULLA-SUPPLY-VOCAB-R008.3 Blocked-register test
- [ ] T049 [US6] MEDULLA-SUPPLY-VOCAB-R009.1 Model
- [ ] T050 [US6] MEDULLA-SUPPLY-VOCAB-R009.2 Behavior
- [ ] T051 [US6] MEDULLA-SUPPLY-VOCAB-R009.3 Wiring
- [ ] T052 [US6] MEDULLA-SUPPLY-VOCAB-R009.4 Refusal
- [ ] T053 [US6] MEDULLA-SUPPLY-VOCAB-R009.5 Evidence
- [ ] T054 [US6] MEDULLA-SUPPLY-VOCAB-R009.6 Hot-path proof
