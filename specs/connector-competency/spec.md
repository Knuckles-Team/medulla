# MEDULLA-CQ-CONN — Connector competency questions

**Owner:** Medulla (native Rust) · **State:** proposed

## Outcome and boundary

Exit gate: the deployed engine can reach rich, related data in every live connector through that connector's declared access contract, tracked per connector. For each of the 66 live-deployed connectors this spec states the use cases the ontology must answer: one single-source question, one cross-source question linking at least one other live connector, and an acceptance test.

A competency question is answered when one query over the ontology returns a correct, provenance-carrying answer from real connected data, or a typed abstention naming the unconnected source. Questions, vocabulary assignment and the coverage table are in [plan.md](plan.md); rows are in [requirements.md](requirements.md). Cross-source composition is not new functionality: it is delivered by `specs/federated-query-and-reasoning` and `specs/unified-data-plane`.

## User Scenarios & Testing

### User Story 1 - Ask one connector a question and get a provenance-carrying answer (Priority: P1)

The user is an operator or an agent that asks a domain question about one live connector. The outcome is a correct answer fetched from the source at query time, so that the operator can trust the connector as a data source.

**Acceptance scenarios**

1. **Given** the connector pack is imported, its mapping is approved and the source answers, **When** the served `Uql` statement of the case runs, **Then** the rows carry source, record id and as-of time and the columns match the case.
2. **Given** the pack is not imported, **When** the same statement runs, **Then** the engine refuses with `SOURCE_REGISTRATION_NOT_FOUND` and returns no row.

Independent test: the table-driven served test of `MEDULLA-CQ-CONN-R067.9` runs the `.q1` case of the connector.

### User Story 2 - Ask a question that spans two connectors (Priority: P1)

The user is an operator who needs an answer that no single connector holds. The outcome is one federated answer whose rows name each source, or a typed abstention that names the unconnected source.

**Acceptance scenarios**

1. **Given** both packs are imported and the `owl:sameAs` links are approved, **When** the cross-source statement runs, **Then** the rows join through the approved links and each row cites every source it used.
2. **Given** one source is not connected, **When** the statement runs, **Then** the answer abstains with reason `Unconnected` naming that source (planned `Method::SourceCoverage` of `MEDULLA-SRC-AUTH-R004.6.1`) and never returns an empty success.

Independent test: the `.q2` case of the connector in `MEDULLA-CQ-CONN-R067.9` and the abstention variants of `MEDULLA-CQ-CONN-R067.10`.

### User Story 3 - Reach every contract kind through one served harness (Priority: P1)

The user is a maintainer who proves a connector. The outcome is one table-driven served test that covers MCP, A2A, API, GraphQL and virtual-graph contracts, with declared materialisation only.

**Acceptance scenarios**

1. **Given** a case file and a `FixtureSource` of each contract kind, **When** the served test runs the case, **Then** the engine fetches at query time and stores no source row.
2. **Given** a case that declares materialisation, **When** it runs, **Then** the row states its reason and the engine refuses an undeclared copy.

Independent test: `MEDULLA-CQ-CONN-R067.9`.

### User Story 4 - Answer from a vocabulary that the ontology declares (Priority: P2)

The user is an ontology owner. The outcome is that every term a question reads is declared in a registered module, so that no question depends on a filename guess.

**Acceptance scenarios**

1. **Given** a registered module, **When** the engine loads `GraphSchemaSources::default()`, **Then** each term of the rollups `MEDULLA-CQ-CONN-R068` to `MEDULLA-CQ-CONN-R082` resolves to its full IRI with its domain and range.
2. **Given** a term that no module declares, **When** a question reads it, **Then** the term is reused from an open vocabulary or minted in the owning core module, and the decision is listed in the data model.

Independent test: `MEDULLA-CQ-CONN-R089` runs the served vocabulary coverage test.

### User Story 5 - Onboard a connector and read it with the ontology (Priority: P2)

The user is an operator who connects a new connector. The outcome is that a parent-ontology question returns native rows and live connector rows with the digest, revision and fetch time.

**Acceptance scenarios**

1. **Given** a fixture connector over MCP, **When** the fleet handshake onboards it and the pack imports, **Then** a parent-term question returns native and live rows.
2. **Given** an orphaned ontology or a held source, **When** the question runs, **Then** the answer abstains and names the source.

Independent test: `MEDULLA-CQ-CONN-R085.6` and `MEDULLA-CQ-CONN-R090`.

### Edge Cases

- A source that is down returns `SOURCE_UNAVAILABLE` and never an empty success (`MEDULLA-FEDERATED-QUERY-R722.4`).
- A held mapping returns `SOURCE_HELD`; an unapproved `owl:sameAs` link adds no join.
- A query with a write stage refuses before any source is called (`MEDULLA-CQ-CONN-R090`).
- A caller without access to a source adds no rows from it and sees it in the coverage block (`MEDULLA-SRC-AUTH-R006.1`).
- A term that no module declares is reused from an open vocabulary when an equivalent exists, otherwise minted in the owning core module with a mapping to the open term and a core superclass (see [data-model.md](data-model.md)).

### Assumptions

- Questions are recorded fixtures; no live credentials or network enter the pull-request gate.
- The connector repositories supply the access contract and the mapping of each connector; this spec supplies the question, the ontology terms and the proof.
- A connector whose vocabulary is not yet declared keeps its question rows blocked until its terms land in a registered module.
- Cross-source joins use only approved `owl:sameAs` links and the identity-resolution contract of `specs/federated-query-and-reasoning`.

## Requirements

- **FR-001**: One single-source question per live connector. Rows: `MEDULLA-CQ-CONN-R001` to `MEDULLA-CQ-CONN-R066`, each `.1` child.
- **FR-002**: One cross-source question per live connector. Rows: `MEDULLA-CQ-CONN-R001` to `MEDULLA-CQ-CONN-R066`, each `.2` child.
- **FR-003**: Every contract kind answers at query time through one served harness. Rows: `MEDULLA-CQ-CONN-R067`, `MEDULLA-CQ-CONN-R067.9`, `MEDULLA-CQ-CONN-R067.10`.
- **FR-004**: Declared materialisation only; never implicit. Rows: `MEDULLA-CQ-CONN-R067.6`.
- **FR-005**: Every question term is declared in a registered module. Rows: `MEDULLA-CQ-CONN-R068` to `MEDULLA-CQ-CONN-R082`, `MEDULLA-CQ-CONN-R087`, `MEDULLA-CQ-CONN-R089`.
- **FR-006**: Connector-pack questions are answered on the hot path. Rows: `MEDULLA-CQ-CONN-R084`, `MEDULLA-CQ-CONN-R084.6`.
- **FR-007**: An onboarded connector answers a parent-ontology question from live data. Rows: `MEDULLA-CQ-CONN-R085`, `MEDULLA-CQ-CONN-R085.6`.
- **FR-008**: One read-only federated query spans two virtual graphs and engine facts. Rows: `MEDULLA-CQ-CONN-R090`.

## Success Criteria

- **SC-001**: Every answer row names its source, record id and as-of time. Rows: `MEDULLA-CQ-CONN-R067.9`.
- **SC-002**: A missing pack or an unconnected source yields a typed refusal or abstention naming it, in 100 percent of the abstention cases. Rows: `MEDULLA-CQ-CONN-R067.10`.
- **SC-003**: No source row is stored unless the case declares its materialisation. Rows: `MEDULLA-CQ-CONN-R067.6`.
- **SC-004**: Each term that a question reads resolves to a full IRI in a registered module. Rows: `MEDULLA-CQ-CONN-R089`.
- **SC-005**: An onboarded fixture connector answers a cross-ontology live question with digest, revision and fetch time. Rows: `MEDULLA-CQ-CONN-R085.6`.

## Requirements and acceptance

Rows `MEDULLA-CQ-CONN-R001` to `MEDULLA-CQ-CONN-R066` are rollups, one per connector; each has a `.1` single-source question and a `.2` cross-source question. The `.3` acceptance rows are retired into the served harness `MEDULLA-CQ-CONN-R067.9`. Cross-repo prerequisites: the connector access contracts and mappings (owning repos: the connector repositories) and the Arbora federated-read surface; see the seams in [plan.md](plan.md).

## Traceability

See [test-spec.md](test-spec.md).

## Virtual-first: the engine reaches source data, it does not copy it

What is ingested from a connected source is its **ontology and access contract**. The data stays in the source. The engine fetches it at query time through an ontology-driven virtual graph, selected by the ontology's source declarations. The contract kinds are A2A, MCP, API (REST), GraphQL and virtual-graph contracts over relational or document stores. A question is answered by federating over those contracts. Copying a source into the engine is an optimisation that needs its own stated reason (latency, availability, history). It is never the default and never a precondition for answering.
