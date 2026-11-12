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
