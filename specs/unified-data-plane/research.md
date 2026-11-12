# unified-data-plane — Research

## Decision: Registration is synchronous, probing is an async state machine

**Decision.** Registration stays a synchronous record write. Probing is a separate async job with the explicit state machine Registered, Probing, Baselined, Drifted or ProbeFailed (`MEDULLA-UNIFIED-DATA-PLANE-R014.2.3`). The SCHEMA_DRIFT baseline is the schema digest captured at the first successful probe. The L3 test runs against containerised MySQL and MariaDB in the nightly lane.

## Decision: Web rows carry text columns; the gateway is the connector port the engine reaches

**Decision.** A web result row is a `RowSet` row whose id is the URL. Its text columns (`title`, `snippet`, `engine`, `connector`, `argument_digest`, `query_text`) live in a new `TextChannels` table beside the numeric `ValueChannels` (`rank`, `retrieved_at`, `from_cache`, `truncated`). Id and score stay ordinary columns, so the one tabular channel of the result-shape decision can absorb both tables later without a second model. The capability of a web tool comes from the host connector port at use (`ConnectorRowSource::web_capability`); the catalog stores nothing new. A host-registered port wins; otherwise the engine posts each call to the fleet gateway named by `MEDULLA_CONNECTOR_GATEWAY_URL` through the shared outbound guard, so a served engine and a test reach connector tools by one path. The cache is process memory keyed by owner salt, source, fingerprint and argument digest; an entry serves while its age is at most the TTL, and the knob `federation.web_cache_ttl_s` (default 900, range 0 to 86400) sets it. Structured arguments (`categories`, `engines`, `language`) travel as pushed filters and are checked against the declared capability; the planner surfaces the query text from the `RankText` branch exactly as for search-index sources.

**Why.** It is the smallest change that makes the row typed without a second row model, keeps Medulla free of any MCP client, and lets the served proof run the same code path a deployment runs.

## Decision: the probe machine lives beside the catalog model, and the catalog is held with the source

**Decision.** `CatalogProbeMachine` sits in `medulla-catalog-types` next to `AttachedCatalogGraph`, so the ontology test needs no heavy dependency and the machine is reachable from the server and the planner without a cycle. The `sqlx` reads sit in `medulla-federation` (`mysql_introspect`), which already links `sqlx`; `medulla-plan::mysql_catalog` holds the type map and the catalog assembly, so no new dependency enters the lock. The allowed scope is a schema list; an empty list reads the connection's own database. The catalog read by the last probe is held beside the source record in the owner-scoped catalog, and a drifted source is refused `SCHEMA_DRIFT` until it is registered again, which takes a new baseline. `foreign.mysql.pool_size` is a registry knob (default 4, bounds 1 to 64).
