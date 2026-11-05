# MEDULLA-CQ-XSRC — Cross-source competency: supply chain to services

**Owner:** Medulla (native Rust) · Cross-repo IDs: Arbora (served intent tool, connected-source list, UQL operation), agent-packages connectors (operation contracts for erpnext, uptime and container-manager).

## Outcome and boundary

This is the program exit gate. An operator asks one question and gets one report that joins supply-chain data with service data. The ontology, not the query, says which connected source is authoritative for each concept. This is the virtual-graph capability: SPARQL, OWL, RDF and SHACL over virtual graphs on connectors.

Exit-gate question, verbatim: **"How does the supply chain affect our services?"**

Generalization questions, verbatim:

- "Which services are exposed by a vulnerability in an image we deploy?"
- "Which business capabilities depend on a host that is over its temperature or capacity limit?"
- "Which open incidents or changes touch a service a customer contract depends on?"

A question is **answered** when one query over the ontology returns a correct, provenance-carrying answer from real connected data. When a source the ontology names is not connected, the answer is a typed abstention that names the source and the concept, never a silent gap or a fallback.

Owned here: the authority seed rows, the linking relations, the report model and the acceptance tests. Not owned here: the authority vocabulary (`specs/source-authority`), the supply-chain and commercial terms and shapes (`specs/supply-chain-vocabulary`), the federated planner, pushdown and virtual-graph execution (`specs/federated-query-and-reasoning`), entity-resolution scoring (`specs/unified-data-plane`) and decision-engine routing (`specs/decision-engine`). The served intent tool belongs to Arbora; connector operation contracts belong to agent-packages.

## User Scenarios & Testing

### User Story 1 - Ask how the supply chain affects our services (Priority: P1)

The user is an operator. The outcome is one report with the impact per service, the items behind each impact, the sources used and why, and a typed abstention for every source that is not connected, so that the operator can act on real data and see what is missing.

**Acceptance scenarios**

1. **Given** recorded responses of the procurement, registry, code-host, container and uptime connectors and approved authority rows, **When** the `SupplyChainImpact` method runs, **Then** the report holds at least one service impact with a supplier, a component version and a source per item, and every source it names appears in an authority row.
2. **Given** the procurement source is not connected, **When** the same question runs, **Then** the report keeps the software-side rows unchanged and carries `Unconnected` naming the source and `:Supplier`.

Independent test: `MEDULLA-CQ-XSRC-R007.1` and `MEDULLA-CQ-XSRC-R007.3`.

### User Story 2 - Change the answer by changing the ontology only (Priority: P1)

The user is an ontology owner. The outcome is that one edit of an authority row moves a class to another source while the query text stays byte-identical, so that no code change is needed to re-point a concept.

**Acceptance scenarios**

1. **Given** two served runs of the same query text, **When** one `vg:SourceAuthority` row is changed between them through `GraphSchema.AttachApproved`, **Then** the plan and the report name the new source.
2. **Given** a rank-1 source that is not connected and an authority row without `vg:allowFallback`, **When** the question runs, **Then** the answer abstains by name and never falls back.

Independent test: `MEDULLA-CQ-XSRC-R007.4` and `MEDULLA-CQ-XSRC-R005.4`.

### User Story 3 - Ask three more cross-source questions of the same shape (Priority: P2)

The user is an operator. The outcome is that the three generalization questions answer through one served method with the same provenance and abstention rules.

**Acceptance scenarios**

1. **Given** recorded registry, code-host, deployment and uptime responses, **When** `Method::CrossSourceQuestion` runs with `ExposedServices`, **Then** one item per exposed service holds the image, the vulnerability identifier and a per-item source.
2. **Given** the code-host source is not connected, **When** the same case runs, **Then** `abstentions` holds `Unconnected` for `:Vulnerability` and `items` is empty.

Independent test: `MEDULLA-CQ-XSRC-R008.4` and the table-driven cases of `MEDULLA-CQ-XSRC-R008.1.2`, `R008.2.4` and `R008.3.4`.

### User Story 4 - Resolve records across sources only through an approved link (Priority: P2)

The user is an operator. The outcome is that a vendor or service known in several sources becomes one node only after an approver is recorded, so that a join never crosses a guess.

**Acceptance scenarios**

1. **Given** three records of one service in three sources, **When** no link is approved, **Then** the join returns no row and lists the candidate as unresolved.
2. **Given** the link is approved, **When** the join runs, **Then** the rows join across the approved `owl:sameAs`.

Independent test: `MEDULLA-CQ-XSRC-R004.5`.

### User Story 5 - Ask which high-confidence agent claims touch a new external report (Priority: P2)

The user is an operator reviewing agent memory. The outcome is one UQL program that selects the claims, clusters their evidence and decays the contradictions by age, served through Arbora.

**Acceptance scenarios**

1. **Given** a report and agent claims ingested through the served surface, **When** the fixture runs under a tenant and principal, **Then** the claims, clusters and decayed weights match the fixed expectations and a second tenant sees no rows.
2. **Given** an unknown report id, **When** the fixture runs, **Then** it refuses with `CQ_REPORT_NOT_FOUND` and returns no rows.

Independent test: `MEDULLA-CQ-XSRC-R009.4` and `MEDULLA-CQ-XSRC-R009.5`.

### User Story 6 - Prove cross-connector answers through the served door (Priority: P2)

The user is the acceptance owner of the final phase scenario. The outcome is a joined answer from two virtual-graph connectors in which every fact names its source, mapping and read time, and in which an unreadable source is named in coverage.

**Acceptance scenarios**

1. **Given** two fixture connectors imported through `ConnectorPack.Import` and an approved link, **When** the served method runs, **Then** the answer joins both and every fact carries a source, a mapping and a read time.
2. **Given** the caller's role cannot read one source, **When** the method runs, **Then** no fact of that source appears and the coverage block lists it with reason `RoleInsufficient`.

Independent test: `MEDULLA-CQ-XSRC-R012.1` and `MEDULLA-CQ-XSRC-R012.2`.

### Edge Cases

- A rank-1 source that is not connected abstains with `Unconnected`; a lower-ranked alternate is used only when the row sets `vg:allowFallback`.
- An authority row on an unapproved mapping is not effective and the class is reported uncovered.
- Equal-rank authority rows are an unresolvable tie and are listed, never chosen by name.
- An unapproved `owl:sameAs` candidate adds no row to a join and is listed as unresolved.
- A report item with no source is refused; a copy with no watermark is served live.
- A query text that contains a connector name fails the class-only check.
- A contract with no curated `:coversService` edge makes the report abstain with `Uncovered`.

### Assumptions

- Questions run over recorded connector responses in the pull-request gate; no live credential or network enters it. The live run belongs to Arbora (`ARBORA-FLEET-R052.2`).
- The source rankings in `plan.md` are proposed judgements, not discovered facts; an operator changes them as ontology data.
- Authority is declared per class, with an optional facet per property, as the source-authority vocabulary defines it.
- Service to business capability uses `kg:supportsCapability` (decision D6 in `plan.md`).
- The decision, planner and entity-resolution mechanisms are cited, not rebuilt here.

## Requirements

- **FR-001**: The ontology declares, per concept, the authoritative connected source, with seed rows for the supply-chain and service concepts. Rows: `MEDULLA-CQ-XSRC-R002.5`, `MEDULLA-CQ-XSRC-R002.6`, `MEDULLA-CQ-XSRC-R003.1`, `MEDULLA-CQ-XSRC-R003.2`, `MEDULLA-CQ-XSRC-R003.3`.
- **FR-002**: Supply-chain and service concepts exist and are bound to sources through approved virtual mappings. Rows: `MEDULLA-CQ-XSRC-R002.1`, `MEDULLA-CQ-XSRC-R002.7`, `MEDULLA-CQ-XSRC-R003.4`, `MEDULLA-CQ-XSRC-R003.5`.
- **FR-003**: A relation links a supply-chain entity to a service, and records in different sources resolve to one node only through an approved link. Rows: `MEDULLA-CQ-XSRC-R004.1`, `MEDULLA-CQ-XSRC-R004.3`, `MEDULLA-CQ-XSRC-R004.4`, `MEDULLA-CQ-XSRC-R004.5`.
- **FR-004**: The query names classes only and the planner derives the sources from the authority declarations. Rows: `MEDULLA-CQ-XSRC-R005.1`, `MEDULLA-CQ-XSRC-R005.2`, `MEDULLA-CQ-XSRC-R005.3`, `MEDULLA-CQ-XSRC-R005.4`, `MEDULLA-CQ-XSRC-R005.5`.
- **FR-005**: The report gives impact per service, the items behind it, sources used with the reason, freshness and typed abstention. Rows: `MEDULLA-CQ-XSRC-R006.1`, `MEDULLA-CQ-XSRC-R006.2`, `MEDULLA-CQ-XSRC-R006.3`, `MEDULLA-CQ-XSRC-R006.4`, `MEDULLA-CQ-XSRC-R006.5`.
- **FR-006**: The exit-gate question is answered end to end, and changing only an authority declaration changes the sources used. Rows: `MEDULLA-CQ-XSRC-R007.1`, `MEDULLA-CQ-XSRC-R007.2`, `MEDULLA-CQ-XSRC-R007.3`, `MEDULLA-CQ-XSRC-R007.4`.
- **FR-007**: Three further questions reuse the same mechanism through one served method. Rows: `MEDULLA-CQ-XSRC-R008.1.1`, `MEDULLA-CQ-XSRC-R008.1.2`, `MEDULLA-CQ-XSRC-R008.2.1`, `MEDULLA-CQ-XSRC-R008.2.3`, `MEDULLA-CQ-XSRC-R008.2.4`, `MEDULLA-CQ-XSRC-R008.3.1`, `MEDULLA-CQ-XSRC-R008.3.3`, `MEDULLA-CQ-XSRC-R008.3.4`, `MEDULLA-CQ-XSRC-R008.4`.
- **FR-008**: One UQL program answers the agent-claims question as one plan. Rows: `MEDULLA-CQ-XSRC-R009.1`, `MEDULLA-CQ-XSRC-R009.2`, `MEDULLA-CQ-XSRC-R009.3`, `MEDULLA-CQ-XSRC-R009.4`, `MEDULLA-CQ-XSRC-R009.5`.
- **FR-009**: The impact report is served as a Medulla method and proven across connectors. Rows: `MEDULLA-CQ-XSRC-R011`, `MEDULLA-CQ-XSRC-R012.1`, `MEDULLA-CQ-XSRC-R012.2`.

## Success Criteria

- **SC-001**: The test of `MEDULLA-CQ-XSRC-R007.4` shows byte-identical query text and different sources after an ontology-only change.
- **SC-002**: Every report item names its source; a source-less item is refused in 100 percent of cases (`MEDULLA-CQ-XSRC-R006.1`).
- **SC-003**: Every unconnected authoritative source appears as a typed abstention naming the source and class (`MEDULLA-CQ-XSRC-R007.3`).
- **SC-004**: A join crosses only an approved link (`MEDULLA-CQ-XSRC-R004.5`).
- **SC-005**: Every fact of a cross-connector answer names its source, mapping and read time (`MEDULLA-CQ-XSRC-R012.1`).

## Traceability

See [test-spec.md](test-spec.md). The design is in [plan.md](plan.md); entities are in [data-model.md](data-model.md); the shortest proof is in [quickstart.md](quickstart.md).

## Virtual-first: the engine reaches source data, it does not copy it

What is ingested from a connected source is its **ontology and access contract**. The data stays in the source. The engine fetches it at query time through an ontology-driven virtual graph, selected by the ontology's source declarations. The contract kinds are A2A, MCP, API (REST), GraphQL and virtual-graph contracts over relational or document stores. A question is answered by federating over those contracts. Copying a source into the engine is an optimisation that needs its own stated reason (latency, availability, history). It is never the default and never a precondition for answering.
