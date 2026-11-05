# MEDULLA-SUPPLY-VOCAB — Supply-chain and service-impact vocabulary

Owner: Medulla. Cross-repo IDs: agent-packages connectors (`erpnext-agent`, `uptime-kuma-agent`, `container-manager-mcp`, `salesforce-agent`, `fan-manager`: class resolution and access contracts), `MEDULLA-CQ-XSRC-001` (consumer), `MEDULLA-UNIFIED-DATA-PLANE-R030` and `MEDULLA-FEDERATED-QUERY-R072` (mechanism).

## Purpose

`MEDULLA-CQ-XSRC-001` answers "How does the supply chain affect our services?" and needs a shared ontology vocabulary. This spec defines that vocabulary end to end: each term has an owner file, a domain and range, a SHACL shape, a source that can supply it and a blocking key for entity resolution.

## User Scenarios & Testing

### User Story 1 - Follow the physical chain from supplier to service (Priority: P1)

The user is an operator. The outcome is a query that names supplier, item, asset, host and service as classes and reaches a service through the declared chain.

**Acceptance scenarios**

1. **Given** a supplier, an item, an asset realizing it, a host of the asset and a typed service running on the host, **When** the reasoner runs, **Then** `kg:suppliesService` links the supplier to the service.
2. **Given** a container-only or pod-only `runsOn` path, **When** the reasoner runs, **Then** no supplier-to-service edge is inferred and no endpoint is retyped as a service.

Independent test: the bound tests of `MEDULLA-SUPPLY-VOCAB-R001.3` run this story through the native reasoner.

### User Story 2 - Follow the software chain from component to service (Priority: P1)

The user is a security reviewer. The outcome is the path from a vulnerable component version through image and workload to the service that uses it.

**Acceptance scenarios**

1. **Given** a service deployed as a workload that uses an image containing a component version, **When** the reasoner runs, **Then** `kg:usesComponent` links the service to the version.
2. **Given** a vulnerability record with a severity outside the closed set, **When** the shapes validate it, **Then** the record is refused.

Independent test: the bound tests of `MEDULLA-SUPPLY-VOCAB-R002.5` and `MEDULLA-SUPPLY-VOCAB-R002.7` run this story.

### User Story 3 - Relate customers, contracts and affected services (Priority: P1)

The user is a customer-facing owner. The outcome is that a contract, its customer and the services it covers, plus the incident or change touching a service, are expressible without duplicating `:Change` or `:Incident`.

**Acceptance scenarios**

1. **Given** a contract with one customer and one covered service, **When** the shapes validate it, **Then** it passes; a contract with no customer or no service is refused.
2. **Given** a service-to-capability link, **When** it uses `kg:supportsCapability` from a service, **Then** it is accepted; a link from a non-service is refused.

Independent test: the bound tests of `MEDULLA-SUPPLY-VOCAB-R003.5` run this story through the shapes.

### User Story 4 - Compare a host reading with its limit (Priority: P2)

The user is an operator. The outcome is a normalized `infra:HostLimit` row, with "over limit" a query condition and never a stored fact.

**Acceptance scenarios**

1. **Given** a host limit with a reading, a limit, a unit and a kind in the closed set, **When** the shape validates it, **Then** it passes.
2. **Given** a limit with a reading but no limit value or an unknown kind, **When** the shape validates it, **Then** it is refused.

Independent test: the bound tests of `MEDULLA-SUPPLY-VOCAB-R004.2` run this story.

### User Story 5 - Choose sources from declarations and join only on approved links (Priority: P2)

The user is the engine answering a class-only question. The outcome is that each concept has a source or a typed abstention, and entities from different sources join only across an approved link.

**Acceptance scenarios**

1. **Given** the recorded responses of the source table, **When** the coverage test runs, **Then** every concept has an authority row or a typed abstention reason.
2. **Given** a name-only match between two sources, **When** the join runs, **Then** the match is a proposal and joins nothing until approved.

Independent test: the bound tests of `MEDULLA-SUPPLY-VOCAB-R008.3` and `MEDULLA-SUPPLY-VOCAB-R007.2` run this story.

### User Story 6 - See which packages a finding reaches (Priority: P2)

The user is a security reviewer. The outcome is every transitive dependent of a package with its path and worst severity.

**Acceptance scenarios**

1. **Given** a chain of five packages with one diamond, **When** a finding on the leaf is propagated through `MineRiskPropagation`, **Then** every dependent is listed with its shortest path and severity.
2. **Given** a package absent from the graph, **When** the finding is propagated, **Then** the typed refusal `ECOSYSTEM_PACKAGE_UNKNOWN` is returned.

Independent test: the bound tests of `MEDULLA-SUPPLY-VOCAB-R009.3` run this story through the served method.

### Edge Cases

- A container-only or pod-only `runsOn` path never infers a supplier-to-service edge.
- A `Vendor` alone does not entail `Supplier`.
- A contract with no covering service edge abstains `Uncovered` naming the contract; it is never an empty success.
- A name-only supplier match across sources is never joined before approval.
- A dependency cycle ends at the depth bound.
- A shared property domain is never narrowed by a module-local declaration.

## Requirements

- **FR-001**: The physical chain is expressible as supplier to item to asset to host to service. Rows: `MEDULLA-SUPPLY-VOCAB-R001`.
- **FR-002**: The software chain is expressible as component version to image to workload to service, with publisher and vulnerability attached. Rows: `MEDULLA-SUPPLY-VOCAB-R002`.
- **FR-003**: Customer, contract, covered service and the incident or change that touches a service are expressible without duplicating `:Change` or `:Incident`. Rows: `MEDULLA-SUPPLY-VOCAB-R003`.
- **FR-004**: Host limits are a normalized row and "over limit" is a query condition. Rows: `MEDULLA-SUPPLY-VOCAB-R004`.
- **FR-005**: Service to business capability uses one declared property, `:supportsCapability`. Rows: `MEDULLA-SUPPLY-VOCAB-R003.4`, `MEDULLA-SUPPLY-VOCAB-R003.5`.
- **FR-006**: Every concept has a recorded source table or a typed abstention. Rows: `MEDULLA-SUPPLY-VOCAB-R005`, `MEDULLA-SUPPLY-VOCAB-R006`, `MEDULLA-SUPPLY-VOCAB-R008`.
- **FR-007**: Entities from different sources join only across an approved `owl:sameAs`; this spec supplies the blocking keys. Rows: `MEDULLA-SUPPLY-VOCAB-R007`.
- **FR-008**: The commercial module, its shapes, namespace and Opportunity date are registered and checked. Rows: `MEDULLA-SUPPLY-VOCAB-R003.13`, `MEDULLA-SUPPLY-VOCAB-R003.14`, `MEDULLA-SUPPLY-VOCAB-R003.15`, `MEDULLA-SUPPLY-VOCAB-R003.16`, `MEDULLA-SUPPLY-VOCAB-R003.17`.
- **FR-009**: A finding on an ecosystem package reaches every transitive dependent with its path. Rows: `MEDULLA-SUPPLY-VOCAB-R009`.

## Success Criteria

- **SC-001**: Each new `.ttl` parses, each shape accepts a valid fixture and refuses each fault, and the class-only impact query parses against the ontology. Rows: `MEDULLA-SUPPLY-VOCAB-R001.4`, `MEDULLA-SUPPLY-VOCAB-R002.7`, `MEDULLA-SUPPLY-VOCAB-R008.1`.
- **SC-002**: A finding on a leaf package lists every transitive dependent within the depth bound. Rows: `MEDULLA-SUPPLY-VOCAB-R009.6`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- Connector payload availability is verified by the connector rows, not inferred from ontology declarations.
- A source gap recorded as blocked in the plan decisions yields a typed abstention, not a guess.

## Scope and interfaces

Owned: new terms and shapes in `supply_chain-v1.ttl`, `commercial-v1.ttl`, `infrastructure-v1.ttl` (host limits), their mappings as `vg:VirtualMapping` fixtures, blocking keys, and the impact query. Not owned: authority declaration (`MEDULLA-CQ-XSRC-R001`), federated planning, the report model (`MEDULLA-CQ-XSRC-R006`), entity-resolution scoring (`R030`), connector internals (agent-packages).

Non-goals: ingesting any source's data; a new decision or planning mechanism; ArchiMate model authoring.

## Traceability

| Requirement | Design section | Test ID | Evidence |
|---|---|---|---|
| FR-001 | plan.md Vocabulary (physical) | T-001 | PENDING |
| FR-002 | plan.md Vocabulary (software) | T-002 | PENDING |
| FR-003 | plan.md Vocabulary (commercial), Decisions D3 | T-003 | PENDING |
| FR-004 | plan.md Vocabulary (host limits) | T-004 | PENDING |
| FR-005 | plan.md Decisions D1 | T-005 | PENDING |
| FR-006 | plan.md Decisions D2, D4 | T-006 | PENDING |
| FR-007 | plan.md Entity resolution | T-007 | PENDING |
| FR-008 | plan.md Commercial acceptance | T-COM-013 through T-COM-017 | PENDING |
| FR-009 | plan.md Ecosystem package risk propagation | T-009 | PENDING |
| SC-001 | plan.md Worked example | T-008 | PENDING |
