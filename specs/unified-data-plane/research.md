# unified-data-plane — Research

## Decision: Registration is synchronous, probing is an async state machine

**Decision.** Registration stays a synchronous record write. Probing is a separate async job with the explicit state machine Registered, Probing, Baselined, Drifted or ProbeFailed (`MEDULLA-UNIFIED-DATA-PLANE-R014.2.3`). The SCHEMA_DRIFT baseline is the schema digest captured at the first successful probe. The L3 test runs against containerised MySQL and MariaDB in the nightly lane.

## Decision: Web rows carry text columns; the gateway is the connector port the engine reaches

**Decision.** A web result row is a `RowSet` row whose id is the URL. Its text columns (`title`, `snippet`, `engine`, `connector`, `argument_digest`, `query_text`) live in a new `TextChannels` table beside the numeric `ValueChannels` (`rank`, `retrieved_at`, `from_cache`, `truncated`). Id and score stay ordinary columns, so the one tabular channel of the result-shape decision can absorb both tables later without a second model. The capability of a web tool comes from the host connector port at use (`ConnectorRowSource::web_capability`); the catalog stores nothing new. A host-registered port wins; otherwise the engine posts each call to the fleet gateway named by `MEDULLA_CONNECTOR_GATEWAY_URL` through the shared outbound guard, so a served engine and a test reach connector tools by one path. The cache is process memory keyed by owner salt, source, fingerprint and argument digest; an entry serves while its age is at most the TTL, and the knob `federation.web_cache_ttl_s` (default 900, range 0 to 86400) sets it. Structured arguments (`categories`, `engines`, `language`) travel as pushed filters and are checked against the declared capability; the planner surfaces the query text from the `RankText` branch exactly as for search-index sources.

**Why.** It is the smallest change that makes the row typed without a second row model, keeps Medulla free of any MCP client, and lets the served proof run the same code path a deployment runs.

## Decision: the probe machine lives beside the catalog model, and the catalog is held with the source

**Decision.** `CatalogProbeMachine` sits in `medulla-catalog-types` next to `AttachedCatalogGraph`, so the ontology test needs no heavy dependency and the machine is reachable from the server and the planner without a cycle. The `sqlx` reads sit in `medulla-federation` (`mysql_introspect`), which already links `sqlx`; `medulla-plan::mysql_catalog` holds the type map and the catalog assembly, so no new dependency enters the lock. The allowed scope is a schema list; an empty list reads the connection's own database. The catalog read by the last probe is held beside the source record in the owner-scoped catalog, and a drifted source is refused `SCHEMA_DRIFT` until it is registered again, which takes a new baseline. `foreign.mysql.pool_size` is a registry knob (default 4, bounds 1 to 64).


## Decision: Column lineage is stored by the lineage drain; the catalog is the probe-held one

**Decision.** `ColumnLineage` stays a read method: `DeriveFromSql` is pure compute, `DeriveFromCatalog { source }` and `Impact` read the caller's own tenant graph and commit nothing. The edge write is the lineage drain: `LineageEvent` gains `sql_text`, `dialect` and `catalog_source` (all optional, defaulted), and `handle_lineage_ingest` (R702.3) lowers each derived column edge as a `Column` node pair plus a `DERIVES_FROM` edge carrying `transform`, `query_digest` and `parser_version` through the same ChangeEnvelope commit as dataset edges. The catalog is the one the probe job already holds beside the owner-scoped source record (`ForeignSourceCatalog::catalog_read`): a second copy as a graph node would drift from it and bypass its `SCHEMA_DRIFT` refusal, and its owner key already isolates tenants. `DeriveFromCatalog` and the drain read it; a source with no held catalog refuses `LINEAGE_CATALOG_MISSING`. A `Column` node per column (`col:<qualified>`) and a `DERIVES_FROM` edge from the output to the input column carry the lineage; the drain reaches the parser through `QueryApi::derive_column_lineage`. The PostgreSQL catalog read (R013.1) mirrors the MySQL one so both dialects fill `views`. Parse errors and oversized statements become `QuarantineReason::SqlUnparseable` and `SqlTooLarge`, kept as `QuarantinedLineageEvent` nodes by the same drain. `ColumnLineage` moves from pure compute to a tenant-graph read in the read-classification table once `Impact` ships.

**Why.** One writer for lineage, no second commit path, no new write method in the nine write-classification tables, and tenant isolation comes from the graph a request is bound to. Recorded here for the rows R763.3 to R763.7 and R013.1. `Impact` reads the RLS-projected graph, so `ColumnLineage` is classified `RLS_ROUTED`. The query digest normalizes whitespace so a re-formatted statement keeps its digest.

## R764.7 and R764.8: query log usage and the usage answer

Decisions (owner standard: most correct, long-term, simplest):

- `pg_stat_statements` has no per-statement time and only running totals, so the log trait
  carries `calls` and an optional `cumulative_key`; one cursor (watermark plus totals) serves
  both the cumulative Postgres log and the MySQL event log. The cursor is a JSON record under the
  durable state directory, written after the points are stored, so a failed write retries and a
  replay of the same state adds nothing.
- The job kind carries the verified owner and tenant scopes stamped at submit. The worker never
  trusts a client value for the scope the usage lands in.
- Joins count for both tables (`with` tag) so either table can suggest the other; filters count
  per table and column. The suggestion minimum is a knob, so a rare join is not a suggestion.
- `CatalogUsage` reads only the usage series of the caller's own tenant and graph key. An asset
  with no series in that scope is `CATALOG_ASSET_NOT_FOUND`, which is also what another tenant
  sees, so the refusal never confirms that an asset exists elsewhere.

## Graph view compilation (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2)

Decision: `medulla-cypher` cannot depend on `medulla-virtual-graph` (the latter reaches the former), so the compiler emits a plain `ViewPlan` of SPARQL text plus `ViewJoin` records, and `apply_view_joins` in the shared virtual-graph crate turns same-database joins into one derived triples map. Property and relationship predicates follow the namespace-of-class naming rule, so a view needs no extra property table. Unbounded `*` is capped by the parser at 16 hops and therefore refuses against the default knob of 8.

## Decision: how a named graph view reaches its sources (MEDULLA-UNIFIED-DATA-PLANE-R038.6.3)

`RegisterGraphView` carries the view, the R2RML text its labels map through, the engine tables and the live relational sources
(`name`, `dsn`, `table`) that mapping reads; the registered foreign-source catalog holds a query, not a table, so it cannot name
the source table of a mapping. The view is owner-scoped in the same catalog partition as foreign sources, in memory only, and its
`mapping_digest` must be the SHA-256 of the mapping text, so a view cannot name a mapping other than the one it runs. A read runs
through `view_virtual` in `medulla-h-sparql`, which shares the SSRF gate, snapshot session and source driver of `SparqlVirtual`
instead of duplicating them; `medulla-h-query` already depends on that crate. EXPLAIN is the leading keyword of the Cypher text, so
the wire gains exactly one field (`view`).

Projection read decision (R026.4.1 to R026.4.3). A projection mapping revision is one graph node of the caller's tenant, so it is a graph fact written through the durable mutation path like every other write; the read handler writes nothing. The table is read by a typed scan of the caller-visible nodes of the mapped label rather than through an OBDA SPARQL rewrite: a projection answers a relational table read with typed columns and a primary-key digest, and the OBDA seam only carries lexical strings, so a rewrite would lose the column types the response must bind. Approval reuses the R005.1 state type, so there is one approval state. Staleness is a higher stored revision of the same mapping id. The session names its mapping in the startup parameter `medulla.mapping` (or `-c medulla.mapping=` in `options` for drivers that cannot send arbitrary startup parameters); while it is set, every non-read statement is refused with SQLSTATE `25006` before the access check, so a graph write grant never makes a projection a writer.
## Decision: where schema_context reads its answer (MEDULLA-UNIFIED-DATA-PLANE-R029.2)

The platform's `schema_context` call is owned by R029.2; no second row or method is added. The
served `Method::SchemaContext` builds its answer only from state the engine already holds for the
verified caller, reusing the existing read paths instead of a new catalog store: the baselined
attached-source catalog (`catalog_read`, R014.2.3) gives columns and joins (the table's own foreign
keys and every foreign key that references it); the caller's registered graph views give the
ontology class, counting only a view that reads exactly this source table, because a view over
several tables cannot say which label belongs to which table; the application is the attached
source and the owner is the agent that registered it; the impact reuses the R763.3 downstream walk
over the stored column edges, inside the same `lineage.impact_*` knobs, so no new limit exists.
The answer always carries every part and the intent names the part the caller needs: a part that
is unknowable for the table is refused for that intent (`SCHEMA_CONTEXT_UNMAPPED`,
`SCHEMA_CONTEXT_CLASS_AMBIGUOUS`) rather than answered empty. The handler lives in
`medulla-h-query` because it reads server state; `medulla-types` keeps the answer types and the
lookup that refuses an unknown table. Another owner's source of the same name resolves exactly like
an unknown source (`SCHEMA_CONTEXT_SOURCE_UNKNOWN`), so the refusal never confirms existence.
WIRE-REGEN needed: the new method and result type change the wire contract vectors.
## Work-item kind registry (R766)
The closed `WorkItemKind` registry holds eight kinds, not six: `agent.execute` is submitted by the delegation path (`DelegationRequest.kind` is forwarded into `SubmitWorkItemRequest`) and `agent_turn` by the durable agent-turn queue, so a closed registry without them would refuse a production caller. Wire strings are unchanged. The registry is the single admission check; fixtures that used an arbitrary kind use a registry kind.
The enum lives in its own module `medulla-work-types::work_item_kind`, beside rather than inside `native_control.rs`; the submit result and write inputs derive the initial status from the pending-dependency count through `submit_work_item_status` instead of carrying it as a second, string-typed copy.
## R031.2: where a source's quota is enforced

A source's quota is one `SourceQuota` (concurrent requests, rows per query, wall time per request) stored with the registration and resolved to the knobs `foreign.default_quota.*` when the registration names none. The optimizer enforces it in `fetch_one`, the one place every foreign request passes: admission is bounded at one source wall so a source holding its own permits never holds another's, rows are metered per source fingerprint in the query session, and a request is measured against its wall. A request narrows the quota through the query budget and never widens it. An error from a source that carries no code becomes `SOURCE_UNAVAILABLE`; one that already carries a code keeps it.

## Decision: where discovery runs and where the contract is written (MEDULLA-UNIFIED-DATA-PLANE-R038.2.1)
Discovery runs before the catalog insert, so a source whose catalog cannot be read refuses with `DISCOVERY_UNREACHABLE` and leaves no catalog entry. PostgreSQL-wire and MySQL-family sources read their live catalog through the readers the probe job uses and hash the canonical, sorted schema bytes with SHA-256; every other kind hashes the schema its registration declares, because it has no catalog to read at registration. The contract is one `AddNode` through the durable internal commit path (DE-R174.9.8), written from the handler that owns the registration: `medulla-h-sparql` depends on `medulla-server-mutation` and receives the request graph, persistence and verified principal as a `ContractWriteTarget`. If the durable write fails the previous catalog entry (or none) is restored, so catalog and contract never disagree. `schema_version` is read from the stored contract: unchanged hash keeps it and writes nothing, a changed hash adds one.

## Metastore sources: `hive()` resolution and discovery (R038.4.3)

- One source is resolved by name from the verified caller's own registry; a call with no source name resolves when the caller owns exactly one metastore source and refuses otherwise. An unknown name and another principal's name give the same refusal.
- The metastore client speaks the unauthenticated Thrift binary protocol; `auth_ref` is validated as a secret reference at registration and carried in the spec. A token-bearing metastore handshake is a separate connector concern.
- The source contract is one `MetadataContract` per source: it folds each table's `MetastoreTable::content_hash`, so any table schema, partition or file change raises the schema version.
- The scan returns its receipt through the query warnings (`QueryResult.warnings`), the single evidence channel of a served read.

## Profile and quality jobs (R761.3, R762.3)

- Decision: profile and quality jobs write their graph nodes in one engine-internal batch keyed by the job id (`data_plane_write.rs`), so a retried attempt replays instead of writing twice. Rejected: a seam call per node.
- Decision: incidents opened by a failing rule streak are correlated with `IncidentAuthority` and written in the job's own batch through the shared `medulla_stream::telemetry::incident_nodes` module (moved from the router so one shape serves both). Rejected: a `DecideApi` seam to `medulla-h-decide`.
- Decision: the verified plain tenant and agent are stamped into the `JobKind` at submit (client values replaced), because the worker claim has no request carrier.
- Decision: R761.3 no longer depends on R803. R803.3 adds the sensitive scan to the profile executor; the old edge made a cycle.
- Decision: SQL-source datasets are named `<source>.<table>` and need named columns; native tables profile through DataFusion.
- Decision: rule and score node ids are tenant-private hashes; `QualityRulePut` stores the rule approved by the verified agent and resets its failure counter.
- Deferred: shape and freshness measurement (R762.9) return `QUALITY_MEASURE_UNAVAILABLE`.
- WIRE-REGEN: `QualityRulePut`, the new `JobKind` variants and `QualityRuleReceipt` were hand-edited into `medulla/generated/models.py`; regenerate models and wire vectors.
