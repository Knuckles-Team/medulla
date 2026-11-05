# MEDULLA-SUPPLY-VOCAB — Data model

This file describes the ontology classes and properties the spec adds and the record the ecosystem risk function returns.

## Entities

### Supply-chain terms (`supply_chain-v1.ttl`)

- Classes: `kg:Supplier` (subclass of `kg:Vendor`), `kg:SupplyChainItem`, `kg:SoftwareComponent`, `kg:ComponentVersion`, `kg:Vulnerability`, `kg:EcosystemPackage`.
- Properties: `suppliesItem`, `realizedByAsset`, `assetHost`, `suppliesService`, `suppliesServiceViaHost`, `containsComponent`, `publishedBy`, `usesImage`, `deployedAs`, `usesComponent`, `cveId`, `severity`, `fixedInVersion`, `affectsComponent`, `affectsPackage`.
- Rule: shared properties keep their domain; module-local sub-properties carry the constraints.

### Commercial terms (`commercial-v1.ttl`)

- Classes: `kg:Customer`, `kg:CustomerContract`, `kg:PurchaseOrder`, `kg:PurchaseInvoice`, `kg:Opportunity`.
- Properties: `contractCustomer`, `coversService`, `contractStatus`, `contractEnd`, `affectsService`, `supportsCapability`, `invoiceSupplier`, `opportunityCustomer`, `contactOf`, `customerLastActivityAt`, `closeDate`.
- Rule: `kg:Organization` and `kg:Person` are reused; no `:Party` or `:Contact` class exists.

### Host limit terms (`infrastructure-v1.ttl`)

- Class: `infra:HostLimit`; properties `limitOf`, `limitKind`, `readingValue`, `limitValue`, `limitUnit`.
- Rule: `limitKind` is one of temperature, disk, memory, cpu; there is no stored `OverLimit` class.

### PackageImpact

- Fields: dependent package, dependency path, worst severity.
- Owner: `crates/medulla-core/src/ecosystem.rs`; refusal `PackageRefusal::UnknownPackage`.
- Relation: `PackageImpactRow` in `crates/medulla-result-types/src/compute_result/mining/impact.rs` carries it on the wire.
