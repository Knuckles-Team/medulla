# MEDULLA-CQ-XSRC — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine answers one question over several connected sources by planning from the ontology's authority rows, reading each source at query time through its approved virtual mapping, and assembling one typed report. This plan adds no planner: it adds the seed authority rows, the linking relations, the report model, the two served methods and the acceptance tests on top of the existing source selection, federated query and unified data plane. Nothing in the query text names a connector.

```text
caller (arbora ask)
  -> Method::SupplyChainImpact / Method::CrossSourceQuestion   crates/medulla-protocol-types/src/protocol/method/method_10.rs
  -> try_handle                                                crates/medulla-h-sparql/src/foreign_source_handlers.rs
  -> handle (new)                                              crates/medulla-h-sparql/src/supply_chain_impact.rs, cross_source_question.rs
  -> select_sources_with                                       crates/medulla-virtual-graph/src/source_selection.rs
  -> report assembly                                           crates/medulla-source-types/src/supply_chain_report.rs
```

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `crates/medulla-virtual-graph`, `crates/medulla-source-types`, `crates/medulla-h-sparql`, `crates/medulla-federation`, `crates/medulla-identity-types` and `crates/medulla-core` (ontology modules). No new third-party dependency.
- **Storage**: None added. Source rows stay at the source; the methods write nothing.
- **Testing**: Rust tests under `crates/medulla-rdf/tests/` and `crates/medulla-core/tests/`, unit tests in the report and handler modules, served Python tests under `tests/federated/` and `tests/test_cq_xsrc_r009.py`; recorded fixtures, no live credential or network in the pull-request gate.
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Handler, type and ontology additions to the existing engine crates.
- **Performance Goals**: A question over recorded fixtures finishes within the budget of `MEDULLA-FEDERATED-QUERY-R722`; the agent-claims latency is measured by `MEDULLA-DURABLE-KERNEL-R042.2`.
- **Constraints**: A wrong answer is a failure, not an abstention. An unavailable required source abstains by name. Each report item carries its source. No version suffix in a name.
- **Scale/Scope**: 46 leaf rows over 6 user stories, 9 `FR-` labels and 5 `SC-` labels.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Questions run in the engine; external stores stay virtual-graph sources.
- **R-II. Every wait has a deadline**: pass. Each foreign read carries the deadline of the federated query contract.
- **R-III. Every row sets budgets and names a benchmark**: pass. `MEDULLA-DURABLE-KERNEL-R042.2` measures the agent-claims program.
- **R-IV. Crates are small, layered and cut over hard**: pass. Handlers sit in `medulla-h-sparql`, types in `medulla-source-types`, selection in `medulla-virtual-graph`; no handler calls a sibling handler.
- **Organization standards**: pass. Rows follow the spec standard, the ecosystem boundaries and the wording standard.

## Project Structure

```text
crates/medulla-core/ontology/                      existing: supply_chain-v1.ttl, infrastructure-v1.ttl, commercial-v1.ttl and shapes
crates/medulla-virtual-graph/src/                  existing: source_selection.rs, supply_chain_impact.rs
crates/medulla-source-types/src/                   existing: supply_chain_report.rs; new: cross_source_report.rs
crates/medulla-identity-types/src/                 existing: authority_audit.rs (NotUsedReason, AuditSourceMode)
crates/medulla-h-sparql/src/                       existing: foreign_source_handlers.rs; new: supply_chain_impact.rs, cross_source_question.rs
crates/medulla-federation/src/federation_opt/      existing: run.rs, trace.rs; new: same_as_join.rs
crates/medulla-protocol-types/src/protocol/method/ existing: method_10.rs gains two variants
crates/medulla-capabilities/src/domains/query.rs   existing: policy rows for the two methods
crates/medulla-rdf/tests/                          existing: xsrc_supply_chain_query.rs and acceptance; new seed fixtures
tests/federated/                                   new: served tests and recorded responses
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns that block planning; no research.md. Which connected sources exist on the day of the live run is unknown from this repository; that run belongs to Arbora (`ARBORA-FLEET-R052.2`).

## Phase 1: Design

The rules below bind every row.

1. **Reuse, never a second planner.** Source selection is `select_sources_with`; the served consumer is the handler family of `MEDULLA-SRC-AUTH-R004.6.1`.
2. **Authority is data.** A `vg:SourceAuthority` links a `vg:SourceConnection` to an ontology class by `vg:authoritativeSource` and `vg:authorityFor`, with `vg:authorityRank` and an optional `vg:allowFallback`. Subclass entailment lets a superclass declaration serve its subclasses. It is effective only while its mapping is approved. The vocabulary is owned by `specs/source-authority`.
3. **Class-only queries.** A question names ontology classes; a test fails if the text contains a connector name.
4. **Typed abstention, never a silent gap.** An unconnected rank-1 source is `Unconnected`; a fallback needs `vg:allowFallback`.
5. **Planned means planned.** `Method::SourceCoverage` (`MEDULLA-SRC-AUTH-R004.6.1`) and the two new methods are cited as planned until their rows land.

### Existing system and reuse

Found in `crates/medulla-core/ontology/`: `vg:SourceConnection`, `vg:MetadataContract`, `vg:VirtualMapping` with `vg:mapsOntologyClass` and `vg:approved` (`virtual_graph-v1.ttl`, shapes in `virtual_graph-v1.shapes.ttl`); `:Service`, `:Host`, `:ContainerImage`, `:Incident`, `:runsOn`, `:affectsEntity` and the host reading terms (`infrastructure-v1.ttl`); `:Supplier`, `:SupplyChainItem`, `:SoftwareComponent`, `:ComponentVersion`, `:Vulnerability`, `:usesComponent`, `:suppliesService` (`supply_chain-v1.ttl`); `:CustomerContract` and `:coversService` (`commercial-v1.ttl`); `:BusinessCapability` and `kg:Change` (`archimate-v1.ttl`). Already built and cited, not rebuilt: ontology-driven source selection (`MEDULLA-FEDERATED-QUERY-R072`), operation-bound API and MCP sources (`MEDULLA-FEDERATED-QUERY-R073`), SQL pushdown (`R046`, `R069`), R2RML virtual graphs (`MEDULLA-UNIFIED-DATA-PLANE-R005`, `R006`), the approval-gated mapping vocabulary (`R037`), entity-resolution proposals (`R030`) and freshness (`R038.1`).

### Architecture

The authority declaration is data in the graph. `select_sources_with` reads it and ranks mappings; the federated planner executes only the chosen mappings; the report is assembled from the rows and the premises.

Concepts and sources (judgement where noted; seed rows, not engine behavior):

| Concept | Rank 1 | Rank 2 | Basis |
|---|---|---|---|
| Supplier, purchase item | erpnext-mcp | leanix-mcp | transactional procurement over catalogue (judgement) |
| Third-party risk | ciso-assistant-mcp | onetrust-mcp | judgement |
| Customer account, contract | salesforce-mcp | twenty-mcp | judgement |
| Container image | dockerhub-mcp | github-mcp | registry says what an image is (judgement) |
| Vulnerability | gitlab-mcp | github-mcp | judgement |
| Service health | uptime-mcp | lgtm-mcp | judgement |
| Service identity | servicenow-mcp (CMDB) | leanix-mcp | judgement |
| Deployed workload, placement | container-manager-mcp | portainer-mcp | judgement |
| Host hardware | systems-manager-mcp | caddy-mcp / technitium-dns-mcp (ingress and names only) | judgement |
| Business capability | leanix-mcp | archimate-mcp | judgement |
| Incident, change | servicenow-mcp | erpnext-mcp | judgement |

### Interfaces and data model

Types and ontology additions are in [data-model.md](data-model.md). Linking relations: `:suppliesItem` (supplier to procurement item), `:usesComponent` (service to component version), `:publishedBy`, and `:suppliesService` as the chain supplier to service. Entity resolution reuses `MEDULLA-UNIFIED-DATA-PLANE-R030`: proposals are inactive until approved, and a join never crosses an unapproved link. Fresh-data decision: no migration; new terms only.

Report: `SupplyChainImpactReport { service_impacts[{service, items[{supplier, component_version, vulnerability?, impact_kind, source}]}], sources_used[{source, class_served, authority_premise}], abstentions[...] }`.

### Live integration path

Question, then the Arbora intent tool (Arbora repository), then the Medulla handler reading authority rows through `select_sources_with`, then the federated planner over approved virtual mappings and operation-bound connectors, then report assembly. Capability coverage (`MEDULLA-DECISION-ENGINE-R126`) answers which components can serve a task; this spec answers which data source is authoritative for a concept.

### Risks, alternatives, and decisions

- Judgement: the source rankings above are proposed, not discovered; the ontology makes them changeable without code.
- Risk: connector operation contracts for erpnext, uptime and container-manager may lag (cross-repo; `MEDULLA-CQ-XSRC-R002.7`, `R003.4`, `R003.5`).
- Alternative rejected: naming connectors in the query. It fails the exit gate (`MEDULLA-CQ-XSRC-R007.4`).
- Entity resolution quality for vendor names is unproven; approval gating bounds the risk.

**D3. Canonical change vocabulary: reuse `kg:Change`, no commercial `kg:ChangeRequest`.** `kg:Change` in `archimate-v1.ttl` is the ServiceNow change request and is a subclass of `kg:BusinessProcess`. The `service_operations-v1.ttl` module keeps its distinct `kg:ChangeRequest` for IT4IT `:touches` semantics. `MEDULLA-SUPPLY-VOCAB-R003.6` records the cross-spec decision for `MEDULLA-CQ-XSRC-R008.3.1`.

**D5. Shared supplier and item relation: `:suppliesItem`, no `:suppliedBy` redeclaration.** `kg:suppliesItem` links `kg:Supplier` to `kg:SupplyChainItem`. Existing `kg:suppliedBy` declarations remain source-owned, including the ERPNext purchase-order relation (`MEDULLA-CQ-XSRC-R002.1`).

**D6. Service-to-capability relation: `kg:supportsCapability`.** `kg:supports` (`core-foundation-v1.ttl`) is the evidence-to-claim link, not a service-to-capability relation, and no archimate relation carries service to capability. The commercial shapes target `kg:supportsCapability` (Service to BusinessCapability), so `MEDULLA-CQ-XSRC-R008.2.3` uses it.

### Agent memory acceptance question

`MEDULLA-CQ-XSRC-R009` is the acceptance question for the agent-memory substrate. It composes `MEDULLA-FEDERATED-QUERY-R720` stages with `MEDULLA-DECISION-ENGINE-R700.7` contradiction records. Arbora serves the program through `ARBORA-OPS-R018`; `MEDULLA-DURABLE-KERNEL-R042.2` measures its latency.

### North star

One engine, medulla, holds all facts. There is no second database and no sync. External stores are virtual-graph sources reached through connectors. The exit gate is competency questions: the ontology answers how to execute a task and produces cross-source reports from connector virtual graphs.

### Delivery phase

Rows `MEDULLA-CQ-XSRC-R012.1` and `MEDULLA-CQ-XSRC-R012.2` serve P5. They supply the engine side of `ARBORA-ACCEPTANCE-R006.7`.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row, producers first: vocabulary and seed rows, then linking, planning, the report model, the served methods and the acceptance rows. Rows that edit the same file run in sequence; rows of different crates run in parallel. The served-handler rows depend on `MEDULLA-SRC-AUTH-R004.6.1`.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Every ontology module that a row changes still loads through `GraphSchemaSources::default()` and passes its shapes check.
- A public item added by a row has a non-test caller under a handler or server crate (`MEDULLA-CONTRACT-R055`).
- Served tests use recorded fixtures and need no live credential or network.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- `Method::UnifiedQuery` to `route_query_gateway` (`crates/medulla-server-graph/src/dispatch/graph_pipeline/gateway.rs`) to `medulla_h_query::query::try_handle` to `dispatch_sql_query` (`crates/medulla-h-query/src/query/dispatch.rs:127`) to `handle_unified_query` (`crates/medulla-h-query/src/query/sql_read.rs:309`) to `foreign_scan` (`crates/medulla-federation/src/federation_opt/run.rs:26`), which reads the approved virtual mappings.
- `Method::Uql` to `dispatch_sql_query` to `handle_uql` (`crates/medulla-h-query/src/query/uql_statement.rs:17`), the entry of the agent-claims program.
- `Method::SourceCoverage` (planned, `MEDULLA-SRC-AUTH-R004.6.1`) to `crates/medulla-server-graph/src/dispatch/graph_pipeline/pipeline.rs` to `medulla_h_sparql::foreign_source_handlers::try_handle` to `source_coverage.rs::handle` (planned) to `select_sources_with` (`crates/medulla-virtual-graph/src/source_selection.rs:596`), the first production caller of source selection.
- `Method::SupplyChainImpact` and `Method::CrossSourceQuestion` (new) to the same `try_handle` to `supply_chain_impact.rs::handle` and `cross_source_question.rs::handle` (new) to `select_sources_with` and the report assemblers of `crates/medulla-source-types/src/`.
- `GraphSchema.AttachApproved` (`crates/medulla-h-admin/src/graph_schema/approved.rs`) attaches the seed rows under `Operator` origin.
- Obsolete paths to delete: none; every row adds to an existing path.

## Cross-repo seams

- Served supply-chain impact method: `MEDULLA-CQ-XSRC-R011` provides, `ARBORA-FLEET-R052.1` consumes. Contract: [supply-chain-impact-method.md](contracts/supply-chain-impact-method.md).
- Connected-source list for the live run: `ARBORA-FLEET-R052.2` provides, `MEDULLA-CQ-XSRC-R007.2` consumes. Contract: [connected-source-list.md](contracts/connected-source-list.md).
- Connector operation contracts for the mapping fixtures: `CONN-UPTIME-KUMA-AGENT-CONNECTOR-CONTENT-R003`, `CONN-ERPNEXT-AGENT-CONNECTOR-CONTENT-R002` and `CONN-CONTAINER-MANAGER-MCP-CONNECTOR-CONTENT-R003` provide, `MEDULLA-CQ-XSRC-R003.4` consumes. Contract: [connector-operation-contracts.md](contracts/connector-operation-contracts.md).
- UQL operation behind the agent-claims acceptance: `ARBORA-OPS-R018` provides, `MEDULLA-CQ-XSRC-R009` consumes. Contract: [arbora-uql-operation.md](contracts/arbora-uql-operation.md).
