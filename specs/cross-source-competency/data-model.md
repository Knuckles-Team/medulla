# MEDULLA-CQ-XSRC — Data model

The report types of `MEDULLA-CQ-XSRC-R006` and the served question types of `MEDULLA-CQ-XSRC-R008.4`.

## Supply-chain impact report

`crates/medulla-source-types/src/supply_chain_report.rs` defines the typed report served by `Method::SupplyChainImpact` (`MEDULLA-CQ-XSRC-R011`):

- `SupplyChainImpactReport { service_impacts, sources_used, abstentions }`: one report answers the exit-gate question; a non-empty abstention list never turns the report into an error.
- `ServiceImpact { service, items }`: one entry per impacted service, sorted by service IRI.
- `ServiceImpactItem { supplier, component_version, vulnerability, impact_kind, source }`: every item carries the source that served it; an item without a source is refused (`MissingItemSource`), never defaulted (`MEDULLA-CQ-XSRC-R006.1`).
- `ImpactKind`: `DirectDependency` (a repository dependency list) or `RuntimeChain` (the `deployedAs → usesImage → containsComponent` chain).
- `SourceUsed { source, class_served, authority_premise }`: the source, the ontology class it served, and the `vg:SourceAuthority` row that named it. `MEDULLA-CQ-XSRC-R006.2` adds `mode` (`AuditSourceMode` of `crates/medulla-identity-types/src/authority_audit.rs`: `live` with no watermark, or `accelerated` with a watermark and the age in seconds); the fields do not exist yet.
- `ReportAbstention`: `Unconnected { source, class }`, `Uncovered { class }`, `Disconnected { class }`, `UnapprovedMapping { class, mapping }` today; `MEDULLA-CQ-XSRC-R006.3` adds one variant per remaining `NotUsedReason` (`Stale`, `ContractMismatch`, `Ambiguous`, `Proposed`, `RoleInsufficient`), with no second list.
- `assemble_supply_chain_report(rows, premises, abstentions)`: the pure, deterministic assembly of `MEDULLA-CQ-XSRC-R006.4`; equal inputs yield byte-identical encodings.

## Cross-source question report

`crates/medulla-source-types/src/cross_source_report.rs` (new) defines the report served by `Method::CrossSourceQuestion` (`MEDULLA-CQ-XSRC-R008.4`): `CrossSourceReport { question, items, sources_used, abstentions }` with `CrossSourceItem { subject_iri, related, source }`, reusing `SourceUsed` and `ReportAbstention` from `supply_chain_report.rs`. An item without a `source` refuses with `CQ_XSRC_ITEM_WITHOUT_SOURCE`; an unknown question id refuses with `CQ_XSRC_UNKNOWN_QUESTION`.

## Ontology additions

- `crates/medulla-core/ontology/supply_chain-v1.ttl`: `:Supplier` (subclass of `:Vendor`), `:SupplyChainItem`, `:suppliesItem`, `:SoftwareComponent`, `:ComponentVersion`, `:publishedBy`, `:Vulnerability`, `:affectsComponent`, `:usesComponent`, `:suppliesService` and `:serviceSupplier` (`MEDULLA-CQ-XSRC-R002` to `R004`).
- `infrastructure-v1.ttl`: `:temperatureCelsius`, `:temperatureLimitCelsius` and `:capacityUtilization` on `:Host` (`MEDULLA-CQ-XSRC-R008.2.1`); the limits of disk, memory and cpu are `infra:HostLimit` rows (`MEDULLA-SUPPLY-VOCAB-R004.1`).
- `commercial-v1.ttl`: `:CustomerContract` and `:coversService` (`MEDULLA-CQ-XSRC-R008.3.1`); the canonical `kg:Change` is reused, no commercial `:ChangeRequest` is added.

## Request and response types

`Method::SupplyChainImpact` (`MEDULLA-CQ-XSRC-R011`) and `Method::CrossSourceQuestion { question: CrossSourceQuestionId }` (`MEDULLA-CQ-XSRC-R008.4`) are new variants in `crates/medulla-protocol-types/src/protocol/method/method_10.rs`. `CrossSourceQuestionId` is the closed enum `ExposedServices`, `HostLimitCapabilities`, `ContractIncidents`.

## Proposed terms (owner review)

None. Every ontology term the rows read exists in a registered module under `crates/medulla-core/ontology/`.
