# MEDULLA-UNIFIED-DATA-PLANE — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md). Tests: [test-spec.md](test-spec.md). Tasks: [tasks.md](tasks.md).

## Summary

Medulla attaches third-party databases and services through one owner-scoped source registry and answers questions over them without copying entity rows. Registration enters at `Method::RegisterForeignSource`. Reads enter at `Method::UnifiedQuery`, the SPARQL and openCypher methods and `Method::Sql`, and all of them reach one `foreign_scan` in `crates/medulla-federation`. Adapters supply connection, dialect rendering, catalog reading, type decoding and change capture. Approved R2RML or operation-bound mappings turn source rows into named virtual graphs. A change-captured copy is a derived read path chosen only when its watermark meets the request's freshness contract.

```text
arbora (admission, auth, principal) --> medulla dispatch
  RegisterForeignSource | UnifiedQuery | Sparql* | CypherQuery | Sql | SemanticIndex | analytics jobs
    registry (verify target, secret ref) > catalog graph (hash) > approved mapping
      > planner (capability-aware pushdown) > foreign_scan > source session (one snapshot)
      > fragment receipts (EXPLAIN, provenance) > residual local join
  adapters --> ChangeEnvelope --> cursor --> graph / search / vector / lake consumers
```

| Concern | Owning code | Contract |
|---|---|---|
| Source registry | `crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs` | One record per source; target verified before persistence; secrets by reference only. |
| Catalog and types | `crates/medulla-catalog-types`, `crates/medulla-types` | Typed, hashed, versioned catalog graph; closed `SourceCategory`; refusal codes. |
| Planning and scans | `crates/medulla-plan`, `crates/medulla-federation`, `crates/medulla-exec` | Capability-aware pushdown; explicit fallback; one snapshot per source per query. |
| Mappings | `crates/medulla-h-sparql/src/rdf/obda.rs` | Approved R2RML or operation-bound mappings only. |
| Change capture | `crates/medulla-server-cdc`, `crates/medulla-stream` | `ChangeEnvelope`, per-subscription cursor, idempotent replay, incident authority. |
| Lake tier | `crates/medulla-lake`, `crates/medulla-server-lake` | Iceberg metadata and the authenticated REST catalog. |
| Analytics | `crates/medulla-h-query`, `crates/medulla-server-graph` | Profile, quality, column lineage, usage and catalog search jobs. |

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: The workspace crates of the Project Structure section, DataFusion providers for attached tables, `sqlx`, `tokio-postgres`, `tiberius` and the vendor or Arrow Flight SQL drivers of the dialect matrix.
- **Storage**: The durable kernel holds source records, mappings, catalog artifacts, capture cursors and accelerated copies. Attached application data stays in its source database.
- **Testing**: Rust unit tests per crate, served Rust tests under `tests/`, served Python tests under `tests/data_plane/`, `tests/federation/` and `tests/embedding/`, and containerized engine runs in the nightly conformance lane.
- **Target Platform**: Linux x86_64 and aarch64 hosts without AVX2 assumptions.
- **Project Type**: Library crates, handler crates and a server binary.
- **Performance Goals**: One snapshot per source per query with a 5000 ms replica lag bound; one connector query per web cache miss inside a 900 s time-to-live; 50 ms p99 enrichment overhead per 100 KiB page; 100,000 anomalies correlated in one pass. Each value is a starting default that a measurement on the reference host replaces.
- **Constraints**: Tenant filters survive every pushdown. Secrets and private application data never appear in EXPLAIN, logs, snapshots or fixtures. No capability is inferred from a dialect name. Each refusal has one stable code. No version suffix in a name.
- **Scale/Scope**: The leaf rows of `requirements.md`, spread over 7 user stories, 19 `FR-` labels and 12 `SC-` criteria.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. No external Redis, graph mirror or second database becomes a read-path authority.
- **R-II. Every wait has a deadline**: pass. Read-your-writes, statement execution, snapshot opening and connector calls each carry a timeout.
- **R-III. Every row sets budgets and names a benchmark**: pass. Hot-path rows name the served proof and the default budget.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name the post-split crates; a lower layer never calls a higher one, and no handler crate calls a sibling handler crate.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries; connector, served-door and harness parts sit in their owners' specs and link by ID.

## Project Structure

```text
crates/medulla-server-ops/src/api/query_txn/    existing: foreign source catalog and registration
crates/medulla-server-router/src/dispatch/      existing: dispatch arms and request boundary
crates/medulla-server-graph/src/dispatch/       existing: graph pipeline dispatch and ingest arms
crates/medulla-h-query/src/query/               existing: SQL, UnifiedQuery and openCypher handlers
crates/medulla-h-sparql/src/rdf/                existing: OBDA rewriter and R2RML support
crates/medulla-federation/src/federation_opt/   existing: foreign_scan, session and fragment receipts
crates/medulla-exec/src/exec/                   existing: operator dispatch to the foreign scan
crates/medulla-plan/                            existing: ForeignSourceSpec and pushdown planning
crates/medulla-types/src/                       existing: typed models; new files marked in their rows
crates/medulla-catalog-types/src/               existing: AttachedCatalogGraph
crates/medulla-lake/ and crates/medulla-server-lake/  existing: metadata writer and Iceberg REST catalog
crates/medulla-stream/                          existing: incident authority and stream records
tests/data_plane/, tests/federation/            existing and new: served Python tests
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns; no research.md.

## Phase 1: Design

The rules below bind every row. The sections after them hold the detail.

1. **One registry (`MEDULLA-UNIFIED-DATA-PLANE-R002`).** SQL federation, OBDA and capture share one source identity. No second registration path exists.
2. **Entity rows stay at the source (`MEDULLA-UNIFIED-DATA-PLANE-R038`).** Metadata and hot subsets materialize; everything else reads live through approved mappings.
3. **Capabilities are declared, never inferred.** An adapter reports `unsupported` for capture or pushdown, and the planner and conformance runner read that state.
4. **One consistent snapshot per source per query (`MEDULLA-UNIFIED-DATA-PLANE-R038.3`).** A transactional source answers every fragment from one read-only transaction.
5. **Approval gates exposure.** A proposed mapping, rule or entity link exposes no data and asserts no edge until an operator approves it.
6. **Fallbacks are visible.** EXPLAIN names every fragment that ran locally and every fragment that read a copy.
7. **Integration-first proof.** A behavior row is VERIFIED only by a test that enters through the production dispatch with a tenant and principal context.
8. **Ownership.** A medulla row keeps the source, mapping, query and catalog side. Served operations, connectors and deployment parts live in the owner's spec and link by ID.

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

### Authority and reuse

Use the existing Medulla foreign catalog and `medulla-plan::ForeignSourceSpec`, the server OBDA rewriter and R2RML support, `CdcHub`/`ChangeEnvelope`, `medulla-lake`, DataFusion providers, `KvStore`/GraphCore, DepClock cache, pgwire/MySQL/MSSQL/RESP listeners, and Medulla's native backup. Extend the actual owners in place; retire superseded registration paths after migration. Reuse the Graft SDK connector contract and Arbora query surface, with no duplicate engine-side connector catalog. MEDULLA-DURABLE-KERNEL-R024 moves fan-out mirror behavior into Medulla before the registry folds it in. Credentials stay as references to the deployment's secret provider.

### Module ownership and request path

`crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs` is the server-side source-record boundary. `crates/medulla-plan` owns planning of bound foreign sources. `crates/medulla-h-sparql/src/rdf/obda.rs` and `crates/medulla-virtual-graph/src/obda.rs` own virtual graph rewriting. `crates/medulla-server-cdc/src/cdc.rs` and `src/server/cdc_sink/` own change envelopes and sinks; `crates/medulla-lake` owns accelerated columnar copies. `crates/medulla-query/src/tables/` owns native user-table storage. Wire listeners authenticate and classify requests before planning. Keep one registry, one source ID and one policy path across these modules.

The request envelope carries tenant, actor, source grant, consistency mode, last committed source position (if any), timeout and trace ID. The server checks these before selecting a path. An attached table plan carries a bound source ID and catalog version, never a raw caller DSN. EXPLAIN can show route, pushdown fragments, source/copy position and fallback reason, with credentials and sensitive predicate values redacted. A prepared plan is invalidated on schema-version or grant change.

#### Source record and adapter interface

The durable record must contain `{source_id, tenant_id, owner_id, endpoint_binding, secret_ref, dialect, dialect_version, capabilities, allowed_catalog_scope, capture_config, current_catalog_version, status}`. Optional policies name acceleration, quotas, write-back and mirror targets. `endpoint_binding` is verified at registration and revalidated when a connection is created; DNS rebinding and private-address bypass are denied by outbound policy. Authorization is checked at registration, query planning and actual connection. Secrets are resolved only by the connection owner and never copied into a plan or trace.

An adapter provides `connect`, `render`, `read_catalog`, `decode_types`, and `capture` or an explicit no-capture result. Capability flags are versioned and testable: projection, predicate, limit, join, aggregate, order, transaction snapshot, and capture mode. The renderer quotes identifiers and binds values using the source dialect; it never concatenates user values into SQL. The catalog reader records stable object IDs, schema version, keys, FKs, indexes, views, enums, JSON shape and source provenance. Unknown types remain opaque typed values rather than silent text casts.

#### SQL destination fixture contract

The SQL registration fixtures for MEDULLA-UNIFIED-DATA-PLANE-R002.1 and the OBDA
constructor fixtures for MEDULLA-UNIFIED-DATA-PLANE-R002.2 explicitly allow
`127.0.0.1:5432` for positive cases and install an empty allowlist for refusals.
Loopback is not implicitly trusted. A restore-on-drop test guard holds the shared
crypto environment write lock, or a local lock when crypto is not compiled, for
both synchronous and asynchronous callers. The foreign-column owner/mapping
fixture uses this admitted numeric address so DNS failure cannot mask its refusal
assertions; those assertions run before any SQL connection.

OBDA constructor checks exercise destination validation only. They do not prove
R002.2's mixed allowed/disallowed `register_table_sources` batch acceptance;
that registry-level test remains required.

### Contracts and data flow

1. **Source registry.** A source record binds stable ID, tenant/owner, verified endpoint, secret reference, dialect and version, read and write capabilities, quota, mapping versions and capture position. Provisioning is an authenticated governed mutation. One registry identity is passed to OBDA, SQL federation, CDC and mirror sinks. Do not resolve arbitrary request-supplied DSNs. Revocation blocks new plans and drains active readers under an explicit policy.
2. **Adapter seam.** Each adapter implements connection, dialect SQL rendering, catalog extraction, type conversion, and capture or an explicit `unsupported` capability. Versioned capabilities drive planning and conformance. Postgres uses `pg_catalog` and `pgoutput`. MySQL and MariaDB use separate binlog/GTID fixtures. SQLite uses lock-safe read-only file access. MSSQL uses CDC/Change Tracking. ClickHouse and warehouses are federation-only unless a later approved contract adds capture. Oracle/Db2 use Debezium. Do not fabricate CDC from a periodic full scan.
3. **Schema intelligence.** Hash a typed catalog snapshot. Collect bounded, policy-gated column and JSON shape statistics. Infer unary/n-ary inclusion dependencies, functional dependencies and candidate keys with support counts, then use fixed-seed Leiden grouping. View definitions and captured joins can provide co-query edges. Deterministic name/type/value/structure matchers propose ontology mapping first. Optional model suggestions remain proposals and cannot approve or activate a mapping. Approved mappings become immutable versioned R2RML named graphs. DDL drift creates a new catalog version and suspends affected mappings pending review. `schema_context` reads the same graph and explains owner, joins, class and impact. Catalog artifacts without an OBDA or context consumer are not admitted.
4. **Queries.** The planner uses typed source values and capability-aware pushdown, including same-source joins/aggregates/order/limit. Fallback is explicit in EXPLAIN and retains tenant filters. The router compares freshness requirement, source position, copy position and latency policy before selecting native/live/accelerated. A bounded wait for the caller's last source commit is required for read-your-writes; timeout is a typed failure or policy-authorized live retry, never a silent stale result.
5. **Change path.** Adapters emit a source-keyed `ChangeEnvelope` carrying source position, transaction/order boundary, table/object ID, operation, before/after shape when available, schema version and event ID. The durable subscription cursor and idempotency key are committed with its consumer effects, or recovered by replay. Existing `CdcHub` fans into approved graph deltas, search/vector indexes, CEP/outbox and optional `medulla-lake` copies. Keyless updates, DDL changes, WAL/binlog retention and lag are explicit states with alerts. A snapshot/catch-up handshake prevents the gap between initial read and streaming. Mirrors are sink direction on the same registry and keep per-target cursor, replay and reconciliation; they never answer reads.
6. **Write path.** External mutation requires MEDULLA-DURABLE-KERNEL-R031 reservation, checked actor and target policy, idempotency and an approval receipt. Graft SDK/app API is used when application rules require it. Return an indeterminate outcome if the external effect may have succeeded but its acknowledgement/audit completion failed; reconcile before retrying.
7. **Native path dependency.** Before app admission, consume the `durable-graph-kernel` proof for point/KV routing, authz/audit parity, durability classes, hot/cold union, crash recovery and store choice. The pgrx companion remains a separate spike admitted only by ADR; this data-plane plan does not implement a second native store.

8. **Virtual-graph metadata.** Add `core:virtual-graph@1` and its shapes to the core ontology corpus. Extend the catalog reader so every source kind writes a hashed `MetadataContract`. Reuse the source registry identity and the R2RML mapping approval path. Add the acceleration policy fields to the source record. Copy no entity row outside that policy. (MEDULLA-UNIFIED-DATA-PLANE-R037, MEDULLA-UNIFIED-DATA-PLANE-R038)

### Delivery sequence

The dependency triggers in [tasks](tasks.md) control implementation. Begin with MEDULLA-UNIFIED-DATA-PLANE-R001 and the kernel-owned replay contract. Build the conformance harness before T1 adapters. Then catalog/mapping/query/capture and three app admissions. Add acceleration, router and write-back after durable positions and audit reservation. T2/T3 dialects follow T1 evidence. Gramps P0–P5 is isolated from production. P6 is a separate operator cutover. The kernel's bursty lane begins after MEDULLA-DURABLE-KERNEL-R016 and supplies MEDULLA-DURABLE-KERNEL-R042 benchmark evidence before this spec makes a comparative claim. Keep feature changes partitioned by actual owner files, not requirement IDs.

#### Dialect implementation matrix

| Tier | Dialect | Driver, catalog and capture contract |
|---|---|---|
| 1 | Postgres | `sqlx`/`tokio-postgres`, `pg_catalog`, `pgoutput`; preserve `jsonb`, arrays, enum and vector types. |
| 1 | MySQL and MariaDB | `sqlx mysql`, `information_schema`, row binlog plus GTID; separate conformance for both engines. |
| 2 | SQLite | Extend `medulla-sqlite-format` for `WITHOUT ROWID`, lock-safe read-only attach; WAL-frame tail only where safe, otherwise declared watermark polling. |
| 2 | SQL Server | `tiberius`, `sys.*` catalog, T-SQL `TOP` and `OFFSET/FETCH`, CDC or Change Tracking positions. |
| 2 | ClickHouse | HTTP/native query, `system.*` catalog, federation and acceleration; declare no native CDC. |
| 3 | Oracle and Db2 | Driver/ODBC query and catalog; Debezium bridge for change events. |
| 3 | MongoDB/DocumentDB | Native driver, bounded inferred document shape as catalog, change streams where supported. |
| 3 | Snowflake, BigQuery, DuckDB, Iceberg | Vendor query or Arrow Flight SQL where offered; federation-only, with Iceberg extending existing federation. |

#### Native transaction and storage dependency

`durable-graph-kernel` owns the mixed graph/table atomicity ADR, hot/cold union, storage-engine evaluation, native and RESP/SQL wire routing, durability classes, batch admission, plan cache and benchmark receipt. This spec consumes their exact-head crash, isolation, restore and performance results before admitting an application. External app data remains under its source database transaction; a Medulla CDC cursor is a derived read position, never a distributed-commit claim. The separate pgrx spike cannot become a parallel database authority by default.

### Design and quality constraints

- CCCC complexity, KISS file/symbol limits, jscpd and Dupehound duplicate gates, formatting, lint and hosted CI must be green on the exact proposed merge head. Record measured deltas and deleted/retired paths; do not add ratchets, suppressions, version-suffixed parallel implementations or a second source of truth.
- SQL semantics and types need differential tests against the real engine; planner pushdown must not bypass tenant filters. Secrets and private app data must not appear in EXPLAIN, logs, snapshots or fixtures.
- Preserve durable format and wire compatibility under the repository policy. New storage decisions require migration/rollback and crash recovery evidence. No benchmark claim follows merely from a structural simplification.
- Cross-repository changes follow the seams below; no owner repository duplicates another's runtime logic.

### Authenticated Iceberg catalog (R039)

Data flow: external engine token → `authorize_catalog_call` → tenant-scoped table list and load.
Attach point: `crates/medulla-lake/src/catalog.rs`, which documents a deny-all stub today. The first routing step is R039.3, locate and wire the listener.
Reuses the verified request envelope, tenant model and existing Iceberg metadata writer.
Decision: reuse the engine envelope. Rejected: a separate OAuth token store, which adds a second authority.
Lakekeeper-style external catalog interchange is not written here; the engine remains the catalog authority.

### Per-modality promotion gate (R040)

Data flow: served row write -> re-embed worker (`MEDULLA-FEDERATED-QUERY-R077.3`) -> `evaluate_promotion` -> `EmbeddingVersionStore::tag_batch` (`crates/medulla-ann/src/version.rs:129`) -> verdict node.

Types: `ModalityCheck`, `PromotionVerdict`, `PromotionBlocked`. Attach point: before `tag_batch`. Production caller: the Wiring row creates the call site in the re-embed worker. Failure behavior: a block keeps the prior version active and tags nothing.

Existing code: `crates/medulla-ann/src/version.rs:32`, `version.rs:54`, `version.rs:129`, `crates/medulla-ann/src/recall.rs:28`, `crates/medulla-table-store/src/tables/embedding_binding.rs:526`, `crates/medulla-modality/src/lib.rs`. Reachable today: no; `EmbeddingVersionStore` has no production caller. Search terms used: embedder_digest, model_digest, promot, projector, modality, recall_at, retrieval_check.

Decision: gate on each modality separately, so an average cannot hide one regression. Rejected alternative: gate on mean recall, because it passes a candidate that breaks a single modality.

Cross-repository: none.


### Graph-backed projections (R026.4)

- Add the projection request/response to the existing approved R003/R005 mapping and query path. Pin mapping and graph revisions; preserve R026 P0–P5 phase evidence.
- Enforce read-only behavior for projections at the query boundary, including callers with a separate graph mutation grant.
- Route explicitly authorized mutation requests through the existing graph `MutationBatch` admission path from R035/R043.
- Served acceptance: read a mapped table through an approved, pinned mapping. Bind the response to schema and graph revisions.
- Deny each projected write for read-only and separately mutation-granted principals. Keep graph revision unchanged.
- Submit an authorized graph mutation through `MutationBatch`. Confirm the projection read reflects its committed revision.
- Also deny stale/unapproved mappings and unauthorized graph mutations.
- Open dependency: the source names no target application, full relational schema, or captured statement corpus. The R026 pilot owner must supply and approve those artifacts before application compatibility is accepted.
- Preserve MEDULLA-DURABLE-KERNEL-R035, MEDULLA-DURABLE-KERNEL-R043, MEDULLA-FEDERATED-QUERY-R080, MEDULLA-UNIFIED-DATA-PLANE-R026, MEDULLA-FEDERATED-QUERY-R080.1, MEDULLA-FEDERATED-QUERY-R080.2, MEDULLA-FEDERATED-QUERY-R080.3, MEDULLA-FEDERATED-QUERY-R080.4, MEDULLA-FEDERATED-QUERY-R080.5, MEDULLA-FEDERATED-QUERY-R080.6.

### Source categories: OLTP, metastore, search index, graph languages and the coverage matrix (R038.3 to R038.6, R022.4)

These rows extend the existing virtual-graph rollups. They add no second planner. Entity rows stay at the source (`MEDULLA-UNIFIED-DATA-PLANE-R038`). Mappings are R2RML-style (`MEDULLA-UNIFIED-DATA-PLANE-R005`). Pushdown reuses the federation optimizer (`MEDULLA-FEDERATED-QUERY-R038`, `MEDULLA-FEDERATED-QUERY-R041`). Connection protocols form the second axis in `MEDULLA-FEDERATED-QUERY-R073.2`.

Call path: served query -> planner -> `foreign_scan` (`crates/medulla-federation/src/federation_opt/run.rs:26`) -> source session -> pushed fragment -> local residual.

#### Source-category coverage matrix (R022.4)

| Category | Standard protocol or format | Pushdown | Mapping form | Hot-path proof | Residency |
|---|---|---|---|---|---|
| Relational SQL | SQL through ODBC or JDBC drivers | filter, projection, join, aggregate, limit, time range | R2RML | `MEDULLA-UNIFIED-DATA-PLANE-R006` differential test | virtual |
| PostgreSQL wire | PostgreSQL frontend/backend protocol 3.0 | filter, projection, join, aggregate, limit, time range | R2RML | `MEDULLA-UNIFIED-DATA-PLANE-R022.2` matrix runner | virtual |
| OLTP row store | SQL with snapshot isolation | filter, projection, join, aggregate, limit, time range | R2RML plus `SnapshotSemantics` | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.6` | virtual |
| OLAP and columnar | SQL; Arrow Flight SQL | filter, projection, join, aggregate, limit, time range | R2RML | `MEDULLA-UNIFIED-DATA-PLANE-R020.2.2` | virtual |
| Time series | SQL with time-bucket aggregates | filter, projection, aggregate, limit, time range | R2RML with a time column | `MEDULLA-UNIFIED-DATA-PLANE-R022.2` matrix runner | virtual |
| Search index | HTTP JSON search query; vector nearest-neighbour query | filter, projection, limit, relevance top-k | document-id mapping plus score channel | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.6` | virtual |
| Document store | JSON documents addressed by JSONPath | filter, projection, aggregate, limit | RML with JSONPath | `MEDULLA-UNIFIED-DATA-PLANE-R022.2` matrix runner | virtual |
| Web metasearch | MCP tool call | category, engine, limit, time range | `cv:SearchResult` with a cache | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.6` | virtual, 900 s time-to-live |
| Key-value store | key lookup and batched key lookup | key filter, limit | key-template mapping | `MEDULLA-FEDERATED-QUERY-R052` | virtual |
| Data lake tables | Apache Iceberg REST catalog; Parquet, ORC, Avro | partition pruning, projection, limit, snapshot time range | R2RML | `MEDULLA-FEDERATED-QUERY-R051`; `MEDULLA-UNIFIED-DATA-PLANE-R038.4.6` | virtual |
| Hadoop-family catalog | Hive metastore Thrift protocol | filter by partition, projection | R2RML over the table schema | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.6` | virtual |
| Spark-compatible engine | Spark Connect gRPC protocol; SQL | filter, projection, join, aggregate, limit, time range | R2RML | `MEDULLA-FEDERATED-QUERY-R053` | virtual |
| Arrow Flight SQL | Arrow Flight SQL | filter, projection, join, aggregate, limit, time range | R2RML | `MEDULLA-UNIFIED-DATA-PLANE-R020.2.2` | virtual |
| Property-graph source | openCypher; Gremlin | filter, projection, pattern join, aggregate, limit | label and type mapping | `MEDULLA-FEDERATED-QUERY-R053`; `MEDULLA-UNIFIED-DATA-PLANE-R038.6.6` | virtual |
| Object storage | HTTP object API with range reads | prefix filter, byte-range projection | file-format mapping | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.6` | virtual |

Budgets: one snapshot per source per query and a 5000 ms replica lag bound. The statement timeout is 30 s, the path depth limit is 8, and catalog pages hold 1,000 tables. These are starting defaults. A measurement on this platform replaces them.

### Web metasearch sources (R038.7)

A web metasearch connector answers a pattern over web results at query time. The connector tools take query text, categories, engines, a language and a page. The engine maps each result to `cv:SearchResult` with URL, title, snippet, engine, rank and retrieval time.

Call path: served SPARQL or UQL -> planner -> `foreign_scan` (`run.rs:26`) -> `resolve_web` -> `web_scan` (`web.rs`) -> `FragmentCacheScope::get_or_fetch_web` (`cache.rs`) -> MCP operation of `MEDULLA-FEDERATED-QUERY-R073` through the connector port (`MEDULLA-UNIFIED-DATA-PLANE-R038.7.9`) -> typed rows with text columns (`MEDULLA-UNIFIED-DATA-PLANE-R038.7.8`).

The web rows keep the URL as the row id, the text columns in `TextChannels` and the numeric columns (`rank`, `retrieved_at`, `from_cache`, `truncated`) in the value channels. A source is a web source when the bound tool's port declares a web capability; the cache entry TTL comes from `federation.web_cache_ttl_s` through `SourceWatermark.entry_ttl_ms`, supplied by `fragment_sources` for each web source.

Selection: a harness that asks Arbora to search the web receives the search tools through `MEDULLA-DECISION-ENGINE-R176` and `ARBORA-FLEET-R037`. Online refresh of embeddings over these results belongs to the adaptive-embeddings rows. They reference `MEDULLA-UNIFIED-DATA-PLANE-R038.7`.

Budget: a 900 s default time-to-live and one connector query per cache miss. These are starting defaults. A measurement on this platform replaces them.

### Web page ingest (R022.4.1, R038.7.7, R651.7)

Search results stay virtual. A web metasearch pattern returns a `WebCandidateSet` with query provenance (`MEDULLA-UNIFIED-DATA-PLANE-R038.7.7`). The engine stores no result row. The fetch selection of `MEDULLA-DECISION-ENGINE-R802` chooses the pages to fetch.

A fetched page is the only web content that the engine stores. It enters as one Document through `SourceIngest` (`MEDULLA-UNIFIED-DATA-PLANE-R651.7`). The source connector is `webarchiver`; `archivebox` is the secondary backend. Both produce the same Document shape.

Call path: arbora `research.web` (`ARBORA-OPS-R067`) -> Graft SDK fetch through `webarchiver` or `archivebox` -> served `SourceIngest` (one Document) -> served `EnrichDocument` -> `crates/medulla-server-graph/src/dispatch/graph_pipeline/web_ingest.rs` -> one ChangeEnvelope through `ApplyChangeEnvelope`.

`EnrichDocument` chunks with `medulla_document::processing` (`MEDULLA-REPO-INGEST-R652`). It embeds through the configured `/v1/embeddings` wire endpoint (`MEDULLA_EMBEDDING_URL`, `_MODEL`, `_DIMS`, optional `_API_KEY`), or through the hash embedder, which it records as `embedder = hash`. The vectors use the `AddEmbedding` path, so `SemanticSearch` and hybrid retrieval find them. Extraction is `MEDULLA-INGEST-WORKER-R701` (`R651.7.7`). Citations are `MEDULLA-FEDERATED-QUERY-R653.7`. Refresh is `MEDULLA-DECISION-ENGINE-R174.9.7`.

Budget: 50 ms p99 enrichment overhead per 100 KiB page and 64 MiB peak memory. These are starting defaults. A measurement on this platform replaces them.

### Profiling, quality, column lineage and usage (R761 to R764)

R761 and R762 are two new `AnalyticsJob` kinds. Both push aggregates to attached sources through
R006 where the dialect allows it. R762 compiles graph rules to SHACL through `crates/medulla-shacl`.
Three consecutive failures of one rule open one incident through R760. R763 extends the dataset
lineage of R702 to columns by parsing SQL text with the shared planner parser. R764 counts asset
use from the query audit and fuses a capped usage prior into catalog search.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row in table order, producer first. A model row precedes the rows that wire it. A row lands in one slice with its caller, so no type lands without a production caller. Adapter tasks of different dialects run in parallel after the conformance harness exists. Tasks that edit the shared `foreign_scan` path or the shared dispatch arms run in sequence.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- SQL semantics and types have differential tests against the real engine; pushdown never bypasses a tenant filter.
- Every hot-path row names a budget and a benchmark, per article R-III, and the default values of this plan are replaced by a measurement on the reference host.
- A public item in a data-plane crate has a non-test caller under a handler or server crate. `MEDULLA-CONTRACT-R055` gates this.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- A comparative performance claim waits for the durable kernel's benchmark receipt (`MEDULLA-DURABLE-KERNEL-R042`).

## Production caller chain

- `Method::RegisterForeignSource` arm (`crates/medulla-server-router/src/dispatch/request_boundary/saga.rs`) to `ForeignSourceCatalog::register` (`crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs`), which verifies the outbound target before it persists.
- `Method::UnifiedQuery` through `dispatch_graph_op` (`crates/medulla-server-graph/src/dispatch/graph_pipeline.rs:68`) to `handle_unified_query` (`crates/medulla-h-query/src/query/sql_read.rs:309`) to `FederatedExec` (`crates/medulla-federation/src/federation_opt/federated.rs`) to `foreign_scan` (`crates/medulla-federation/src/federation_opt/run.rs:26`), reached from the operator dispatch at `crates/medulla-exec/src/exec/dispatch.rs:364`.
- `Method::Sql` to `handle_sql` (`crates/medulla-h-query/src/query/sql_read.rs:136`), whose session registers `hive` and the other table functions in `register_pg_common` (`crates/medulla-sql/src/sql/exec.rs:75`).
- `Method::CypherQuery` to `handle_cypher_query` (`crates/medulla-h-query/src/query/cypher.rs:47`) to `handle_cypher_read` (`cypher.rs:67`), which compiles a named view to the virtual SPARQL path.
- Every scan records a `FragmentTrace` through `FederationSession::record` (`crates/medulla-federation/src/federation_opt/session.rs:113`), which feeds EXPLAIN and provenance.
- The Iceberg REST listener starts in `spawn_iceberg_listener` (`src/main.rs:903`) and serves through `serve_inner` (`crates/medulla-server-lake/src/lake/rest/mod.rs:296`) to `route` (`mod.rs:403`) after `resolve_carrier` (`mod.rs:160`).
- `Method::SemanticIndex` arm (`crates/medulla-server-router/src/dispatch/router/control_plane.rs:404`) to `handle_semantic_index` in `crates/medulla-h-retrieve`, where the promotion gate runs before a binding goes `Live`.
- The Postgres wire listener `serve_prepared` (`crates/medulla-wire/src/pgwire/mod.rs:1940`) classifies each statement and refuses a write to a projected table.
- Obsolete paths to delete: `CatalogPrincipal` and `authorize_catalog_call` in `crates/medulla-lake/src/catalog.rs`, which have no production caller once the carrier gate is the only authorization.

## Cross-repo seams

- Served catalog and schema operations: `MEDULLA-UNIFIED-DATA-PLANE-R765` provides, `ARBORA-OPS-R067` consumes. Contract: [arbora-served-operations.md](contracts/arbora-served-operations.md).
- Governed write-back and its idempotency key: `MEDULLA-UNIFIED-DATA-PLANE-R012.1` provides, `SCION-GOVERNED-WRITEBACK-R001` consumes. Contract: [scion-governed-writeback.md](contracts/scion-governed-writeback.md).
- Source ingest of fetched pages and outside quality results: `MEDULLA-UNIFIED-DATA-PLANE-R651.7.3` provides, `SCION-SOURCE-INGEST-R001` consumes. Contract: [scion-source-ingest.md](contracts/scion-source-ingest.md).
- Gramps pilot corpus and phase exits: `MEDULLA-UNIFIED-DATA-PLANE-R026.2.2` provides, `CONN-GRAMPS-MCP-CONNECTOR-CONTENT-R001` consumes. Contract: [gramps-native-pilot.md](contracts/gramps-native-pilot.md).
- Cross-application entity links: `MEDULLA-UNIFIED-DATA-PLANE-R030.3` provides, `CONN-IMMICH-API-R001` consumes. Contract: [immich-entity-links.md](contracts/immich-entity-links.md).
