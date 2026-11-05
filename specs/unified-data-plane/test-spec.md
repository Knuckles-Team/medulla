# Test and acceptance specification

**State:** PROPOSED. Evidence must cite the exact merged commit, fixture/source version, command, environment and result. A passing isolated branch is implementation evidence, not acceptance. See [requirements](spec.md), [design](plan.md) and [tasks](tasks.md).

| Gate | Positive proof | Negative and failure proof | Requirement IDs |
|---|---|---|---|
| Registry and identity | One owner-scoped source drives SQL, named OBDA, CDC and mirror registration; secret references resolve only at service boundary. | Wrong tenant, expired/rotated secret, rebinding attempt, private/outbound address blocked, missing grant and stale source version fail before mutation or network connection. | MEDULLA-UNIFIED-DATA-PLANE-R001, MEDULLA-UNIFIED-DATA-PLANE-R002, MEDULLA-DURABLE-KERNEL-R036, MEDULLA-UNIFIED-DATA-PLANE-R031 |
| Catalog and mapping | Repeat snapshots produce stable hashes; keys, views, enums and nested JSON shapes match native catalog; approved R2RML answers known SPARQL/UQL/REASON questions. | Bounded sample budget exceeded, schema drift, inferred false FK, model-only proposal, unapproved mapping, deleted column and cross-app exposure do not leak or auto-approve. | MEDULLA-UNIFIED-DATA-PLANE-R003–MEDULLA-UNIFIED-DATA-PLANE-R005, MEDULLA-UNIFIED-DATA-PLANE-R029 |
| SQL and OBDA | Compare pushed-down and non-pushed results with native source for nulls, collation, time zones, numeric/array/JSON types, pagination, joins and aggregates; EXPLAIN identifies route and fallbacks. | Unsupported dialect expression falls back visibly; tenant predicate cannot be dropped; malformed mapping/hostile SQL cannot escape allowed table/owner scope. | MEDULLA-UNIFIED-DATA-PLANE-R006, MEDULLA-UNIFIED-DATA-PLANE-R007, MEDULLA-UNIFIED-DATA-PLANE-R011, MEDULLA-UNIFIED-DATA-PLANE-R022 |
| CDC and acceleration | Initial snapshot plus replay equals current source after restart and source failover; graph/search/vector/CEP consumers and lake copy converge once by source position. | Duplicate/out-of-order event, dropped key, keyless update, DDL mid-stream, WAL/binlog retention pressure, unavailable sink and stale copy leave a diagnosable lag or typed error; read-your-writes never returns stale success. | MEDULLA-UNIFIED-DATA-PLANE-R008–MEDULLA-UNIFIED-DATA-PLANE-R011, MEDULLA-UNIFIED-DATA-PLANE-R021 |
| Write-back and mirrors | Approved idempotent API/direct write reserves MEDULLA-DURABLE-KERNEL-R031 audit; consume the kernel mirror cursor/reconcile receipt and restore SQL tables into real Postgres. | Missing approval/audit, duplicate key, ambiguous external response, empty-default mirror, down target and retry do not create duplicate or unlogged effects. Mirror is never selected as a read authority. | MEDULLA-UNIFIED-DATA-PLANE-R012; kernel dependency MEDULLA-DURABLE-KERNEL-R036 |
| Dialect matrix | For each adapter's covered engine/version, catalog, type map, supported pushdown and CDC or explicit no-CDC capability pass. Source query equals Medulla result. | Disconnected source, schema/version change, incompatible type, unsupported capture and native-source discrepancy fail its row rather than being counted green. | MEDULLA-UNIFIED-DATA-PLANE-R013–MEDULLA-UNIFIED-DATA-PLANE-R022 |
| Platform and app admissions | PITR and scheduled restore drill. Per-app roles. Three attached apps including MariaDB. Connector ingest retired only after parity. Consume the finance-owned Ghostfolio import receipt. | One app migration failure rolls back without affecting others. Revoked role, stale replica, extension mismatch and duplicate Ghostfolio import are caught. | MEDULLA-UNIFIED-DATA-PLANE-R023, MEDULLA-UNIFIED-DATA-PLANE-R024. Finance dependency MEDULLA-FINANCE-PRIMITIVES-R012 |
| Native Gramps admission | P0 XML digest. P1 real-Postgres control and statement corpus. P2 unmodified Medulla failure capture. P3 differential replay and export digest. P4 three restore paths and kill-9. P5 approved OBDA answer and `schema_context`. | Every suspected feature has a captured failing/passing test before implementation. SQLSTATE, app migrations, crash anomalies, dump/restore mismatch and cross-store partial commit are explicit failures. P6 requires separate operator approval, 30-day stability and failover evidence. | MEDULLA-UNIFIED-DATA-PLANE-R025–MEDULLA-UNIFIED-DATA-PLANE-R028, MEDULLA-UNIFIED-DATA-PLANE-R033. Kernel dependencies MEDULLA-DURABLE-KERNEL-R032/MEDULLA-DURABLE-KERNEL-R033/MEDULLA-DURABLE-KERNEL-R034/MEDULLA-DURABLE-KERNEL-R035 |
| Immich and entity resolution | API fixture/live smoke, full four-user ingest followed by no-op replay and one incremental upload; approved links answer cross-app relationship question. | Unnamed faces remain unlinked, low-score matches remain proposals, cross-user media/GPS access denied, API schema digest drift and missing connector registration fail release. | MEDULLA-UNIFIED-DATA-PLANE-R030, MEDULLA-UNIFIED-DATA-PLANE-R032 |
| Kernel dependency: bursty storage | Consume the `durable-graph-kernel` exact-head native point/KV, durability, batch, recovery and DepClock proof before admitting a dependent app. | Missing authz/HMAC, stale cached plan or partial batch ack blocks app admission; this row does not implement a second kernel. | Kernel-owned MEDULLA-DURABLE-KERNEL-R032/MEDULLA-DURABLE-KERNEL-R037–MEDULLA-DURABLE-KERNEL-R041 |
| Kernel dependency: benchmarks | Consume the same-hardware raw benchmark receipt from `durable-graph-kernel`. | A missing target, changed class, unreported gap or unrepeatable run blocks a comparative claim. | Kernel-owned MEDULLA-DURABLE-KERNEL-R042 |
| Virtual-graph metadata | A registered API, MCP and SQL source each yield one hashed `MetadataContract`. An approved mapping answers a live read. A hot mapping copies only its declared subset. | Credential-bearing reference, undiscovered field, unapproved mapping, missing hot-subset predicate and stale watermark each refuse or fall back to live reads. | MEDULLA-UNIFIED-DATA-PLANE-R037, MEDULLA-UNIFIED-DATA-PLANE-R038 |

## Reproducible contributor environment

Use a current Rust toolchain from `rust-toolchain.toml` if present, Python from `pyproject.toml`, Docker with Compose, and the repository lockfiles. Run commands from the Medulla repository root. Build the same feature set named by each implementation PR; record `git rev-parse HEAD`, `rustc --version`, `python3 --version`, `docker compose version`, container image digests and test command. Sample data must be synthetic, deterministic and safe to publish. Never use a real family tree, photo library, account or credential as a test fixture.

For a base SQL adapter fixture, save the following as a temporary `compose.yaml` outside the repository and run `docker compose -p medulla-data-plane -f compose.yaml up -d --wait`. Use isolated test credentials and teardown with `docker compose -p medulla-data-plane -f compose.yaml down -v`. CI must pin image digests and versions in its committed matrix before it can claim a green dialect row. The versions here are a reproducible local starting point.

```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: egtest
      POSTGRES_PASSWORD: egtest-local-only
      POSTGRES_DB: egtest
    ports: ["127.0.0.1:15432:5432"]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U egtest -d egtest"]
      interval: 2s
      retries: 30
  mariadb:
    image: mariadb:11.4
    environment:
      MARIADB_ROOT_PASSWORD: egtest-local-only
      MARIADB_USER: egtest
      MARIADB_PASSWORD: egtest-local-only
      MARIADB_DATABASE: egtest
    ports: ["127.0.0.1:13306:3306"]
    healthcheck:
      test: ["CMD-SHELL", "mariadb-admin ping -h localhost -u root -pegtest-local-only"]
      interval: 2s
      retries: 30
```

Create a SQLite fixture with Python's `sqlite3` module using a normal table, a `WITHOUT ROWID` table, JSON text, nulls, Unicode and a foreign key. For each adapter add a committed schema/data fixture covering primary and composite keys, no-key tables, generated IDs, nullable columns, decimal/timezone/array or document types where supported, DDL change and a known capture sequence. The conformance harness must itself create/reset these fixtures. A test must not depend on a developer's running application.

## Required acceptance artifacts

Each dialect entry needs a machine-readable capability declaration, image/driver version, source schema fixture, Medulla result, native result, canonical comparison rule, pushed-down SQL/EXPLAIN and capture replay result or explicit no-capture reason. Compare column type, value, SQLSTATE and row order when ordered; normalize only documented nondeterminism. Cover null ordering, numeric precision/overflow, integer division, implicit casts, LIKE escaping, timestamps/time zones, JSON path, `DISTINCT ON`, functional-dependence grouping and boolean text output. Generate separate reports for Postgres regression queries, generated TLP/NoREC partition cases and captured synthetic app traffic. A known difference is a reviewed named deviation with owner and failing fixture, never a self-updating baseline.

For native hosting, the synthetic Gramps-like fixture must include person/family/event relations, JSON object payloads, an inferred `reference` relation, indexes and a sample import/export digest. P0–P5 artifacts are: baseline digest/counts; real-Postgres control digest and driver log; Medulla failure corpus with SQLSTATE; post-fix differential report; independent replica/dump/Medulla-backup restore digests plus kill-at-commit report; approved R2RML, live SPARQL answer and `schema_context` result. P6 adds the explicit decision, rehearsed failover, independent export and 30-day observation. If the app's real traffic is used privately to find a gap, publish a minimized synthetic reproducer before accepting the fix.

For Immich, commit a synthetic OpenAPI fixture digest and generator output test, read/write MCP fixtures, per-user ownership fixtures, a no-op replay and one-change incremental fixture. A connector registration test must enumerate every required release catalog and reject a missing site. Entity-resolution fixtures must include true match, same-name false match, unnamed face, missing birth date, cross-user image and unapproved candidate; no `owl:sameAs` becomes active before approval.

For CDC, fail at each snapshot-to-stream boundary, before and after cursor/effect commit, and during source failover. Check equal final snapshot, once-only derived effect, visible lag, bounded retention and mapping suspension on DDL drift. For mirrors, stop a sink, advance source, restart, reconcile a deliberately corrupted target and restore SQL tables to real Postgres. For native transactions, inject process termination at each table/graph commit boundary and compare post-restart state. For bursty durability, repeat abrupt termination under all three classes, measure actual async acknowledged-loss interval against ≤100 ms, and show that sync never acknowledges a lost mutation. Report p99, throughput, memory and restart recovery for each class rather than mixing them.

The benchmark report must name host CPU/RAM/storage, kernel, compiler, container digests, dataset size, load/concurrency, warm-up, run length, durability class, raw outputs and confidence interval. The target axes are: YCSB A–F beats Postgres in async class; RESP p99 within 2× Redis in ephemeral class; pgbench and HammerDB TPC-C report their gap; LDBC SNB Interactive reports graph performance; and a converged graph+vector+SQL+time-series query beats the same query assembled from Postgres with relevant extensions. A claimed win must use the same hardware and comparable durability and disclose failures. No claim is allowed for an unrun row.

## Program gates

At the exact proposed merge head run scoped Rust/Python tests, per-dialect container conformance, `pre-commit run --config .config/pre-commit.yaml --all-files`, `bash scripts/ci_parity.sh`, `python3 scripts/check_status_page.py` and the hosted CI matrix. Capture the CCCC, KISS, jscpd and Dupehound reports from their real configured gates. Record route traces, replay positions, before/after dependency and LOC/symbol counts, old-path deletion, and a restore drill. If a gate cannot run, keep the item `IMPLEMENTED-UNVALIDATED` with the limitation. A green documentation check cannot turn an unimplemented row into `ACCEPTED`.

Authenticated Iceberg catalog (R039): a valid token lists and loads its own tables. Missing and cross-tenant tokens are refused on every route. A partitioned fixture prunes one manifest by a partition predicate, and a pinned external reader loads one table.

### MEDULLA-UNIFIED-DATA-PLANE-R040 tests

- `MEDULLA-UNIFIED-DATA-PLANE-R040.1`: `crates/medulla-types/src/embedding_promotion.rs::tests::checks_round_trip`.
- `MEDULLA-UNIFIED-DATA-PLANE-R040.2`: `crates/medulla-types/src/embedding_promotion.rs::tests::one_regression_blocks_promotion`.
- `MEDULLA-UNIFIED-DATA-PLANE-R040.3`: `crates/medulla-ann/src/version.rs::tests::tag_requires_passing_verdict`.
- `MEDULLA-UNIFIED-DATA-PLANE-R040.4`: `crates/medulla-ann/src/version.rs::tests::blocked_promotion_tags_nothing`.
- `MEDULLA-UNIFIED-DATA-PLANE-R040.5`: `crates/medulla-types/src/embedding_promotion.rs::tests::verdict_is_stored_with_modality_entries`.
- `MEDULLA-UNIFIED-DATA-PLANE-R040.6`: `tests/embedding/test_promotion_gate_served.py::test_regressed_modality_blocks_promotion`.

<!-- spec-definition-patch:owner-batch-20261012-unified-data-plane -->
## Planned owner acceptance — 2026-10-12

These bullets define future acceptance targets. They are not test evidence or receipts. Exercise each row's actual producer/consumer boundary and retain existing covered behavior.

- **`MEDULLA-UNIFIED-DATA-PLANE-R026.4`**: EG:64; source SHA-256 `c55bc629b1bfefc9d0346b75b8bb95e5a231b891e7e34297df17b960d6c9773c`.
  - Read a mapped table through a served external client using an approved, pinned mapping. Bind schema/graph revisions and row digest.
  - Attempt INSERT, UPDATE, DELETE, and DDL through read-only and mutation-granted principals. Refuse each attempt and preserve graph revision.
  - Submit an authorized graph `MutationBatch`. Confirm the projection read reflects its committed revision.
  - Refuse stale/unapproved mappings and unauthorized graph mutations.
  - Hold application-level SQL/schema conformance until the R026 owner provides the approved target mapping and captured statement corpus.
  - Preserve MEDULLA-DURABLE-KERNEL-R035, MEDULLA-DURABLE-KERNEL-R043, MEDULLA-FEDERATED-QUERY-R080, MEDULLA-UNIFIED-DATA-PLANE-R026, and MEDULLA-FEDERATED-QUERY-R080.1 through R080.6.
<!-- /spec-definition-patch -->

## MEDULLA-UNIFIED-DATA-PLANE-R038.3 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.1` | `crates/medulla-types/src/metadata_acceleration.rs::tests::snapshot_semantics_round_trip`: A serde round trip keeps every field and rejects an unknown category or isolation. |

| Behavior | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.2` | `crates/medulla-federation/src/federation_opt/tests.rs::fragments_share_snapshot`: Two fragments of one query read one snapshot while a concurrent writer commits. |

| Wiring | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.3` | `crates/medulla-federation/src/federation_opt/tests.rs::foreign_scan_opens_snapshot`: A served query over a transactional source reaches the snapshot path. |

| Refusal | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.4` | `crates/medulla-federation/src/federation_opt/tests.rs::snapshot_refusals`: Each case returns its code and runs no source statement. |

| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.5` | `crates/medulla-federation/src/federation_opt/tests.rs::provenance_names_snapshot`: The answer provenance names one snapshot token per source. |

| Hot-path proof | `MEDULLA-UNIFIED-DATA-PLANE-R038.3.6` | `tests/federation/test_oltp_snapshot_served.py::test_fragments_share_one_snapshot`: The answer matches the snapshot oracle and the engine row count for that source stays unchanged. |


Hot-path proof for `MEDULLA-UNIFIED-DATA-PLANE-R038.3`: `tests/federation/test_oltp_snapshot_served.py::test_fragments_share_one_snapshot`.

## MEDULLA-UNIFIED-DATA-PLANE-R038.4 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.1` | `crates/medulla-lake/tests/metastore.rs::metastore_spec_round_trips`: A serde round trip keeps every field and rejects an unknown format. |

| Behavior | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.2` | `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan`: A pushed predicate reads only the matching partitions and stripes. |

| Wiring | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.3` | `tests/federation/test_metastore_orc_avro_served.py::test_hive_function_registered_and_scoped`: A served query over a metastore table reaches the pruned scan. |

| Refusal | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.4` | `crates/medulla-lake/tests/metastore.rs::metastore_refusals`: Each case returns its code and reads no data file. |

| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.5` | `crates/medulla-lake/tests/metastore.rs::receipt_counts_match`: The receipt counts match the files the scan opened. |

| Hot-path proof | `MEDULLA-UNIFIED-DATA-PLANE-R038.4.6` | `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan`: The answer matches the full-scan oracle and the receipt shows pruning. |


Hot-path proof for `MEDULLA-UNIFIED-DATA-PLANE-R038.4`: `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan`.

## MEDULLA-UNIFIED-DATA-PLANE-R038.5 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.1` | `crates/medulla-types/src/search_source.rs::tests::search_capability_round_trips`: A serde round trip keeps every field and rejects an unknown score kind. |

| Behavior | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.2` | `crates/medulla-federation/src/federation_opt/tests.rs::relevance_pushdown_replenishes`: A pushed query returns k results with remote scores in their channel. |

| Wiring | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.3` | `crates/medulla-federation/src/federation_opt/tests.rs::served_search_reaches_fusion`: A served query reaches the remote index and the shared fusion. |

| Refusal | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.4` | `crates/medulla-federation/src/federation_opt/tests.rs::search_refusals`: Each case returns its code or pages, and no result carries an invented score. |

| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.5` | `crates/medulla-federation/src/federation_opt/tests.rs::search_receipt`: The receipt matches the remote request. |

| Hot-path proof | `MEDULLA-UNIFIED-DATA-PLANE-R038.5.6` | `tests/federation/test_search_index_pushdown_served.py::test_relevance_pushed_and_k_replenished`: The answer holds k results, and the engine stores no index documents. |


Hot-path proof for `MEDULLA-UNIFIED-DATA-PLANE-R038.5`: `tests/federation/test_search_index_pushdown_served.py::test_relevance_pushed_and_k_replenished`.

## MEDULLA-UNIFIED-DATA-PLANE-R038.6 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-UNIFIED-DATA-PLANE-R038.6.1` | `crates/medulla-types/src/virtual_graph_view.rs::tests::graph_view_round_trips`: A serde round trip keeps every field and rejects an unmapped label. |

| Behavior | `MEDULLA-UNIFIED-DATA-PLANE-R038.6.2` | `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::pattern_pushes_join`: One pattern renders one pushed SQL statement per same-source join. |

| Wiring | `MEDULLA-UNIFIED-DATA-PLANE-R038.6.3` | `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::served_read_resolves_view`: A served openCypher read over a mapped source reaches the view resolution. |

| Refusal | `MEDULLA-UNIFIED-DATA-PLANE-R038.6.4` | `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::view_refusals`: Each case returns its code and runs no source statement. |

| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R038.6.5` | `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::explain_names_mapping`: EXPLAIN names the mapping digest of each fragment. |

| Hot-path proof | `MEDULLA-UNIFIED-DATA-PLANE-R038.6.6` | `tests/federation/test_graph_language_virtual_served.py::test_cypher_matches_sparql_over_sql_source`: The two answers are equal, and the engine row count for that source stays unchanged. |


Hot-path proof for `MEDULLA-UNIFIED-DATA-PLANE-R038.6`: `tests/federation/test_graph_language_virtual_served.py::test_cypher_matches_sparql_over_sql_source`.

## MEDULLA-UNIFIED-DATA-PLANE-R022.4 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R022.4` | `crates/medulla-types/src/dialect_conformance.rs::tests::matrix_covers_every_source_category`: A unit test fails when a category lacks a row. It reports `unproven` for a category without a passing proof. |

## MEDULLA-UNIFIED-DATA-PLANE-R038.7 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.1` | `crates/medulla-types/src/web_source.rs::tests::web_result_round_trips`: A serde round trip keeps every field and rejects a result without a URL. |

| Behavior | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.2` | `crates/medulla-federation/src/federation_opt/tests.rs::web_search_pushdown_and_ttl`: A repeated pattern within the time-to-live sends one connector query. |

| Wiring | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.3` | `crates/medulla-federation/src/federation_opt/tests.rs::served_web_search_reaches_cache`: A served pattern over web results reaches the connector operation and the shared cache. |

| Refusal | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.4` | `crates/medulla-federation/src/federation_opt/tests.rs::web_search_refusals`: Each case returns its code, and no expired row serves a read. |

| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.5` | `crates/medulla-federation/src/federation_opt/tests.rs::web_result_provenance`: Each row names its engine, rank and retrieval time, and a cached row says so. |

| Hot-path proof | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.6` | `tests/federation/test_web_metasearch_served.py::test_pattern_pushes_to_connector_with_ttl_cache`: The connector receives two queries, the rows are typed, and the graph store holds no result rows. |


Hot-path proof for `MEDULLA-UNIFIED-DATA-PLANE-R038.7`: `tests/federation/test_web_metasearch_served.py::test_pattern_pushes_to_connector_with_ttl_cache`.

## MEDULLA-UNIFIED-DATA-PLANE-R651.7 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Matrix | `MEDULLA-UNIFIED-DATA-PLANE-R022.4.1` | `crates/medulla-types/src/dialect_conformance.rs::tests::matrix_states_residency`: The test fails when a category row has no residency value. |
| Candidates | `MEDULLA-UNIFIED-DATA-PLANE-R038.7.7` | `crates/medulla-federation/src/federation_opt/tests.rs::web_candidates_carry_query_provenance`: Candidates carry query provenance, and the store holds no result rows. |
| Model | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.1` | `web_document.rs::tests::web_provenance_round_trips`: A serde round trip keeps every field and rejects a missing Document ID or an unknown backend. |
| Behavior | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.2` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_web_ingest_tests.rs::links_chunks_vectors_and_facts`: One Document links its chunks and vectors; an equal digest is a no-op, and a new digest supersedes. |
| Wiring | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.3` | `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`: The served call commits chunks, edges and vectors, and `SemanticSearch` returns a chunk. |
| Refusal | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.4` | `tests/web_document_enrich_served.rs::test_web_document_refusals`: Each case returns its code and writes nothing. |
| Evidence | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.5` | `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`: The receipt counts match a separate read of the chunks. |
| Hot-path proof | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.6` | `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`: Both backends store equal chunk sets, and `document_chunk_tree` meets the budgets. |
| Extraction | `MEDULLA-UNIFIED-DATA-PLANE-R651.7.7` | `tests/ingest/test_web_document_extraction_served.py::test_enriched_chunks_extract_facts`: Each extracted fact links to a chunk of the Document. |

Hot-path proof for `MEDULLA-UNIFIED-DATA-PLANE-R651.7`: `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`.

## Profiling, data quality, column lineage and asset usage

| Row | Proving test |
|---|---|
| `MEDULLA-UNIFIED-DATA-PLANE-R761` | See each child. |
| `MEDULLA-UNIFIED-DATA-PLANE-R761.1` | `crates/medulla-types/tests/profile.rs::profile_round_trips` |
| `MEDULLA-UNIFIED-DATA-PLANE-R761.2` | `crates/medulla-compute/src/sketch.rs::tests::hll_precision_14_within_error_bound` |
| `MEDULLA-UNIFIED-DATA-PLANE-R761.3` | `tests/data_plane/test_profile_served.py::test_profile_job_attaches_to_catalog` |
| `MEDULLA-UNIFIED-DATA-PLANE-R761.4` | `tests/data_plane/test_profile_served.py::test_profile_refusals` |
| `MEDULLA-UNIFIED-DATA-PLANE-R761.5` | `tests/data_plane/test_profile_served.py::test_profile_method_recorded` |
| `MEDULLA-UNIFIED-DATA-PLANE-R761.6` | `cargo bench --bench column_profile` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762` | See each child. |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.1` | `crates/medulla-types/tests/quality.rs::rule_round_trips` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.2` | `crates/medulla-core/src/quality.rs::tests::three_failures_open_one_incident` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.3` | `tests/data_plane/test_quality_served.py::test_rule_scores_and_incident` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.4` | `tests/data_plane/test_quality_served.py::test_quality_refusals` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.5` | `tests/data_plane/test_quality_served.py::test_score_evidence_reads_back` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.6` | `cargo bench --bench quality_evaluate` |
| `MEDULLA-UNIFIED-DATA-PLANE-R763` | See each child. |
| `MEDULLA-UNIFIED-DATA-PLANE-R763.1` | `crates/medulla-types/tests/lineage.rs::column_edge_round_trips` |
| `MEDULLA-UNIFIED-DATA-PLANE-R763.2` | `crates/medulla-federation/src/column_lineage.rs::tests::view_columns_resolve` |
| `MEDULLA-UNIFIED-DATA-PLANE-R763.3` | `tests/data_plane/test_column_lineage_served.py::test_view_lineage_and_impact` |
| `MEDULLA-UNIFIED-DATA-PLANE-R763.4` | `tests/data_plane/test_column_lineage_served.py::test_unparseable_sql_quarantines` |
| `MEDULLA-UNIFIED-DATA-PLANE-R763.5` | `tests/data_plane/test_column_lineage_served.py::test_edge_evidence_reads_back` |
| `MEDULLA-UNIFIED-DATA-PLANE-R763.6` | `cargo bench --bench column_lineage` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764` | See each child. |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.1` | `crates/medulla-types/tests/asset_usage.rs::usage_round_trips` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.2` | `crates/medulla-types/tests/asset_usage.rs::usage_prior_is_capped` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.3` | Retired; see MEDULLA-TYPED-PACKS-R108.16. |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.4` | `tests/data_plane/test_catalog_search_served.py::test_search_refusals` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.5` | `tests/data_plane/test_catalog_search_served.py::test_hit_components_listed` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.6` | `cargo bench --bench catalog_search` |

## Source query-log intelligence, rule proposals and outside quality results

| Row | Proving test |
|---|---|
| `MEDULLA-UNIFIED-DATA-PLANE-R762.7` | `crates/medulla-types/tests/quality.rs::profile_yields_proposed_rules` |
| `MEDULLA-UNIFIED-DATA-PLANE-R762.8` | `tests/data_plane/test_quality_served.py::test_outside_scores_open_incident` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.7` | `tests/data_plane/test_catalog_search_served.py::test_source_query_log_feeds_usage` |
| `MEDULLA-UNIFIED-DATA-PLANE-R764.8` | `tests/data_plane/test_catalog_search_served.py::test_frequent_users_and_join_suggestions` |

## Planned bindings

These entries name tests that do not exist yet. Each is the exact post-split path of the test that proves its row; none is test evidence.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.2:** `crates/medulla-federation/src/federation_opt/tests.rs::fragments_share_snapshot` (planned). Proves the row: Behavior: one read-only transaction per source per query.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.3:** `crates/medulla-federation/src/federation_opt/tests.rs::foreign_scan_opens_snapshot` (planned). Proves the row: Wiring: `foreign_scan` opens the snapshot and the capability row declares it.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.3:** `tests/federation/test_oltp_snapshot_served.py::test_fragments_share_one_snapshot` (planned). Proves the row: Wiring: `foreign_scan` opens the snapshot and the capability row declares it.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.4:** `crates/medulla-federation/src/federation_opt/tests.rs::snapshot_refusals` (planned). Proves the row: Refusal: no snapshot, stale replica, or reachable write path.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.4:** `tests/federation/test_oltp_snapshot_served.py::test_snapshot_refusals` (planned). Proves the row: Refusal: no snapshot, stale replica, or reachable write path.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.5:** `crates/medulla-federation/src/federation_opt/tests.rs::provenance_names_snapshot` (planned). Proves the row: Evidence: EXPLAIN and provenance name the snapshot.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.5:** `tests/federation/test_oltp_snapshot_served.py::test_provenance_names_one_token` (planned). Proves the row: Evidence: EXPLAIN and provenance name the snapshot.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.6:** `tests/federation/test_oltp_snapshot_served.py::test_fragments_share_one_snapshot` (planned). Proves the row: Hot-path proof: a concurrent writer does not split the answer.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.3.6:** `tests/federation/conftest.py::pg_source` (planned). Proves the row: Hot-path proof: a concurrent writer does not split the answer.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4:** `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan` (planned). Proves the row: Metastore catalogs and ORC and Avro files answer in place (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.1:** `crates/medulla-lake/tests/metastore.rs::metastore_spec_round_trips` (planned). Proves the row: Model: metastore spec, file formats and table metadata.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.1:** `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan` (planned). Proves the row: Model: metastore spec, file formats and table metadata.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.2:** `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan` (planned). Proves the row: Behavior: catalog listing, partition pruning, ORC and Avro reads (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.3:** `tests/federation/test_metastore_orc_avro_served.py::test_hive_function_registered_and_scoped` (planned). Proves the row: Wiring: the `hive()` table function and discovery.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.4:** `crates/medulla-lake/tests/metastore.rs::metastore_refusals` (planned). Proves the row: Refusal: schema mismatch, unsupported layout, unreachable catalog.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.5:** `crates/medulla-lake/tests/metastore.rs::receipt_counts_match` (planned). Proves the row: Evidence: the scan receipt counts partitions, files, stripes and bytes.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.6:** `tests/federation/test_metastore_orc_avro_served.py::test_partition_pruned_scan` (planned). Proves the row: Hot-path proof: a pruned scan over a recorded metastore.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5:** `tests/federation/test_search_index_pushdown_served.py::test_relevance_pushed_and_k_replenished` (planned). Proves the row: Search-index sources answer with relevance pushdown (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.1:** `crates/medulla-types/src/search_source.rs::tests::search_capability_round_trips` (planned). Proves the row: Model: `SearchIndexCapability` on the operation-bound source.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.1:** `tests/federation/test_search_index_pushdown_served.py::test_relevance_pushed_and_k_replenished` (planned). Proves the row: Model: `SearchIndexCapability` on the operation-bound source.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.2:** `crates/medulla-federation/src/federation_opt/tests.rs::relevance_pushdown_replenishes` (planned). Proves the row: Behavior: push text match, nearest-neighbour search, filters and top-k.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.3:** `crates/medulla-federation/src/federation_opt/tests.rs::served_search_reaches_fusion` (planned). Proves the row: Wiring: the search remote joins the shared fusion.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.3:** `tests/federation/test_search_index_pushdown_served.py::test_remote_branch_fuses_with_local` (planned). Proves the row: Wiring: the search remote joins the shared fusion.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.4:** `crates/medulla-federation/src/federation_opt/tests.rs::search_refusals` (planned). Proves the row: Refusal: vector-space mismatch, page overflow, invented scores.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.5:** `crates/medulla-federation/src/federation_opt/tests.rs::search_receipt` (planned). Proves the row: Evidence: each fragment receipt names the index request.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.5.6:** `tests/federation/test_search_index_pushdown_served.py::test_relevance_pushed_and_k_replenished` (planned). Proves the row: Hot-path proof: pushed relevance with k replenished.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6:** `tests/federation/test_graph_language_virtual_served.py::test_cypher_matches_sparql_over_sql_source` (planned). Proves the row: openCypher answers over relational and lake sources through the virtual view, without a copy (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6.2:** `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::pattern_pushes_join` (planned). Proves the row: Behavior: compile node and relationship patterns to the virtual SPARQL path.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6.3:** `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::served_read_resolves_view` (planned). Proves the row: Wiring: a named view routes the read through view compilation.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6.3:** `tests/federation/test_graph_language_virtual_served.py::test_view_is_owner_scoped` (planned). Proves the row: Wiring: a named view routes the read through view compilation.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6.4:** `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::view_refusals` (planned). Proves the row: Refusal: view writes, unmapped labels, over-deep paths, unsupported clauses.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6.5:** `crates/medulla-cypher/src/cypher/tests/virtual_view.rs::explain_names_mapping` (planned). Proves the row: Evidence: EXPLAIN names the pushed statement and mapping digest per fragment.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.6.6:** `tests/federation/test_graph_language_virtual_served.py::test_cypher_matches_sparql_over_sql_source` (planned). Proves the row: Hot-path proof: openCypher and SPARQL agree over one relational source.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7:** `tests/federation/test_web_metasearch_served.py::test_pattern_pushes_to_connector_with_ttl_cache` (planned). Proves the row: Web metasearch sources answer at query time through a cached connector query (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.1:** `crates/medulla-types/src/web_source.rs::tests::web_result_round_trips` (planned). Proves the row: Model: `WebSearchCapability` and `WebResult`.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.1:** `tests/federation/test_web_metasearch_served.py::test_pattern_pushes_to_connector_with_ttl_cache` (planned). Proves the row: Model: `WebSearchCapability` and `WebResult`.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.2:** `crates/medulla-federation/src/federation_opt/tests.rs::web_search_pushdown_and_ttl` (planned). Proves the row: Behavior: push query arguments and cache by argument digest with a time-to-live.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.3:** `crates/medulla-federation/src/federation_opt/tests.rs::served_web_search_reaches_cache` (planned). Proves the row: Wiring: web search reaches the connector call and the shared cache.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.3:** `tests/federation/test_web_metasearch_served.py::test_cache_is_owner_scoped` (planned). Proves the row: Wiring: web search reaches the connector call and the shared cache.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.4:** `crates/medulla-federation/src/federation_opt/tests.rs::web_search_refusals` (planned). Proves the row: Refusal: unsupported arguments, expired entries, unreachable connector.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.5:** `crates/medulla-federation/src/federation_opt/tests.rs::web_result_provenance` (planned). Proves the row: Evidence: each row carries connector, argument digest, engine, rank and retrieval time.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.6:** `tests/federation/test_web_metasearch_served.py::test_pattern_pushes_to_connector_with_ttl_cache` (planned). Proves the row: Hot-path proof: one connector call inside the TTL.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.7:** `crates/medulla-federation/src/federation_opt/tests.rs::web_candidates_carry_query_provenance` (planned). Proves the row: Candidate handoff: web results are fetch candidates, not stored records.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.7.7:** `tests/federation/test_web_metasearch_served.py::test_candidates_not_stored` (planned). Proves the row: Candidate handoff: web results are fetch candidates, not stored records.
- **MEDULLA-UNIFIED-DATA-PLANE-R039.3:** `crates/medulla-server-lake/src/lake/rest/mod.rs::tests::every_route_has_a_scope_row` (planned). Proves the row: Every catalog route passes the carrier gate.
- **MEDULLA-UNIFIED-DATA-PLANE-R039.4:** `crates/medulla-lake/tests/ltap.rs::partition_spec_emitted_and_manifest_pruned` (planned). Proves the row: Partition spec and per-partition field summaries in emitted Iceberg metadata.
- **MEDULLA-UNIFIED-DATA-PLANE-R039.4:** `tests/test_lake_iceberg_delta_parity.py::test_partitioned_table_reads_in_pyiceberg` (planned). Proves the row: Partition spec and per-partition field summaries in emitted Iceberg metadata.
- **MEDULLA-UNIFIED-DATA-PLANE-R039.5:** `tests/test_lake_iceberg_delta_parity.py::test_iceberg_rest_external_read_and_cross_tenant_denied` (planned). Proves the row: End-to-end external read through the authenticated catalog.
- **MEDULLA-UNIFIED-DATA-PLANE-R040:** `tests/embedding/test_promotion_gate_served.py::test_regressed_modality_blocks_promotion` (planned). Proves the row: A per-modality retrieval gate blocks promotion of a shared embedding space (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R040.3:** `crates/medulla-ann/src/version.rs::tests::tag_requires_passing_verdict` (planned). Proves the row: Wiring: the gate runs before a binding with a new model version goes `Live`.
- **MEDULLA-UNIFIED-DATA-PLANE-R040.3:** `crates/medulla-h-retrieve/src/semantic_index/promotion.rs::tests::transition_runs_gate` (planned). Proves the row: Wiring: the gate runs before a binding with a new model version goes `Live`.
- **MEDULLA-UNIFIED-DATA-PLANE-R040.4:** `crates/medulla-ann/src/version.rs::tests::blocked_promotion_tags_nothing` (planned). Proves the row: Refusal: a blocked promotion leaves the prior version active.
- **MEDULLA-UNIFIED-DATA-PLANE-R040.5:** `crates/medulla-types/src/embedding_promotion.rs::tests::verdict_is_stored_with_modality_entries` (planned). Proves the row: Evidence: each `PromotionVerdict` is a graph node linked to the model and version.
- **MEDULLA-UNIFIED-DATA-PLANE-R040.5:** `tests/embedding/test_promotion_gate_served.py::test_verdict_readable_by_query` (planned). Proves the row: Evidence: each `PromotionVerdict` is a graph node linked to the model and version.
- **MEDULLA-UNIFIED-DATA-PLANE-R040.6:** `tests/embedding/test_promotion_gate_served.py::test_regressed_modality_blocks_promotion` (planned). Proves the row: Hot-path proof: a regressed image check blocks a served promotion.
- **MEDULLA-UNIFIED-DATA-PLANE-R026.4:** `tests/projection_read_served.rs::projected_table_read_binds_revisions_and_refuses_writes` (planned). Proves the row: Graph-backed relational projections, read-only (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R026.4.1:** `tests/projection_read_served.rs::projected_read_binds_revisions` (planned). Proves the row: Projection read binds mapping and graph revisions.
- **MEDULLA-UNIFIED-DATA-PLANE-R026.4.2:** `tests/projection_read_served.rs::projected_writes_refuse_for_both_principals` (planned). Proves the row: Projected tables refuse every SQL write.
- **MEDULLA-UNIFIED-DATA-PLANE-R026.4.3:** `tests/projection_read_served.rs::authorized_mutation_is_visible_in_next_read` (planned). Proves the row: Graph changes reach the projection only through MutationBatch.
- **MEDULLA-UNIFIED-DATA-PLANE-R750:** `tests/data_plane/test_history_search_served.py::test_search_is_scope_filtered_before_ranking` (planned). Proves the row: Conversation history source, run audit rows and step store (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R750.753:** `tests/data_plane/test_history_search_served.py::test_search_is_scope_filtered_before_ranking` (planned). Proves the row: Hot-path proof: scoped history search over one million steps.
- **MEDULLA-UNIFIED-DATA-PLANE-R651:** `tests/ingest/test_document_delete_served.py::test_delete_cascades_and_leaves_no_orphan` (planned). Proves the row: A document delete cascades across the record, vectors and graph with no orphan (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R651.1:** `crates/medulla-document/tests/cascade.rs::cascade_types_round_trip` (planned). Proves the row: Model: cascade plan, orphan report and delete receipt.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.1:** `tests/ingest/test_document_delete_served.py::test_delete_cascades_and_leaves_no_orphan` (planned). Proves the row: Model: cascade plan, orphan report and delete receipt.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.2:** `crates/medulla-document/tests/cascade.rs::plan_is_deterministic_and_complete` (planned). Proves the row: Behavior: plan from back-references, apply as one batch, verify the orphan report.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.3:** `tests/ingest/test_document_delete_served.py::test_method_registered_and_scoped` (planned). Proves the row: Wiring: the served `DocumentDelete` method reaches the cascade.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.4:** `tests/ingest/test_document_delete_served.py::test_delete_refusals_change_nothing` (planned). Proves the row: Refusal: missing document, live external references, oversized or partial cascade.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.5:** `tests/ingest/test_document_delete_served.py::test_deletion_record_readable` (planned). Proves the row: Evidence: the delete receipt and a content-free deletion record.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.6:** `tests/ingest/test_document_delete_served.py::test_delete_cascades_and_leaves_no_orphan` (planned). Proves the row: Hot-path proof: a 500-chunk delete leaves no orphan.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.7.1:** `web_document.rs::tests::web_provenance_round_trips` (planned). Proves the row: Model.
- **MEDULLA-UNIFIED-DATA-PLANE-R651.7.7:** `tests/ingest/test_web_document_extraction_served.py::test_enriched_chunks_extract_facts` (planned). Proves the row: Extraction: a worker submits facts for the enriched chunks and the engine grounds them.
- **MEDULLA-UNIFIED-DATA-PLANE-R700:** `tests/data_plane/test_actions_served.py::test_failed_criterion_applies_no_effect` (planned). Proves the row: Governed Actions: typed parameters, submission criteria, effects and write-back (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R700.2:** `crates/medulla-work-types/src/action_spec.rs::tests::template_resolution_is_single_pass` (planned). Proves the row: Behavior: parameters, ordered criteria, one-pass templates, one mutation batch.
- **MEDULLA-UNIFIED-DATA-PLANE-R700.2:** `crates/medulla-work-types/src/action_spec.rs::tests::criteria_stop_at_first_failure` (planned). Proves the row: Behavior: parameters, ordered criteria, one-pass templates, one mutation batch.
- **MEDULLA-UNIFIED-DATA-PLANE-R700.3:** `tests/data_plane/test_actions_served.py::test_register_submit_and_outbox_event` (planned). Proves the row: Wiring: `ActionRegister` and `ActionSubmit` reach the apply path and the outbox.
- **MEDULLA-UNIFIED-DATA-PLANE-R700.6:** `tests/data_plane/test_actions_served.py::test_failed_criterion_applies_no_effect` (planned). Proves the row: Hot-path proof: a failed criterion applies no effect.
- **MEDULLA-UNIFIED-DATA-PLANE-R701:** `tests/data_plane/test_standards_drift_served.py::test_missing_standard_is_not_zero` (planned). Proves the row: Standards conformance: cohorts, drift scores and consolidation recommendations (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R701.1:** `crates/medulla-catalog-types/src/standards.rs::tests::standards_types_round_trip` (planned). Proves the row: Model.
- **MEDULLA-UNIFIED-DATA-PLANE-R701.2:** `crates/medulla-core/src/standards.rs::tests::drift_and_cohorts_are_single_pass` (planned). Proves the row: Behavior.
- **MEDULLA-UNIFIED-DATA-PLANE-R701.3:** `tests/data_plane/test_standards_drift_served.py::test_cross_application_recommendation_is_a_proposal` (planned). Proves the row: Wiring: `StandardsDrift` reads the graph and stays a proposal across attached applications.
- **MEDULLA-UNIFIED-DATA-PLANE-R701.4:** `crates/medulla-core/src/standards.rs::tests::cross_tenant_refused_and_unscored_is_not_applicable` (planned). Proves the row: Refusal.
- **MEDULLA-UNIFIED-DATA-PLANE-R701.5:** `crates/medulla-core/src/standards.rs::tests::score_lists_satisfied_and_failed_rules` (planned). Proves the row: Evidence.
- **MEDULLA-UNIFIED-DATA-PLANE-R701.6:** `tests/data_plane/test_standards_drift_served.py::test_missing_standard_is_not_zero` (planned). Proves the row: Hot-path proof: a missing standard scores unscored, never zero.
- **MEDULLA-UNIFIED-DATA-PLANE-R702:** `tests/data_plane/test_lineage_drain_served.py::test_replay_adds_no_edges_and_bad_events_quarantine` (planned). Proves the row: ETL runs record lineage and consume external lineage run events (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R702.3:** `tests/data_plane/test_lineage_drain_served.py::test_lake_sweep_run_event_reaches_lineage_graph` (planned). Proves the row: Wiring: `LineageIngest` is called by the lake sweep and the arbora route; lineage reads are plain graph queries.
- **MEDULLA-UNIFIED-DATA-PLANE-R702.6:** `tests/data_plane/test_lineage_drain_served.py::test_replay_adds_no_edges_and_bad_events_quarantine` (planned). Proves the row: Hot-path proof: replay adds no edges and malformed events quarantine.
- **MEDULLA-UNIFIED-DATA-PLANE-R710:** `crates/medulla-durable/tests/stream_adapter.rs::mapping_is_pure_and_cursor_is_stable` (planned). Proves the row: Message-stream records normalize into a `ChangeEnvelope`: model and behavior.
- **MEDULLA-UNIFIED-DATA-PLANE-R711:** `tests/data_plane/test_stream_adapters_served.py::test_checkpoint_returns_after_durable_write` (planned). Proves the row: Message-stream records normalize into a `ChangeEnvelope`: refusal, evidence and hot-path proof.
- **MEDULLA-UNIFIED-DATA-PLANE-R030.701:** `tests/data_plane/test_identity_evidence_served.py::test_high_entropy_names_do_not_merge` (planned). Proves the row: Identity evidence kinds and merge decisions: refusal, evidence and hot path.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.2.1:** `tests/federation/test_acceleration_served.py::test_registration_writes_contract_only` (planned). Proves the row: Discovery writes one `MetadataContract` per registered source kind.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.2.2:** `crates/medulla-types/src/metadata_acceleration.rs::tests::expired_watermark_falls_back_to_live` (planned). Proves the row: The copy route is bounded by rows and by watermark.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.2.2:** `tests/federation/test_acceleration_served.py::test_stale_watermark_reads_live` (planned). Proves the row: The copy route is bounded by rows and by watermark.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.2.3:** `crates/medulla-federation/src/federation_opt/tests.rs::trace_names_route_per_fragment` (planned). Proves the row: EXPLAIN states whether each fragment ran live or from a copy.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.2.3:** `tests/federation/test_acceleration_served.py::test_explain_names_live_and_copy` (planned). Proves the row: EXPLAIN states whether each fragment ran live or from a copy.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1:** `crates/medulla-lake/tests/metastore.rs::partition_pruning_reads_only_matching_partitions` (planned). Proves the row: Listing pages at the knob size and partitions prune by pushed predicates.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.2:** `crates/medulla-lake/tests/metastore.rs::orc_stripe_pruning_skips_non_matching_stripes` (planned). Proves the row: ORC scans skip stripes by stripe statistics.
- **MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.3:** `crates/medulla-lake/tests/metastore.rs::avro_reader_schema_projection` (planned). Proves the row: Avro scans project the reader schema.
- **MEDULLA-UNIFIED-DATA-PLANE-R750.754:** `tests/data_plane/test_history_search_served.py::test_commit_writes_audit_and_search_finds_step` (planned). Proves the row: Wiring: terminal commit writes audit rows and steps; `HistorySearch` reads them.
- **MEDULLA-UNIFIED-DATA-PLANE-R761:** `tests/data_plane/test_profile_served.py::test_profile_job_attaches_to_catalog` (planned). Proves the row: Dataset and column profiling (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R761.2:** `crates/medulla-compute/src/sketch.rs::tests::hll_precision_14_within_error_bound` (planned). Proves the row: Behavior: pushed aggregates where the dialect allows, sampling otherwise, sketch for distinct counts.
- **MEDULLA-UNIFIED-DATA-PLANE-R761.2:** `crates/medulla-h-jobs/src/jobs/executor_profile.rs::tests::sample_profile_counts_are_consistent` (planned). Proves the row: Behavior: pushed aggregates where the dialect allows, sampling otherwise, sketch for distinct counts.
- **MEDULLA-UNIFIED-DATA-PLANE-R761.3:** `tests/data_plane/test_profile_served.py::test_profile_job_attaches_to_catalog` (planned). Proves the row: Wiring: the `DatasetProfile` job kind attaches profiles to the catalog.
- **MEDULLA-UNIFIED-DATA-PLANE-R761.4:** `tests/data_plane/test_profile_served.py::test_profile_refusals` (planned). Proves the row: Refusal: out-of-scope dataset, unsupported source.
- **MEDULLA-UNIFIED-DATA-PLANE-R761.5:** `tests/data_plane/test_profile_served.py::test_profile_method_recorded` (planned). Proves the row: Evidence: each profile records its method and sample size.
- **MEDULLA-UNIFIED-DATA-PLANE-R761.6:** `tests/data_plane/test_profile_served.py::test_profile_job_attaches_to_catalog` (planned). Proves the row: Hot-path proof: profile ten columns of a native dataset.
- **MEDULLA-UNIFIED-DATA-PLANE-R762:** `tests/data_plane/test_quality_served.py::test_rule_scores_and_incident` (planned). Proves the row: Data quality rules, scores and quality incidents (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R762.2:** `crates/medulla-core/src/quality.rs::tests::three_failures_open_one_incident` (planned). Proves the row: Behavior: compile each rule kind, score it, count consecutive failures.
- **MEDULLA-UNIFIED-DATA-PLANE-R762.2:** `crates/medulla-core/src/quality.rs::tests::each_rule_kind_scores` (planned). Proves the row: Behavior: compile each rule kind, score it, count consecutive failures.
- **MEDULLA-UNIFIED-DATA-PLANE-R762.3:** `tests/data_plane/test_quality_served.py::test_rule_scores_and_incident` (planned). Proves the row: Wiring: `QualityRulePut` and the `QualityEvaluate` job kind.
- **MEDULLA-UNIFIED-DATA-PLANE-R762.4:** `tests/data_plane/test_quality_served.py::test_quality_refusals` (planned). Proves the row: Refusal: invalid rule, invalid threshold, unapproved rule.
- **MEDULLA-UNIFIED-DATA-PLANE-R762.5:** `tests/data_plane/test_quality_served.py::test_score_evidence_reads_back` (planned). Proves the row: Evidence: each score records the compiled query digest and the row counts.
- **MEDULLA-UNIFIED-DATA-PLANE-R762.6:** `tests/data_plane/test_quality_served.py::test_rule_scores_and_incident` (planned). Proves the row: Hot-path proof: evaluate 100 rules over a native dataset.
- **MEDULLA-UNIFIED-DATA-PLANE-R762.8:** `tests/data_plane/test_quality_served.py::test_outside_scores_open_incident` (planned). Proves the row: Quality results from outside checkers arrive through `SourceIngest`.
- **MEDULLA-UNIFIED-DATA-PLANE-R763:** `tests/data_plane/test_column_lineage_served.py::test_view_lineage_and_impact` (planned). Proves the row: Column-level lineage from SQL text (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R763.2:** `crates/medulla-federation/src/column_lineage.rs::tests::view_columns_resolve` (planned). Proves the row: Derive column edges from SQL with the shared parser.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.2:** `tests/data_plane/test_column_lineage_served.py::test_view_lineage_and_impact` (planned). Proves the row: Derive column edges from SQL with the shared parser.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.3:** `tests/data_plane/test_column_lineage_served.py::test_view_lineage_and_impact` (planned). Proves the row: Wire column lineage into the catalog reader, the lineage drain and impact queries.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.3:** `crates/medulla-h-query/src/query/column_lineage.rs::handle` (planned). Proves the row: Wire column lineage into the catalog reader, the lineage drain and impact queries.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.4:** `tests/data_plane/test_column_lineage_served.py::test_unparseable_sql_quarantines` (planned). Proves the row: Quarantine SQL that does not parse.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.5:** `tests/data_plane/test_column_lineage_served.py::test_edge_evidence_reads_back` (planned). Proves the row: Record query digest and parser version on each column edge.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.6:** `tests/data_plane/test_column_lineage_served.py::test_thousand_views_derive` (planned). Proves the row: Hot-path proof for column lineage.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.7:** `crates/medulla-catalog-types/src/attached_catalog.rs::tests::view_order_does_not_change_hash` (planned). Proves the row: Catalog carries view definitions.
- **MEDULLA-UNIFIED-DATA-PLANE-R763.7:** `tests/data_plane/test_column_lineage_served.py::test_view_lineage_and_impact` (planned). Proves the row: Catalog carries view definitions.
- **MEDULLA-UNIFIED-DATA-PLANE-R764:** `tests/data_plane/test_catalog_search_served.py::test_search_ranks_used_asset_higher` (planned). Proves the row: Asset usage statistics and catalog search (rollup).
- **MEDULLA-UNIFIED-DATA-PLANE-R764.3.1:** `tests/data_plane/test_catalog_search_served.py::test_usage_counts_come_from_served_queries` (planned). Proves the row: Record asset usage from the server's own query dispatch.
- **MEDULLA-UNIFIED-DATA-PLANE-R764.4:** `tests/data_plane/test_catalog_search_served.py::test_search_refusals` (planned). Proves the row: Refuse unauthorized hits and oversized catalog queries.
- **MEDULLA-UNIFIED-DATA-PLANE-R764.5:** `tests/data_plane/test_catalog_search_served.py::test_hit_components_listed` (planned). Proves the row: List score components on each catalog hit.
- **MEDULLA-UNIFIED-DATA-PLANE-R764.6:** `tests/data_plane/test_catalog_search_served.py::test_search_over_ten_thousand_assets` (planned). Proves the row: Hot-path proof for catalog search.
- **MEDULLA-UNIFIED-DATA-PLANE-R764.7:** `tests/data_plane/test_catalog_search_served.py::test_source_query_log_feeds_usage` (planned). Proves the row: Usage from attached-source query logs.
- **MEDULLA-UNIFIED-DATA-PLANE-R764.8:** `tests/data_plane/test_catalog_search_served.py::test_frequent_users_and_join_suggestions` (planned). Proves the row: Frequent users and usage suggestions.
- **MEDULLA-UNIFIED-DATA-PLANE-R765:** `tests/federation/test_foreign_source_catalog_served.py::test_visibility_probe_and_revocation` (planned). Proves the row: Serve the foreign source catalog: list, get, probe, share and unshare.
- **MEDULLA-UNIFIED-DATA-PLANE-R766:** `crates/medulla-work-store/src/redb_store/work_item/submit.rs::tests::research_web_kind_submits_and_unknown_kind_refuses` (planned). Proves the row: Engine work item kind for web research.
- **MEDULLA-UNIFIED-DATA-PLANE-R767:** `tests/served_query_completeness.rs::tenant_context_view_scopes_two_tenants_without_copying` (planned). Proves the row: Tenant context view over approved sources.

- `MEDULLA-UNIFIED-DATA-PLANE-R002.1`: `crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs::an_inline_secret_dsn_is_refused_and_never_persisted` (planned). A `Sql` spec whose DSN carries `user:password@` returns `CONNECTION_SECRET_INLINE` and leaves the catalog empty; a secret-reference DSN registers.
- `MEDULLA-UNIFIED-DATA-PLANE-R003.1`: `crates/medulla-catalog-types/src/attached_catalog.rs::content_hash_matches_golden_value` (planned). A fixed three-table catalog hashes to a literal pinned value, so a hasher or field-order change is caught.
- `MEDULLA-UNIFIED-DATA-PLANE-R013`: `tests/attached_postgres_served.rs::postgres_types_and_capture_match_native` (planned). Runs against the `postgres:16` container of the nightly lane and asserts equal rows and one change envelope per committed change for each covered type.
- `MEDULLA-UNIFIED-DATA-PLANE-R014.2.2`: `tests/attached_mysql_served.rs::catalog_and_types_match_native` (planned). L3 test in the nightly lane against `mysql:8.4` and `mariadb:11.4`: catalog hash and each mapped type equal the native result.
- `MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1`: `crates/medulla-sqlite-format/tests/wal_snapshot.rs::snapshot_sees_last_commit_and_survives_checkpoint_race` (planned). L2: builds a WAL with a torn tail frame and a racing checkpoint; asserts the last committed rows and the `SQLITE_SNAPSHOT_UNSTABLE` refusal after the timeout.
- `MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.2`: `tests/attached_sqlite_served.rs::watermark_poll_emits_each_change_once` (planned). Inserts and updates rows, asserts one envelope per change and no duplicate after a restart.
- `MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.3`: `tests/attached_sqlite_served.rs::live_file_reads_and_capture_under_concurrent_writer` (planned). L3: four writer threads and 2,000 writes while the engine reads and polls; asserts zero busy errors and rows equal to the `sqlite3` result.
- `MEDULLA-UNIFIED-DATA-PLANE-R017.2.2`: `tests/attached_clickhouse_served.rs::federated_reads_match_native_and_capture_is_refused` (planned). L3 against the `clickhouse/clickhouse-server` container of the nightly lane; compares rows with the native engine and asserts `CAPABILITY_UNSUPPORTED` on a capture request.
- `MEDULLA-UNIFIED-DATA-PLANE-R020.2.2`: `tests/attached_warehouse_served.rs::iceberg_and_duckdb_reads_match_native` (planned). L3: compares Iceberg and DuckDB rows with the native engines and asserts `CAPABILITY_UNSUPPORTED` for kind `snowflake`.
- `MEDULLA-UNIFIED-DATA-PLANE-R021.2`: `tests/e2e_debezium_bridge.rs::consumer_feeds_drafts_in_offset_order` (planned). Feature `cdc-kafka`, ignored without a broker: publishes a fixture topic and asserts one applied draft per event in offset order and no duplicate after a consumer restart.
- `MEDULLA-UNIFIED-DATA-PLANE-R022.2`: `tests/attached_conformance.rs::manifest_pins_every_image_digest` (planned). L1: every `[[engine]]` carries a `sha256:` digest; an entry without one fails. Also `tests/attached_conformance.rs::every_matrix_entry_has_a_green_entry_or_a_reviewed_deviation` (planned, L3).
- `MEDULLA-UNIFIED-DATA-PLANE-R022.3.2`: `tests/attached_conformance.rs::replay_equals_snapshot` (planned). L3: digest equality for the fixture and a failing entry for a dropped event.
- `MEDULLA-UNIFIED-DATA-PLANE-R026.2.2`: `tests/native_hosting_gramps.rs::p0_p1_p2_exit_artifacts` (planned). L3: a valid exit for P0, P1 and P2 and a stored difference list for P2.
- `MEDULLA-UNIFIED-DATA-PLANE-R026.3.2`: `tests/native_hosting_gramps.rs::p4_restore_and_p5_schema` (planned). L3: three equal restore digests and the schema answer.
- `MEDULLA-UNIFIED-DATA-PLANE-R029.2`: `tests/data_plane/test_schema_context_served.py::test_schema_context_answers_each_intent_and_refuses_unknown_table` (planned). L4: joins, class, application and impact for a known table and the typed refusal for an unknown one.
- `MEDULLA-UNIFIED-DATA-PLANE-R030.3`: `tests/data_plane/test_entity_link_served.py::test_no_sameas_before_approval` (planned). L4: true, false and ambiguous pairs; no `owl:sameAs` edge before approval and one after.
- `MEDULLA-UNIFIED-DATA-PLANE-R031.2`: `tests/attached_isolation_served.rs::quota_breach_and_failure_do_not_degrade_other_source` (planned). L3: hangs source A and breaches its quota; source B answers within its quota and A returns the codes.
- `MEDULLA-UNIFIED-DATA-PLANE-R035`: `crates/medulla-server-api/src/icv_guard.rs::attached_schema_write_reuses_the_compiled_policy` (planned). Counts `compose_validated_sources` calls over 1,000 writes to a graph with a dynamic schema source and asserts one compile and an `IcvPolicy` built once.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.1`: `crates/medulla-stream/tests/incident_authority.rs::model_round_trips_and_status_order_is_closed` (planned). L2: `AnomalyReading` and `IncidentRecord` serde round trip; the status enum orders open, acknowledged, resolved.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.2`: `crates/medulla-stream/tests/incident_authority.rs::anomalies_inside_window_join_one_incident` (planned). L2: anomalies inside the window join one reused incident; one after the window opens a second.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.3`: `crates/medulla-capabilities/tests/contract_registry.rs::incident_methods_listed` (planned). L2: the five variants appear in the registry with scopes `incident:write` and `incident:read`.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.3`: `tests/data_plane/test_incident_authority_served.py::test_five_methods_served_under_caller_tenant` (planned). L4: all five methods answer under the caller's tenant carrier and a body tenant is ignored.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.4`: `crates/medulla-stream/tests/incident_authority.rs::refusals_leave_state_unchanged` (planned). L2: each code and an unchanged incident set after every refusal.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.5`: `crates/medulla-stream/tests/incident_authority.rs::one_incident_per_signature_with_its_evidence_set` (planned). L2: signature digest uniqueness per open incident, evidence ids per incident, and the open, acknowledged, resolved order.
- `MEDULLA-UNIFIED-DATA-PLANE-R760.6`: `tests/data_plane/test_incident_authority_served.py::test_anomalies_correlate_into_one_incident` (planned). L4: 10,000 anomalies over 100 entities give one incident per signature and the call returns under 25 ms p99.
- `MEDULLA-UNIFIED-DATA-PLANE-R014`: `tests/attached_mysql_served.rs::mysql_and_mariadb_reads_match_native` (planned). Root L3 test `tests/attached_mysql_served.rs::mysql_and_mariadb_reads_match_native` runs against the `mysql:8.4` and `mariadb:11.4` containers of the nightly lane (R022.2).
- `MEDULLA-UNIFIED-DATA-PLANE-R016`: `tests/attached_virtual_dialect_served.rs::mssql_virtual_graph_reads_through_loopback_connector` (planned). Root L3 test `tests/attached_virtual_dialect_served.rs::mssql_virtual_graph_reads_through_loopback_connector` registers a loopback connector that declares `mssql`, then answers a query through `UnifiedQuery`.
- `MEDULLA-UNIFIED-DATA-PLANE-R016.2`: `crates/medulla-federation/tests/mssql_render.rs::top_and_offset_fetch_render` (planned). Test: `crates/medulla-federation/tests/mssql_render.rs::top_and_offset_fetch_render` (L2) asserts the rendered text.
- `MEDULLA-UNIFIED-DATA-PLANE-R018`: `tests/attached_virtual_dialect_served.rs::oracle_and_db2_virtual_graph_reads_through_loopback_connector` (planned). Root L3 test `tests/attached_virtual_dialect_served.rs::oracle_and_db2_virtual_graph_reads_through_loopback_connector` (L3).
- `MEDULLA-UNIFIED-DATA-PLANE-R019`: `tests/attached_virtual_dialect_served.rs::document_virtual_graph_infers_shape_and_reads` (planned). Root L3 test `tests/attached_virtual_dialect_served.rs::document_virtual_graph_infers_shape_and_reads` registers a loopback document connector and asserts the inferred shape and the read.
- `MEDULLA-UNIFIED-DATA-PLANE-R026`: `tests/native_hosting_gramps.rs::phases_run_in_order_and_each_exit_is_valid` (planned). Root L3 test `tests/native_hosting_gramps.rs::phases_run_in_order_and_each_exit_is_valid` asserts the order and each exit artifact.
- `MEDULLA-UNIFIED-DATA-PLANE-R028`: `tests/pg_differential.rs::every_difference_is_a_named_deviation_and_none_is_suppressed` (planned). Test: `tests/pg_differential.rs::every_difference_is_a_named_deviation_and_none_is_suppressed` (L3) fails on an unlisted difference and on a deviation without a fixture.
- `MEDULLA-UNIFIED-DATA-PLANE-R038`: `tests/federation/test_acceleration_served.py::test_cold_live_hot_copy_stale_live` (planned). Root L4 test: `tests/federation/test_acceleration_served.py::test_cold_live_hot_copy_stale_live` runs three mappings: cold live, hot from the copy, stale live again.
