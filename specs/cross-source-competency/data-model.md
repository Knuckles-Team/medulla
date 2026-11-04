# MEDULLA-CQ-XSRC — Data model

The report types of `MEDULLA-CQ-XSRC-R006` and the served question types of `MEDULLA-CQ-XSRC-R008.4`.

## Supply-chain impact report

`crates/medulla-source-types/src/supply_chain_report.rs` defines the typed report served by `Method::SupplyChainImpact` (`MEDULLA-CQ-XSRC-R011`):

- `SupplyChainImpactReport { service_impacts, sources_used, abstentions }`: one report answers the exit-gate question; a non-empty abstention list never turns the report into an error.
- `ServiceImpact { service, items }`: one entry per impacted service, sorted by service IRI.
- `ServiceImpactItem { supplier, component_version, vulnerability, impact_kind, source }`: every item carries the source that served it; an item without a source is refused (`MissingItemSource`), never defaulted (`MEDULLA-CQ-XSRC-R006.1`).
- `ImpactKind`: `DirectDependency` (a repository dependency list) or `RuntimeChain` (the `deployedAs → usesImage → containsComponent` chain).
- `SourceUsed { source, class_served, authority_premise }`: the source, the ontology class it served, and the `vg:SourceAuthority` row that named it. Freshness per source is `live` (no watermark) or `accelerated` (watermark and age in seconds) (`MEDULLA-CQ-XSRC-R006.2`).
- `ReportAbstention`: `Unconnected { source, class }`, `Uncovered { class }`, `Disconnected { class }`, `UnapprovedMapping { class, mapping }`, one variant per engine reason, with no second list (`MEDULLA-CQ-XSRC-R006.3`).
- `assemble_supply_chain_report(rows, premises, abstentions)`: the pure, deterministic assembly of `MEDULLA-CQ-XSRC-R006.4`; equal inputs yield byte-identical encodings.

## Cross-source question report

`crates/medulla-types/src/cross_source_report.rs` defines the report served by `Method::CrossSourceQuestion` (`MEDULLA-CQ-XSRC-R008.4`): `CrossSourceReport { question, items, sources_used, abstentions }` with `CrossSourceItem { subject_iri, related, source }`, reusing `SourceUsed` and `ReportAbstention` from `supply_chain_report.rs`. An item without a `source` refuses with `CQ_XSRC_ITEM_WITHOUT_SOURCE`; an unknown question id refuses with `CQ_XSRC_UNKNOWN_QUESTION`.

## Ontology additions

- `supply_chain-v1.ttl` (new file): `:Supplier` (subclass of `:Vendor`), `:SupplyChainItem`, `:suppliesItem`, `:SoftwareComponent`, `:ComponentVersion`, `:publishedBy`, `:Vulnerability`, `:affectsComponent`, `:usesComponent`, `:suppliesService` and `:serviceSupplier` (`MEDULLA-CQ-XSRC-R002` to `R004`).
- `infrastructure-v1.ttl`: `:temperatureCelsius`, `:temperatureLimitCelsius`, `:capacityUtilization`, `:capacityLimit` on `:Host` (`MEDULLA-CQ-XSRC-R008.2.1`).
- `commercial-v1.ttl`: `:CustomerContract` and `:coversService` (`MEDULLA-CQ-XSRC-R008.3.1`); the canonical `kg:Change` is reused, no commercial `:ChangeRequest` is added.
