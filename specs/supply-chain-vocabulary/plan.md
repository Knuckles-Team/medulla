# MEDULLA-SUPPLY-VOCAB — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The cross-source question "how does the supply chain affect our services?" needs a shared vocabulary that no single connector owns. This plan holds that vocabulary: the physical and software supply-chain terms in `supply_chain-v1.ttl`, the commercial terms in `commercial-v1.ttl`, host limits in `infrastructure-v1.ttl`, their SHACL shapes, the `vg:VirtualMapping` fixtures per source concept, the blocking keys for entity resolution, the class-only impact query and the ecosystem package risk function. It reuses the existing `kg:` and `infra:` classes, the `CoreSpec` registry of `GraphSchemaSources` and the impact handler; it adds no store and copies no source data.

## Technical Context

- **Language/Version**: Turtle ontology and shapes files; Rust 2021 edition on the pinned toolchain for the tests and the propagation function.
- **Primary Dependencies**: `medulla-core`, `medulla-graph-core`, `medulla-rdf`, `medulla-shacl`, `medulla-h-mining`, `medulla-result-types`.
- **Storage**: Core schema sources compiled into the binary; no store.
- **Testing**: Parser, reasoner and shape tests in `crates/medulla-core/tests/` and `crates/medulla-rdf/tests/`; a served test in `crates/medulla-h-mining`.
- **Target Platform**: Linux x86_64 and aarch64 hosts.
- **Project Type**: Ontology files plus library crates.
- **Performance Goals**: `propagate_package_risk` finishes a 10000-package chain at p99 50 ms (`MEDULLA-SUPPLY-VOCAB-R009.6`).
- **Constraints**: One core module constrains each property; shared property domains never narrow; no connector name in a class.
- **Scale/Scope**: 54 leaf rows over 6 user stories.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Terms live in core sources; mappings stay virtual.
- **R-II. Every wait has a deadline**: pass. Traversal is depth-bounded.
- **R-III. Every row sets budgets and names a benchmark**: pass. `MEDULLA-SUPPLY-VOCAB-R009.6` holds the budget and the nightly group.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates; `medulla-h-mining` adds one `medulla-core` dependency.
- **Organization standards**: pass. Rows follow the spec standard and the wording standard.

## Project Structure

```text
crates/medulla-core/ontology/supply_chain-v1.ttl           existing: supply-chain terms
crates/medulla-core/ontology/supply_chain-v1.shapes.ttl    existing: shapes, wired by R001.4
crates/medulla-core/ontology/commercial-v1.ttl             existing: commercial terms
crates/medulla-core/ontology/commercial-v1.shapes.ttl      existing: commercial shapes
crates/medulla-core/ontology/infrastructure-v1.ttl         existing: host limit terms
crates/medulla-core/ontology/infrastructure-v1.shapes.ttl  existing: HostLimitShape
crates/medulla-graph-core/src/graph/schema_sources.rs      existing: CoreSpec entries
crates/medulla-core/src/ecosystem.rs                       existing: propagate_package_risk
crates/medulla-core/tests/                                 existing: vocabulary, fixtures, blocking keys
crates/medulla-rdf/tests/                                  existing: declaration and reasoner tests
crates/medulla-h-mining/src/mining/impact.rs               existing: ecosystem branch added
crates/medulla-result-types/src/compute_result/mining/impact.rs  existing: EcosystemQuery added
```

## Phase 0: Research

none

## Phase 1: Design

### Architecture

Owner of every term: `Medulla`, `crates/medulla-core/ontology/`. Sources keep their data. Each source's ontology and access contract are attached and a `vg:VirtualMapping` (`virtual_graph-v1.ttl:107`, `vg:mapsOntologyClass` line 72, `vg:approved` line 94) maps its native entity to these classes. Source choice comes from `vg:SourceAuthority` rows (`MEDULLA-CQ-XSRC-R001`), planning from `MEDULLA-FEDERATED-QUERY-R072/R073`.

```
question "how does the supply chain affect our services?"
  -> classes only: infra:Service <- kg:suppliesService / kg:usesComponent / kg:coversService
  -> authority rows (MEDULLA-CQ-XSRC) pick one source per class
  -> access contract fetch at query time (MCP tool / SOQL / GraphQL)
  -> join across approved owl:sameAs only (MEDULLA-UNIFIED-DATA-PLANE-R030)
  -> rows + freshness + abstentions -> SupplyChainImpactReport (MEDULLA-CQ-XSRC-R006)
```

### Vocabulary

Prefixes: `kg:` = `http://knuckles.team/kg#`; `infra:` = `http://knuckles.team/kg/infrastructure#`. All new classes are `owl:Class` with `rdfs:label` and a BFO parent consistent with neighbours.

#### Physical and commercial supply chain — new file `supply_chain-v1.ttl` (prefix `:` = `kg:`)

| Term | Kind | Domain to range / parent | Note |
|---|---|---|---|
| `kg:Supplier` | class | `rdfs:subClassOf kg:Vendor` (`trm-v1.ttl:298`) | same IRI as `erpnext.ttl` `:Supplier` |
| `kg:SupplyChainItem` | class | `rdfs:subClassOf bfo:0000031` | connector `erp:Item` maps here |
| `kg:suppliesItem` | object property | `kg:Supplier` to `kg:SupplyChainItem` | derived from PO: `PurchaseOrder -suppliedBy-> Supplier`, `PurchaseOrder -contains-> Item` |
| `kg:realizedByAsset` | object property | `kg:SupplyChainItem` to `kg:AssetInstance` (`trm-v1.ttl`) | ERPNext Asset links item to a deployed asset |
| `kg:assetHost` | object property | `kg:AssetInstance` to `infra:Host` | asserted only through approved `owl:sameAs` or a CMDB CI link |
| `infra:serviceRunsOn` | object property | `infra:Service` to `infra:Host`. Subproperty of `infra:runsOn` | projected only for independently identified Service records. Never infer this edge from an arbitrary runsOn triple |
| `kg:suppliesService` | object property | `kg:Supplier` to `infra:Service` | derived. Physical chain on sub-property `kg:suppliesServiceViaHost`: `suppliesItem, realizedByAsset, assetHost, ^infra:serviceRunsOn` (a service runs on a host). Software chain lives on the inverse `kg:serviceSupplier`: `deployedAs, usesImage, containsComponent, supplyChainVersionOf, publishedBy` (service to supplier) |

`infra:runsOn` also links containers and pods to hardware. The physical chain must therefore use `infra:serviceRunsOn`. A container-only or pod-only runsOn fixture must not infer suppliesService or retype the runtime entity as a Service.

Reuse, not redefinition: `kg:suppliedBy` is **not** redeclared in this work; trm-v1.ttl already declares it for TechnologyProduct to Vendor. `erpnext.ttl` fixes its domain to `:PurchaseOrder`; a shared property must not be constrained by one module (the `trm-v1.ttl` `trmCurrentValue` precedent). The cross-source spec's "`:suppliedBy` item to supplier" is replaced by `kg:suppliesItem` (Decision D5).

#### Software supply chain — same file

| Term | Kind | Domain to range / parent | Note |
|---|---|---|---|
| `kg:SoftwareComponent` | class | `rdfs:subClassOf bfo:0000031` | a library, package or base image |
| `infra:ContainerImage` | existing | add `rdfs:subClassOf kg:SoftwareComponent` in `infrastructure-v1.ttl` | one triple |
| `kg:ComponentVersion` | class | `rdfs:subClassOf bfo:0000031` | |
| `kg:versionOf` | object property | `kg:ComponentVersion` to `kg:SoftwareComponent` | |
| `kg:versionString` | datatype | `kg:ComponentVersion` to `xsd:string` | |
| `kg:containsComponent` | object property | `infra:ContainerImage` to `kg:ComponentVersion` | from Docker Scout SBOM (`hub_scout` action `sbom`) |
| `kg:publishedBy` | object property | `kg:SoftwareComponent` to `kg:Supplier` | Docker Hub namespace / GitHub or GitLab owner |
| `kg:usesImage` | object property | `infra:Container` or `infra:Workload` to `infra:ContainerImage` | same IRI as `container.ttl:83` |
| `kg:deployedAs` | object property | `infra:Service` to `infra:Workload` | from entity resolution of service and workload |
| `kg:usesComponent` | object property | `infra:Service` to `kg:ComponentVersion` | chain: `deployedAs, usesImage, containsComponent` (plus a direct form from a repository dependency list) |
| `kg:Vulnerability` | class | `rdfs:subClassOf kg:TechnologyRisk` if `trm-v1.ttl` declares it, else `bfo:0000031` | same IRI as the dockerhub ontology |
| `kg:cveId`, `kg:severity`, `kg:fixedInVersion` | datatype | `kg:Vulnerability` to `xsd:string` | |
| `kg:affectsComponent` | object property | `kg:Vulnerability` to `kg:ComponentVersion` | |
| `kg:hasVulnerability` | object property | `infra:ContainerImage` to `kg:Vulnerability` | same IRI as dockerhub; kept, not replaced |

`kg:vulnerabilityCount` in `trm-v1.ttl` is left unchanged.

#### Commercial — new file `commercial-v1.ttl`

| Term | Kind | Domain to range / parent | Note |
|---|---|---|---|
| `kg:Customer` | class | `rdfs:subClassOf kg:Organization, kg:BusinessActor` | same IRI as `erpnext.ttl`; Salesforce `Account` and Twenty `Company` resolve to it |
| `kg:CustomerContract` | class | `rdfs:subClassOf bfo:0000031` | |
| `kg:contractCustomer` | object property | `kg:CustomerContract` to `kg:Customer` | |
| `kg:coversService` | object property | `kg:CustomerContract` to `infra:Service` | |
| `kg:contractStatus`, `kg:contractEnd` | datatype | string, date | filter: active contracts only |
| `kg:affectsService` | object property | `kg:Incident` or `kg:Change` to `infra:Service` | ServiceNow CI relation (`servicenow_api` `get_change_request_ci`, CMDB instance) mapped to it; `infra:Incident` keeps `infra:affectsEntity` |

No `:ChangeRequest` class is added (Decision D3).

#### Capability link — in `commercial-v1.ttl`

`kg:supportsCapability`: `infra:Service` to `kg:BusinessCapability` (`archimate-v1.ttl:19`). See Decision D1.

#### Host limits — additions to `infrastructure-v1.ttl`

| Term | Kind | Domain to range | Note |
|---|---|---|---|
| `infra:HostLimit` | class | `rdfs:subClassOf bfo:0000031` | one normalized (reading, limit) pair; a virtual row, not stored by default |
| `infra:limitOf` | object property | `infra:HostLimit` to `infra:Host` | |
| `infra:limitKind` | datatype | string in `{temperature, disk, memory, cpu}` | closed by SHACL `sh:in` |
| `infra:readingValue`, `infra:limitValue` | datatype | `xsd:decimal` | same unit per kind |
| `infra:limitUnit` | datatype | string (`celsius`, `bytes`, `count`) | |

"Over limit" is the query condition `?r > ?l`; no `OverLimit` class is stored (a stored breach would go stale between fetches).

#### SHACL (files `supply_chain-v1.shapes.ttl`, `commercial-v1.shapes.ttl`, host shape in `infrastructure-v1.shapes.ttl` created if absent)

- `SupplierShape`: `rdfs:label` min 1. `SupplyChainItemShape`: label min 1, at most one `suppliesItem` inverse not required.
- `ComponentVersionShape`: exactly one `versionOf` (class `SoftwareComponent`), exactly one `versionString`.
- `VulnerabilityShape`: exactly one `cveId`, `severity` in `{low, medium, high, critical, unknown}`, at least one `affectsComponent` or `hasVulnerability` inverse.
- `UsesComponentShape`: `usesComponent` subject class `infra:Service`, object class `ComponentVersion`.
- `CustomerContractShape`: exactly one `contractCustomer` (class `Customer`), at least one `coversService` (class `infra:Service`).
- `SupportsCapabilityShape`: subject `infra:Service`, object `BusinessCapability`.
- `HostLimitShape`: exactly one `limitOf`, one `limitKind` (`sh:in`), one `readingValue`, one `limitValue`.

### Entity resolution across sources

Mechanism: `MEDULLA-UNIFIED-DATA-PLANE-R030` (blocking, field similarity, Fellegi-Sunter, proposals inactive until approved; `crates/medulla-types/src/entity_link_proposal.rs`, `assert_same_as`). Its `AttachedApplication` enum covers Gramps, Immich, Twenty and Firefly only; the consumers `MEDULLA-CQ-XSRC-R004.3/.4` extend it. This spec adds only the **blocking keys per concept** as a data fixture (`crates/medulla-core/tests/fixtures/supply_chain_blocking_keys.ttl`):

| Concept | Sources joined | Blocking keys (strongest first) |
|---|---|---|
| `kg:Supplier` | erpnext `Supplier.supplier_name`, leanix `Provider` name, dockerhub `Organization`/namespace, github/gitlab owner | normalized legal name; web domain; namespace slug |
| `infra:Service` | uptime monitor, container-manager workload/K8sService, portainer `Service`, leanix `Application`, servicenow CI | normalized service name; hostname from monitor URL; namespace+name |
| `infra:Host` | systems-manager `HardwareNode`, fan-manager `ManagedHost`, ERPNext asset, CMDB CI | hostname; BMC address; serial number |
| `infra:ContainerImage` | dockerhub `Repository`/tag, container-manager image, portainer `Image` | `registry/repository:tag`; digest (exact) |
| `kg:Customer` | erpnext `Customer`, salesforce `Account`, twenty `Company` | normalized name; domain |

A digest match is an exact key; a name-only match is always a proposal, never auto-approved.

### Wiring

- Ontology load: Each `crates/medulla-core/ontology/*-v1.ttl` file is a `CoreSpec` entry in `crates/medulla-graph-core/src/graph/schema_sources.rs`; the `supply_chain`, `commercial` and `infrastructure` entries compose into every graph.
- Mappings: `vg:VirtualMapping` fixtures under `crates/medulla-core/tests/fixtures/` loaded by the same harness as `MEDULLA-CQ-XSRC-R002.7`.
- Authority rows (which source wins per class): `MEDULLA-CQ-XSRC-R002.5/.6`, `R003.1-.3`; this spec supplies the per-concept source table below, they declare the rows.
- Scopes: no new scope; reads use the existing federated-query read scope in the identity registry (`MEDULLA-IDENTITY-R004`).
- Connector side (cross-repo, agent-packages): resolve `schema_mappings.<X>.ontology_class` in each `connector_manifest.yml` to the medulla-core classes; the erpnext manifest currently lists `Supplier`, `Item`, `PurchaseOrder`, `Customer` as `ontology_class = None [UNRESOLVED]` (`erpnext-agent/connector_manifest.yml` review_todos).

### Decisions

**D1. Service to business capability: DECIDED `kg:supportsCapability` (new), not `:supports`, not a bare ArchiMate relationship.**
Evidence: `:supports` (`core-foundation-v1.ttl:575`) is defined as Evidence to claim ("SUPPORTS edge convention"); reusing it for service to capability mixes two meanings. `archimate-v1.ttl` declares only element anchors (`:BusinessCapability` line 19, `:Change` 33) and **no relationship properties**; the relationships live in the archimate-mcp ontology as reified `:ArchimateRelationship` plus direct `:relRealization`/`:relServing` (`archimate-mcp/.../ontology/archimate.ttl:106,397,402`), needing two hops and a model to be loaded. LeanIX uses a connector-side `supports` (Application to BusinessCapability, `leanix-agent/connector_manifest.yml:13`). Decision: one direct property, filled by three mappings (LeanIX `supports`; ArchiMate Realization or Serving from an application element to a capability; a CMDB business-service relation). Alternative: the ArchiMate relationship as the only form, rejected because it makes the report depend on an authored model. Revisit if archimate becomes the authority; then declare `kg:supportsCapability rdfs:subPropertyOf` nothing and keep the mapping.

**D2. erpnext suppliers and purchase orders: CONFIRMED, with a mapping gap.**
Tool `erpnext_agent_resource` (`erpnext_agent/mcp/mcp_resource.py:16`), actions `list_documents`, `get_document` (dispatch to `api_client_resource.py`), doctypes `Supplier`, `Item`, `Purchase Order`. Access contracts `supplierAccess`, `itemAccess`, `purchaseOrderAccess` exist in `erpnext_agent/ontology/access_contracts.ttl` (mcp_tool, offset pagination, pushdown filter and limit). The sync presets (`connectors/mcp_source_presets.json`) cover only Item and Sales Order, which does not matter for virtual access. Gaps: Supplier/Item/PurchaseOrder `ontology_class` unresolved in the manifest (row `MEDULLA-SUPPLY-VOCAB-R005.3`, agent-packages). `list_documents` does not return PO child lines, so item to supplier needs `get_document` per PO (row R005.2).

**D2b. uptime service health: CONFIRMED operation, no service link.**
`uptime_kuma_monitors` action `get_monitors`/`get_monitor` and `uptime_kuma_status` action `get_heartbeats` (`uptime_kuma_agent/mcp_server.py:74,140`). Resource `UptimeMonitor` and `HeartbeatStat` in the manifest. No access-contract `.ttl` was found under `uptime_kuma_agent/ontology/` (row R006.2 adds it, agent-packages). A monitor is not a `Service`; it resolves to one by name or URL host (blocking keys above). `lgtm-mcp` also lists `UptimeMonitor`/`HeartbeatStat` (rank 2).

**D2c. container-manager deployed workloads: CONFIRMED operation, no access contract.**
`cm_k8s_workloads` actions `list_pods`, `list_statefulsets` (`mcp/mcp_k8s_workloads.py:372`), container tool action `list_containers` (`mcp_server.py:426`), `cm_image_operations` action `list_images` (`mcp_server.py:301`); `container.ttl:83` has `usesImage`; manifest resources `Pod`, `Deployment`, `K8sService`, `SwarmService`. No `ac:operation` access contract under `container_manager_mcp/ontology/` (row R006.1, agent-packages). A `list_deployments` action was not found in the workloads tool; Deployment-level data is reached through `list_pods` plus `managedByDeployment`. portainer-agent (`Service`, `Image`, `Container`) is the rank-2 source.

**D3. `:ChangeRequest`: DECIDED not added commercially.** `kg:Change` (`archimate-v1.ttl:33`) is the ServiceNow change request and is a `:BusinessProcess`. The cross-source requirement `MEDULLA-CQ-XSRC-R008.3.1` reads `:CustomerContract` and `:coversService` only; `service_operations-v1.ttl` retains its distinct `:ChangeRequest` class for IT4IT `:touches` semantics. This amendment is recorded here. Alternative: `owl:equivalentClass`, rejected as a parallel name.

**D4. Which sources can supply each concept (CONFIRMED from tool lists):**

| Concept | Rank 1 | Rank 2 | Operation (CONFIRMED) | Not available |
|---|---|---|---|---|
| `Supplier` | erpnext | leanix `Provider` | `erpnext_agent_resource list_documents` (doctype Supplier); `leanix_source_factsheets` (`leanix_agent/mcp_server.py:172`, Provider in manifest) | |
| `SupplyChainItem` | erpnext | | `erpnext_agent_resource list_documents` (Item) | |
| `suppliesItem` | erpnext PO | | `list_documents` (Purchase Order) + `get_document` for lines | |
| `realizedByAsset` | | | no verified connected operation joins purchased Item to deployed AssetInstance | **BLOCKED**: owner operator plus erpnext-agent/servicenow mapping. Identify the Item-to-Asset field before declaring the physical chain covered. Return typed Uncovered until then |
| `AssetInstance` / `assetHost` | servicenow CMDB | erpnext asset | `get_cmdb_instances` (`servicenow_api/mcp_server.py:182`); systems-manager `HardwareNode` | erpnext Asset doctype read not verified; not used |
| `SoftwareComponent`, `ComponentVersion`, `containsComponent` | dockerhub (SBOM) | gitlab `dependencies` | `hub_scout` actions `sbom`, `summary` (`dockerhub_api/mcp/mcp_scout.py:23-28`); `gitlab_vulnerabilities` action `dependencies` (`gitlab_api/mcp_server.py:1119`) | github dependency graph: only `github_dependabot` alerts found, no SBOM action |
| `Vulnerability`, `affectsComponent` | dockerhub (`cves`, `vulnerabilities`) | gitlab (`get_project`), github Dependabot | `hub_scout`; `gitlab_vulnerabilities`; `github_dependabot` actions list/get (`github_agent/mcp/mcp_dependabot.py:150`) | |
| `publishedBy` | dockerhub namespace/Organization | github/gitlab owner | manifest resources `Organization`, `Namespace` | |
| `ContainerImage`, `usesImage` | container-manager | portainer, dockerhub | `cm_image_operations list_images`; container `list_containers`; portainer `Image`/`Container` resources | |
| `Service` health | uptime | lgtm | `uptime_kuma_status get_heartbeats` | |
| `deployedAs` (workload) | container-manager | portainer `Service`/`Stack` | `list_pods`, `list_containers` | |
| `Customer` | salesforce `Account` | twenty `Company`, erpnext `Customer` | `salesforce_soql` (`mcp/mcp_salesforce.py:27`); twenty GraphQL `crm` tool | |
| `CustomerContract` | salesforce | | `salesforce_soql` can query the standard `Contract` sObject if enabled in the org. The connector ontology declares no `Contract` class | **twenty: no contract object in manifest (Company, Opportunity, CrmNote, CrmTask, PipelineStage). Not available.** erpnext: no contract doctype in `erpnext.ttl` |
| `coversService` | | | no connected source records which service a contract covers | **BLOCKED**: Salesforce standard `Contract` has no service field. Needs either a custom field or a curated link. Owner: operator (CRM data) plus `MEDULLA-SUPPLY-VOCAB-R005.9` mapping once the field is named. Until then the report abstains `Uncovered` for contract questions |
| `Incident`, `Change`, `affectsService` | servicenow | erpnext `ErpNextIssue` (incident only) | `get_change_request_ci`, `get_cmdb_instances` | |
| `supportsCapability` | leanix | archimate | LeanIX `supports` relation (manifest); `relRealization`/`relServing` | |
| `HostLimit.readingValue` (temperature) | fan-manager | | `TemperatureReading.celsius`, `ThermalTrend.maxCelsius` (manifest fields) | |
| `HostLimit.limitValue` (temperature) | fan-manager `FanControlPolicy.maxTemperature` | | manifest field `maxTemperature` | the BMC vendor critical threshold is not exposed; the limit is the operator policy ceiling (DECIDED, D6) |
| `HostLimit` capacity (disk, memory, cpu) | systems-manager | | `HardwareNode`/`DiskVolume` fields `capacityBytes`, `freeBytes`, `memoryTotal`, `cpuCount` | no source exposes a capacity **limit**; the limit is a declared threshold (D6) |

**D5. `:suppliedBy` is not redeclared;** `kg:suppliesItem` carries supplier to item (reasoning in Vocabulary). Amends `MEDULLA-CQ-XSRC-R002.1`.

**D6. Limits are declared, readings are fetched.** A temperature limit comes from the fan-manager policy field; capacity limits (for example 85 percent of `capacityBytes`) are `infra:HostLimit` rows declared in a small ontology-side threshold table (`infra:limitValue` on a declared `HostLimit`), because no connected source provides one. Alternative (use IPMI sensor critical thresholds) rejected: not exposed by any connector read here.

### Worked example

Question: "Which services are exposed by a vulnerable component supplied by a vendor we also buy hardware from?"

```sparql
PREFIX kg: <http://knuckles.team/kg#>
PREFIX infra: <http://knuckles.team/kg/infrastructure#>
SELECT ?service ?supplier ?comp ?ver ?cve ?sev WHERE {
  ?service a infra:Service ; kg:usesComponent ?cv .
  ?cv kg:versionOf ?comp ; kg:versionString ?ver .
  ?comp kg:publishedBy ?supplier .
  ?vuln kg:affectsComponent ?cv ; kg:cveId ?cve ; kg:severity ?sev .
  ?supplier kg:suppliesItem ?item .          # physical side: same supplier after approved sameAs
}
```

Trace: `kg:Vulnerability` and `kg:ComponentVersion` select dockerhub (`hub_scout cves`, `sbom`); `infra:Service` selects container-manager plus uptime; `kg:Supplier` selects erpnext. A docker namespace "acme" and an ERPNext supplier "Acme Corp" are joined only if `MEDULLA-UNIFIED-DATA-PLANE-R030` produced an approved `owl:sameAs`; otherwise the row is returned with `abstention: UnapprovedLink`. Answer rows: `{service: "git", supplier: "Acme Corp", comp: "openssl", ver: "3.0.1", cve: "CVE-...", sev: "high", sources: [dockerhub, container-manager, erpnext]}`.

### Out of scope

Copying source data. The authority vocabulary (`MEDULLA-CQ-XSRC-R001`). The federated planner. Entity-resolution scoring. The report type. Building new connector tools (only mapping and access-contract rows in agent-packages).

### Commercial acceptance

Rows `MEDULLA-SUPPLY-VOCAB-R003.7` through `R003.17` own commercial declarations and acceptance.
Erpnext, Salesforce, and Twenty use `commercial-v1.ttl`; `MEDULLA-CQ-CONN-R083` owns their mapping acceptance.
The vocabulary reuses `kg:Organization` and `kg:Person` for Party and Contact representations.
R003.13 registers the module and generated catalog entry.
R003.14 registers its shapes companion; R003.15 checks namespace compatibility.
R003.16 supplies closeDate; R003.17 checks valid and invalid Opportunity fixtures.

Implementation follows tasks.md producer order. Registration precedes declarations; closeDate and the shapes companion precede Opportunity validation.
Tests check exact RDF types, canonical IRIs, domains and ranges. Existing shared property domains must not narrow.
Missing illustrative properties require bounded producer rows with named tests before fixtures consume them.
Contact joins use approved opaque identity links, never raw email matching.
Connector payload availability remains unverified and cannot be inferred from ontology declarations.
Live competency tests must abstain when required source fields or approved mappings are absent.

### Ecosystem package risk propagation (R009)

Data flow: served impact operation → `handle_impact` (`crates/medulla-h-mining/src/mining/impact.rs:268`) → `propagate_package_risk` (`crates/medulla-core/src/ecosystem.rs:69`) → `PackageImpact` list with paths. `medulla-h-mining` depends on `medulla-core` for this call, a downward dependency that keeps the layering.

Types: `:EcosystemPackage`, `:affectsPackage`, `PackageImpact { dependent, path, severity }` and `PackageRefusal::UnknownPackage`.

Attach point: the package branch of `handle_impact`, called by the server dispatch. The transitive `:dependsOn` property and its inverse already exist (`core-foundation-v1.ttl:306`, `:414`).

Failure behavior: an unknown package returns the typed refusal. A cycle in dependencies ends at the depth bound and marks the result `truncated`.

Reused: `:dependsOn`, the impact handler and the supply-chain ontology. New: one class, one property and one propagation function.

Decision: reverse traversal at query time. Rejected: materialize every transitive edge, because each change rewrites every materialized edge.

Cross-repository: the harness rows that ingest package manifests depend on the class name `:EcosystemPackage`.

## Phase 2: Task approach

`tasks.md` has one task per leaf row. Order: registration and declarations (R001, R002, R003), shapes, host limits (R004), mapping fixtures (R005, R006), blocking keys (R007), the impact query and recorded responses (R008), then ecosystem risk (R009). Rows on different files run in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Every new `.ttl` parses with the native parser; every shape accepts a valid fixture and refuses each fault.
- A public item in `medulla-core` has a non-test caller under a handler or server crate; `handle_impact` calls `propagate_package_risk`.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- Ontology terms: `crates/medulla-graph-core/src/graph/schema_sources.rs` (`CoreSpec` `supply_chain` at line 1162, `commercial` at line 1155, `infrastructure` at line 893) → `GraphSchemaSources::default()` → the composed schema of every graph → `validate_entering_schema` (`crates/medulla-server-api/src/schema_compose.rs:84`).
- Mapping fixtures: attached as operator schema sources → `fixture_from_schema_sources` (`crates/medulla-virtual-graph/src/source_selection/live.rs:198`) → `select_sources_with` for `Method::SourceCoverage` (`MEDULLA-SRC-AUTH-R004.6.1`).
- Ecosystem risk: `Method::MineRiskPropagation` → `crates/medulla-h-mining/src/mining/gateway_derived.rs:281` → `insight.rs::handle_risk_propagation` (line 205) → `impact.rs::handle_impact` (line 268) → `medulla_core::ecosystem::propagate_package_risk_with`.
- Obsolete path to delete: none.

## Cross-repo seams

- Connector class resolution and access contracts: `MEDULLA-SUPPLY-VOCAB-R005` and `MEDULLA-SUPPLY-VOCAB-R006` provide the classes, `CONN-ERPNEXT-AGENT-CONNECTOR-CONTENT-R002` and `CONN-SALESFORCE-AGENT-CONNECTOR-CONTENT-R002` consume them. Contract: [connector-class-resolution.md](contracts/connector-class-resolution.md).
- Mapping acceptance: `MEDULLA-SUPPLY-VOCAB-R003.13` provides the registered commercial module, `MEDULLA-CQ-CONN-R083` consumes it. Contract: [commercial-mapping-acceptance.md](contracts/commercial-mapping-acceptance.md).
