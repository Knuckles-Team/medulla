# MEDULLA-UNIFIED-DATA-PLANE — Unified data plane

Component: Unified data plane

| Field | Value |
|---|---|
| Spec ID | `unified-data-plane` |
| Owner | Medulla |
| Scope | Attached-source, dialect, source-category, catalog-analytics, app-admission and freshness obligations; native storage and finance import are adjacent owner contracts |

## Outcome and boundaries

Medulla is the converged engine for Medulla-owned data: property graph, SQL, RDF/OWL, vector, time series, provenance and reasoning over one durable authority and one query surface. Third-party applications keep their real database. Medulla attaches to each through a single owner-scoped registry, serves live SQL and named virtual graphs, optionally maintains a change-captured accelerated copy, and exposes a freshness-aware query route. Native hosting is an app-specific admission track, first measured with Gramps. Immich is an API connector and entity-resolution pilot owned by the connector repository; it is not a native-hosting candidate. Ghostfolio is a one-way finance import from attached Postgres until an explicit retirement decision.

This spec does not authorize a production cutover, direct writes around an application's business logic, automatic `owl:sameAs` assertions, or a performance claim without the stated benchmark gate. P6 Gramps cutover requires the operator's go-ahead after P0–P5 evidence.

Non-goals: no second connector registry, no second phase registry beside the semantic stages, no kernel mirror-sink registry, no pgrx companion hosting, and no shared relational platform stack. Those belong to the service stacks, the connector repositories or the durable kernel, or are retired in `requirements.md`.

## User Scenarios & Testing

### User Story 1 - Attach one source and understand its schema (Priority: P1)

The user is an operator or application owner who attaches a third-party database. The outcome is one owner-scoped source that serves SQL, mapped graphs and schema questions without copying entity rows.

**Acceptance scenarios**

1. **Given** an operator registers a SQL source with a secret reference, **When** the registry verifies the outbound target and reads the catalog, **Then** the source is stored without a secret value, one hashed `AttachedCatalogGraph` exists, and an inline password or an unverified address is refused.
2. **Given** an approved R2RML mapping over the attached catalog, **When** a caller queries the named virtual graph through SPARQL, UQL or REASON, **Then** the answer reads the source live, and an unapproved mapping exposes no data.
3. **Given** a pushable projection, filter, join, aggregate, order or limit, **When** the planner runs the query, **Then** the source executes it and EXPLAIN names every fragment that fell back.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R002.1` run this story through the served registration entry point.

### User Story 2 - Attach any supported dialect with a conformance proof (Priority: P1)

The user is an operator who attaches Postgres, MySQL, MariaDB, SQLite, SQL Server, ClickHouse, Oracle, Db2, a document store or a warehouse. The outcome is a five-part adapter with a versioned conformance entry for each engine version.

**Acceptance scenarios**

1. **Given** a dialect adapter and a recorded or containerized engine, **When** the conformance runner executes the matrix row, **Then** the entry records catalog, type map, pushdown and capture results, and a capability the adapter does not offer is declared `unsupported` and never simulated.
2. **Given** a source with no native change feed, **When** the adapter registers, **Then** the planner sees the capture capability as `unsupported`, and watermark polling or a Debezium bridge feeds `ChangeEnvelope` records only where the row names it.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R022.2` run the matrix.

### User Story 3 - Admit an application and earn native hosting (Priority: P2)

The user is an operator who admits self-hosted applications one at a time. The outcome is an admission stage machine, a reviewed differential record and a Gramps pilot that ends in evidence rather than a cutover claim.

**Acceptance scenarios**

1. **Given** an application is attached, **When** its old ingest connector is proposed for retirement, **Then** the stage machine refuses retirement before result and change parity, and a failed step rolls back.
2. **Given** the Gramps statement corpus, **When** P0 to P5 run, **Then** each phase exit is typed, ordered and reviewed, and P6 stays a separate explicit authorization.
3. **Given** an approved graph projection, **When** an external client writes through a projected table, **Then** the write is refused, and a graph change reaches the projection only through `MutationBatch`.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R026` run the pilot phases.

### User Story 4 - Read every source category live, once consistently (Priority: P2)

The user is an analyst whose question spans a transactional store, a metastore with columnar files, a search index, a graph language and a web metasearch connector. The outcome is one answer per question with a named snapshot, receipts and no stored copy.

**Acceptance scenarios**

1. **Given** a transactional source, **When** a federated read touches it twice, **Then** both fragments read one snapshot and a concurrent writer cannot split the answer.
2. **Given** a registered metastore, **When** the query calls `hive('db.table')`, **Then** partitions and stripes prune by the pushed predicate and the scan receipt counts what was skipped.
3. **Given** a search-index source and a web metasearch source, **When** a fused query runs, **Then** relevance and rank come from the remote, the web result stays a candidate and never a stored record, and a repeated query inside the time-to-live makes no second connector call.
4. **Given** an external engine, **When** it reads the Iceberg REST catalog, **Then** every route passes the carrier gate and a cross-tenant token is refused.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R038.3.6` and `MEDULLA-UNIFIED-DATA-PLANE-R039.5` run this story.

### User Story 5 - Link entities, follow changes and correlate incidents (Priority: P2)

The user is a data steward. The outcome is entity links that exist only after approval, change envelopes that replay idempotently and incidents that group anomalies by signature.

**Acceptance scenarios**

1. **Given** candidate pairs from two applications, **When** blocking, similarity and Fellegi–Sunter scoring run, **Then** no `owl:sameAs` edge exists before approval and one exists after.
2. **Given** a message-stream record, **When** it normalizes into a `ChangeEnvelope`, **Then** a replay applies each change once and a malformed record is refused with a stable code.
3. **Given** 10,000 anomalies over 100 entities, **When** the incident authority correlates them, **Then** each signature yields one incident with its evidence set.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R030.3` and `MEDULLA-UNIFIED-DATA-PLANE-R760.6` run this story.

### User Story 6 - Profile, score, trace and search the catalog (Priority: P3)

The user is a data steward or analyst. The outcome is dataset profiles, quality scores, column lineage, asset usage and ranked catalog search, each with evidence.

**Acceptance scenarios**

1. **Given** an attached dataset, **When** the profile job runs, **Then** each profile records its method and sample size, and an out-of-scope dataset is refused.
2. **Given** a quality rule over a native dataset, **When** it fails three times in a row, **Then** one incident opens through the incident authority.
3. **Given** SQL text that defines a view, **When** the shared parser reads it, **Then** column edges carry the query digest and parser version, and SQL that does not parse is quarantined.
4. **Given** a registered foreign source, **When** a caller lists, probes, shares or unshares it, **Then** visibility follows the caller scope and a revocation takes effect on the next call.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R762.6` and `MEDULLA-UNIFIED-DATA-PLANE-R765` run this story.

### User Story 7 - Delete, act, remember and promote safely (Priority: P3)

The user is an application operator. The outcome is a document delete that leaves no orphan, governed actions, scoped history search and an embedding promotion that one modality regression blocks.

**Acceptance scenarios**

1. **Given** a document with 500 chunks, **When** `DocumentDelete` runs, **Then** the record, vectors and graph nodes go in one batch and the orphan report is empty.
2. **Given** an action whose submission criterion fails, **When** it is submitted, **Then** no effect applies and the outbox stays empty.
3. **Given** a candidate embedding model whose image recall falls below its floor, **When** the binding transitions to `Live`, **Then** the transition is refused and the prior version stays active.

Independent test: the bound tests of `MEDULLA-UNIFIED-DATA-PLANE-R651.6` and `MEDULLA-UNIFIED-DATA-PLANE-R040.6` run this story.

### Edge Cases

- A source with no capture capability stays visible to the planner as `unsupported`; no capability is inferred from a dialect name.
- An accelerated copy never answers a read-your-writes request before its committed source position; the wait times out with a typed error and never returns a stale success.
- A request names a source of another tenant or owner and fails closed before any outbound call.
- A replica lag above the bound refuses the read instead of answering from a stale snapshot.
- A rule or mapping that is proposed but not approved exposes no data and asserts no link.
- A Debezium or stream record with a before image but no operation code is refused and never guessed.
- An unreachable catalog, an unsupported file layout or a schema mismatch returns a stable refusal code and no partial result.
- A web result expires with its time-to-live and is never written as a stored record.

## Requirements

- **FR-001**: Record the attached-source default, native-hosting gate, router consistency and pgrx boundary as architecture decisions. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R001`.
- **FR-002**: Register one tenant- and owner-scoped attached source with secrets by reference and a verified outbound target; one registry serves SQL federation and OBDA. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R002`, `MEDULLA-UNIFIED-DATA-PLANE-R002.1`, `MEDULLA-UNIFIED-DATA-PLANE-R002.2`.
- **FR-003**: Read each dialect catalog into a versioned, typed, hashed schema graph; profile under a bounded policy; infer dependencies and deterministic Leiden groups. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R003`, `MEDULLA-UNIFIED-DATA-PLANE-R004`, `MEDULLA-UNIFIED-DATA-PLANE-R029`.
- **FR-004**: Compile deterministic-first, operator-approved ontology mappings to versioned R2RML named virtual graphs; model output is a proposal only. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R005`.
- **FR-005**: Push typed projection, filter, same-source join, aggregate, order and limit to the source when supported, and show every fallback in EXPLAIN. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R006`, `MEDULLA-UNIFIED-DATA-PLANE-R007`.
- **FR-006**: Normalize change capture to `ChangeEnvelope`, persist the source position per subscription and replay idempotently. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R008`, `MEDULLA-UNIFIED-DATA-PLANE-R009`, `MEDULLA-UNIFIED-DATA-PLANE-R010`, `MEDULLA-UNIFIED-DATA-PLANE-R021`, `MEDULLA-UNIFIED-DATA-PLANE-R710`, `MEDULLA-UNIFIED-DATA-PLANE-R711`.
- **FR-007**: Route native, live or accelerated reads by policy and freshness; read-your-writes waits a bounded time and never returns a stale success. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R011`.
- **FR-008**: Govern attached-source write-back with approval, idempotency and an audit reservation. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R012`.
- **FR-009**: Implement five-part adapters for Postgres, MySQL, MariaDB, SQLite, SQL Server, ClickHouse, Oracle, Db2, document stores and warehouses with a per-engine conformance entry. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R013`, `MEDULLA-UNIFIED-DATA-PLANE-R014`, `MEDULLA-UNIFIED-DATA-PLANE-R015`, `MEDULLA-UNIFIED-DATA-PLANE-R016`, `MEDULLA-UNIFIED-DATA-PLANE-R017`, `MEDULLA-UNIFIED-DATA-PLANE-R018`, `MEDULLA-UNIFIED-DATA-PLANE-R019`, `MEDULLA-UNIFIED-DATA-PLANE-R020`, `MEDULLA-UNIFIED-DATA-PLANE-R022`.
- **FR-010**: Admit applications one at a time and retire an ingest connector only after attached parity. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R024`, `MEDULLA-UNIFIED-DATA-PLANE-R031`.
- **FR-011**: Earn native hosting through the Gramps traffic, differential and restore pilot; expose graph-backed projections read-only. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R026`, `MEDULLA-UNIFIED-DATA-PLANE-R027`, `MEDULLA-UNIFIED-DATA-PLANE-R028`, `MEDULLA-UNIFIED-DATA-PLANE-R026.4`.
- **FR-012**: Resolve entities across applications by blocking, similarity and Fellegi–Sunter proposals with approved links only. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R030`.
- **FR-013**: Ship the virtual-graph vocabulary and the metadata-discovery contract; materialize metadata and hot subsets and read the rest live. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R037`, `MEDULLA-UNIFIED-DATA-PLANE-R038`, `MEDULLA-UNIFIED-DATA-PLANE-R038.1`, `MEDULLA-UNIFIED-DATA-PLANE-R038.2`.
- **FR-014**: Answer transactional, metastore, search-index, graph-language and web metasearch sources in place with receipts. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R038.3`, `MEDULLA-UNIFIED-DATA-PLANE-R038.4`, `MEDULLA-UNIFIED-DATA-PLANE-R038.5`, `MEDULLA-UNIFIED-DATA-PLANE-R038.6`, `MEDULLA-UNIFIED-DATA-PLANE-R038.7`, `MEDULLA-UNIFIED-DATA-PLANE-R022.4`.
- **FR-015**: Serve the Iceberg REST catalog only through the verified carrier gate and emit partition metadata. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R039`, `MEDULLA-UNIFIED-DATA-PLANE-R039.3`, `MEDULLA-UNIFIED-DATA-PLANE-R039.4`, `MEDULLA-UNIFIED-DATA-PLANE-R039.5`.
- **FR-016**: Derive health anomalies and incidents and share the compiled ontology cache with attached-schema graphs. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R035`, `MEDULLA-UNIFIED-DATA-PLANE-R036`, `MEDULLA-UNIFIED-DATA-PLANE-R760`.
- **FR-017**: Profile datasets, score data quality, derive column lineage and rank catalog search with usage. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R761`, `MEDULLA-UNIFIED-DATA-PLANE-R762`, `MEDULLA-UNIFIED-DATA-PLANE-R763`, `MEDULLA-UNIFIED-DATA-PLANE-R764`, `MEDULLA-UNIFIED-DATA-PLANE-R701`, `MEDULLA-UNIFIED-DATA-PLANE-R702`.
- **FR-018**: Serve the foreign source catalog and the tenant context view; accept web research work items. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R765`, `MEDULLA-UNIFIED-DATA-PLANE-R766`, `MEDULLA-UNIFIED-DATA-PLANE-R767`.
- **FR-019**: Cascade document deletes, govern actions, store scoped history and gate embedding promotion per modality. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R651`, `MEDULLA-UNIFIED-DATA-PLANE-R700`, `MEDULLA-UNIFIED-DATA-PLANE-R750`, `MEDULLA-UNIFIED-DATA-PLANE-R040`.

## Success Criteria

- **SC-001**: A source registration with an unverified target, an inline secret or a cross-owner scope leaves no catalog entry. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R002.1`, `MEDULLA-UNIFIED-DATA-PLANE-R002.2`.
- **SC-002**: The catalog hash of one fixed catalog is stable across repeated reads and reordering. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R003.1`.
- **SC-003**: An unapproved mapping or link exposes no data and asserts no edge. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R005.1`, `MEDULLA-UNIFIED-DATA-PLANE-R030.1`.
- **SC-004**: Every dialect and engine version has a green conformance entry for its declared capabilities. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R022.1`, `MEDULLA-UNIFIED-DATA-PLANE-R022.2`.
- **SC-005**: A snapshot replay equals the source snapshot after a crash or failover. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R022.3.2`.
- **SC-006**: A federated read of a transactional source reads one snapshot while a writer runs. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R038.3.6`.
- **SC-007**: A pruned metastore scan reads only the partitions and stripes the predicate keeps. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R038.4.6`.
- **SC-008**: A web query repeated inside its time-to-live makes one connector call. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R038.7.6`.
- **SC-009**: The Iceberg REST catalog refuses an unauthenticated or cross-tenant request on every route. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R039.3`.
- **SC-010**: A promotion blocked on one modality leaves the prior embedding version active. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R040.4`, `MEDULLA-UNIFIED-DATA-PLANE-R040.6`.
- **SC-011**: The incident authority correlates 100,000 anomalies in one pass. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R760.6`.
- **SC-012**: Gramps P0 to P5 produce typed, reviewed phase exits before any production decision. Rows: `MEDULLA-UNIFIED-DATA-PLANE-R026.1`, `MEDULLA-UNIFIED-DATA-PLANE-R026.2.2`, `MEDULLA-UNIFIED-DATA-PLANE-R026.3.2`.

## Assumptions

- Arbora supplies a verified principal, tenant and source grant on every served request.
- A deployment layer supplies the secret-reference resolver and the outbound network policy; Medulla resolves a reference at connection time and verifies the target address.
- The reference host runs the benchmark and conformance suites that measure each budget; a default value in a row is a starting point that a measurement on that host replaces.
- The source database stays the write authority for attached application data.
- `MEDULLA-UNIFIED-DATA-PLANE-R039.1` and `MEDULLA-UNIFIED-DATA-PLANE-R039.2` stay retired: they required types that `MEDULLA-UNIFIED-DATA-PLANE-R039.3` deletes.

## Behavior detail

### Source and capability contract

An attached source is identified by an immutable source ID and scoped to one tenant and owner. Its record contains endpoint identity, a secret reference, dialect/version, supported query and capture capabilities, table grants, mapping versions, capture subscription and cursor, acceleration policy, quotas, and read and write direction. Registration verifies the endpoint against outbound policy before persistence. Per-request DSNs and inline passwords are forbidden. Revocation stops new plans and makes active reads follow a documented drain policy. An adapter may report `unsupported` for capture or pushdown; that state is visible to the planner and conformance harness. No capability is inferred solely from a dialect name.

The source database remains the write authority for attached application data. Medulla's native store is the authority for Medulla-owned data, approved mappings, catalog artifacts, capture cursors and local accelerated copies. A change-captured copy is a derived read path, selected only when its version and freshness meet the request's consistency contract.

### Virtual graphs over every source kind

The operator rule is "materialize metadata and hot subsets, virtualize the rest". Every registered source publishes a `MetadataContract`: schemas, entities, relationships and endpoints. The contract is the connection contract and the metadata-discovery contract in one record. A question reads source rows live through approved `VirtualMapping` records, as a virtual graph. SQL and Iceberg sources use R2RML named virtual graphs (MEDULLA-UNIFIED-DATA-PLANE-R005). API, MCP, A2A and GraphQL sources use operation-bound mappings (MEDULLA-FEDERATED-QUERY-R073). Source selection for a cross-source question belongs to MEDULLA-FEDERATED-QUERY-R072. The Meristem consumer is MERISTEM-CONTROL-R029 and MERISTEM-CONTROL-R030 in [Meristem](https://github.com/Knuckles-Team/agent-utilities/tree/main/specs/agent-control-plane).

### Native-hosting qualification

Postgres wire compatibility is a measured gate, not a blanket promise. Capture an unmodified candidate application's driver startup, migrations and real statement corpus. Test the candidate against real Postgres and Medulla with equal schema/data, comparing rows, column types, ordered results and SQLSTATE. The compatibility inventory includes B-tree indexes, collation, POSIX regex, `bytea`, sequences, savepoints, row/advisory locks, `LISTEN/NOTIFY`, triggers, PL/pgSQL, JSONB operators and indexes, transaction isolation, driver `SET`/`SHOW`, SQLSTATE, schemas/search path, constraints and referential actions, enums/domains/composites, materialized views, large objects, timezone and collation behavior. Implement only gaps proven by captured traffic or a separately approved admission requirement; record each passing or failing case. No app is admitted until its migration tool passes, differential deviations are reviewed, replica lag objective is met, and every backup path has been restored successfully.

The Gramps reference pilot has seven phases. P0 records an export, object counts and content digest. P1 imports that export into a disposable real-Postgres tree, verifies an equal export and captures the full driver statement corpus. P2 replays the corpus against an unmodified Medulla tenant and records each SQLSTATE; no Medulla code change is part of measurement. P3 fixes only proven gaps with a failing test first and requires equal import/export content or reviewed deviations. P4 verifies a real-Postgres replica, a dump and a Medulla backup by independent scratch restores, plus kill-during-import recovery. P5 profiles the catalog and JSON shape, approves R2RML mapping to Person/Family/Event, answers a grandparents query over live tables and exposes `schema_context`. P6 is a separate, explicitly authorized production decision after P0–P5; it requires the replica, independent nightly export, rehearsed fallback and 30 days of observation. A declined P6 closes the native-hosting track with a written evidence report, not a false cutover claim.

The Immich pilot is an API connector, not a hosted database. The client generation, the tools and the release catalogs belong to the connector repository (`MEDULLA-UNIFIED-DATA-PLANE-R032` is retired). Medulla keeps the mappings (`MEDULLA-UNIFIED-DATA-PLANE-R005`) and the cross-application entity links (`MEDULLA-UNIFIED-DATA-PLANE-R030`). Face vectors stay with the application, and per-user media, location and face data never cross user scope.

### Required outcomes and exclusions

At least three self-hosted apps, including one on MariaDB, must be attached end to end through the registry. Their old ingest connectors retire only after result and change parity. Ghostfolio is an optional one-way import into the finance ontology. Idempotent import precedes any separate retirement decision. Every dialect tier gets a green versioned conformance entry for its declared capabilities. The native track closes through the Gramps acceptance evidence or a documented decision not to cut over. No external Redis or graph mirror may be introduced as a read-path authority. No second connector registry, cache product or storage authority may be created.

### Program exit

All attached-source dialect tiers must pass their applicable conformance rows. Three self-hosted apps including a MariaDB app must be attached with ingest connectors retired. The accelerated copy must hold its lag objective across restart and failover. Gramps must either meet the native-hosting exit evidence or close with an evidence report if P6 is not approved. Record the architecture decisions of MEDULLA-UNIFIED-DATA-PLANE-R001, which includes the pgrx no-go. The [test specification](test-spec.md) defines failures as well as successes.

<!-- spec-definition-patch:owner-batch-20261012-unified-data-plane -->

Requirement IDs are defined in [requirements.md](requirements.md). Delivery status is computed from the rows, never written here.

### Graph-backed relational projections (`MEDULLA-UNIFIED-DATA-PLANE-R026.4`)

Expose graph-backed relational projections through an approved, version-pinned schema mapping. Reuse `AttachedCatalogGraph`/R2RML and the existing query surface. A projection-read request binds tenant, actor, graph, mapping id/revision, table, selected columns, typed predicates, and query digest. Bind the response to mapping revision, graph revision, column types, and result digest. Projection reads are read-only. Refuse INSERT, UPDATE, DELETE, or DDL through projected tables. Apply the same refusal to read-only principals and principals with a separate write grant. Write requests name the target graph mutation. Route them through existing policy/admission and the authoritative `MutationBatch` path. A separate mutation grant never makes a projection a writer. Preserve R026 P0–P5. Reuse R003/R005 for catalog and mapping. Reuse R029 for schema context.

**Boundary and preserved scope:** External application -> approved relational projection read; writes -> separately authorized graph `MutationBatch`. Preserve MEDULLA-DURABLE-KERNEL-R035, MEDULLA-DURABLE-KERNEL-R043, MEDULLA-FEDERATED-QUERY-R080, MEDULLA-UNIFIED-DATA-PLANE-R026, MEDULLA-FEDERATED-QUERY-R080.1, MEDULLA-FEDERATED-QUERY-R080.2, MEDULLA-FEDERATED-QUERY-R080.3, MEDULLA-FEDERATED-QUERY-R080.4, MEDULLA-FEDERATED-QUERY-R080.5, MEDULLA-FEDERATED-QUERY-R080.6. The schema/catalog and `schema_context` owners remain R003/R005/R029.

**Open dependency:** The source record names no external application, relational table/column contract, or driver statement corpus. Keep application-level schema compatibility open until the R026 P0/P5 owner furnishes and approves the concrete schema mapping and captured statements. Do not imply a universal relational schema.
