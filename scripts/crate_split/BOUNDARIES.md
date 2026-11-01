# Medulla crate boundaries after the split

This is the target crate map that comes after the facade split (MEDULLA-CONTRACT-R056.11).
The machine-readable map is `boundaries.toml`. `route_rows.py` reads it and writes
`row_routing.jsonl`. It also measures current lines and projects the lines each crate will
have once its open rows land. Run `--update --check` after any spec edit or module move. It
exits 1 when a crate goes over budget or a row has no target.

## Rules

- Each crate has one responsibility.
- Layering is acyclic and points downward. A crate depends only on crates at a strictly
  lower level. Crates at the same level are siblings and have no edges between them.
- A crate holds at most 25,000 lines of Rust. Vendored patches are excluded.
- Old paths are not preserved by re-exports. The codemod rewrites every `use` in place.
- A PR compiles only the crates it touches and their dependents.

Projection: 100 lines per open row (implementation plus bound test). Competency and
vocabulary rows count 60 lines, because they are mostly served queries and ontology
terms. The estimates live in `[estimates]`. Open means not VERIFIED and not RETIRED,
computed at HEAD with the pipelines spec generator. Today that is 2,395 leaf rows.

## Crate count

- Before: 66 crates under `crates/` plus the root `medulla-engine`, which makes 67. Twelve
  of the 66 are facade scaffolds from manifest.toml whose code still lives in `src/`.
- After: 132 crates. That is 42 unchanged small leaf crates plus the 90 crates in
  boundaries.toml. `medulla-query` is removed: its modules go to `medulla-sql`,
  `medulla-cypher`, `medulla-tables` and `medulla-table-store`. 66 crates are new.
- Every projected crate fits the budget. The largest are
  `medulla-agent-types` (23.1k), `medulla-catalog-types` (23.0k), `medulla-graph-core` (22.9k) and
  `medulla-decision-types` (22.9k).

## Layered graph

```
F12 medulla-engine (executable)
F11 medulla-server
F10 server-graph | server-consensus | server-obs | server-lake | server-identity | server-router   (siblings)
F9  h-query | h-sparql | h-retrieve | h-mining | h-admin | h-packs | h-decide | h-compute
    | h-ingest | h-graph | h-jobs | h-txn | wire | wire-msg                                     (siblings, no edges)
F8  raft-xshard | wire-core
F7  raft
F6  persistence-records | persistence-ops
F5  persistence
F4  server-auth | server-mutation | server-blob
F3  server-api
F2  work-store | resource-store | lane-store
F1  store
F0  kernel
--- every facade crate may depend on any leaf crate below ---
L6  decision-records | decision-eval | decision-memory | competency | competency-xsrc
    | repo-index | sdlc | parity | placement
L5  uql | nlq | federation | exec | retrieval | plan | decision-plan | packs | catalog | vocab | ingest
L4  sql | cypher | tables | table-store | virtual-graph | graph-algos | mining | compute
    | finance | decision-policy | decision-rank
L3  graph-core | isolation | core | semantic-index | ann-codes | reasoner | decision-stats | finance-market
L2  storage | storage-owner | numeric | transaction | rdf | the 42 unchanged leaves (geo, tsdb, shacl, ...)
    | protocol-types (the type hub above every domain type crate)
L1  identity-types < mutation-types < index-types < result-types < agent-types < catalog-types
    < decision-types   (one-way edges, left to right only)
L0  medulla-types (thin)
```

The levels are the target. Existing edges that run against them, such as a module in L3
that calls one in L4 today, are back-edges. The codemod lane must cut them with
`edges.py` before it moves the module. The level assignment is the contract the cut must
meet.

## Families with explicit boundaries

- **Decision engine** (750 open rows). The ladder stages map to crates:
  - `decision-types` (L1) holds the vocabulary: ResolutionKind, evidence classes,
    requests and answers.
  - `decision-stats` (L3) holds the fitted heads, calibration, conformal bounds and
    off-policy evaluation.
  - `decision-policy` (L4) holds stages 3-5: policy, hard constraints, knobs, admission,
    refusals, budgets and the bounded exact solve.
  - `decision-rank` (L4) holds stage 6: ranking the legal options that remain.
  - `decision-plan` (L5) holds stage 7 and AgentAssemble: task graphs, delegation,
    harness and skills.
  - `decision-memory` (L6) holds decision context and recall.
  - `decision-eval` (L6) holds served proofs, quality, drift and nightly evaluation.
  - `decision-records` (L6) holds receipts, the ledger, the R805 revision lifecycle,
    provenance and replay.
  - `h-decide` (F9) only serves Decide, AgentAssemble, DecisionCommit and DecisionLog.
    It holds no ladder logic.
- **Federated query**:
  - `rdf` holds SPARQL.
  - `reasoner` holds OWL, rules and closure.
  - `uql` and `nlq` hold the languages.
  - `federation` handles cross-source planning.
  - `exec` holds the physical executor.
  - `retrieval` holds retrieval, expansion and verdicts, and is the spec default.
  - `h-query`, `h-sparql` and `h-retrieve` serve them.
- **Durable graph kernel**:
  - `kernel` holds the mutation gateway and is the spec default.
  - `store` holds redb, shards and group commit.
  - `resource-store` holds leases and the scheduler.
  - `transaction` holds the outbox and the change feed.
  - `storage` and `storage-owner` handle recovery.
  - `persistence-ops` holds backup, reshard and upgrade.
  - `raft` and `raft-xshard` handle consensus.
  - `wire-core` holds the listeners.
  - `reasoner` takes temporal forward chaining.
- **Typed packs**:
  - `packs` handles import, validation and roll-up.
  - `catalog` holds lookup and lineage.
  - `shacl` holds shapes.
  - `catalog-types` holds the record types.
  - `h-packs` serves them.
  - `vocab` holds the four vocabulary specs.
- **Connector competency**:
  - `competency` holds the connector questions.
  - `competency-xsrc` holds the cross-source and orchestration questions, plus the
    connector skill/tool/prompt/resource questions.
- **Unified data plane**:
  - `virtual-graph` (L4) holds OBDA, mappings, source selection and source authority.
  - `catalog` takes the source-authority catalog rows.

## The medulla-types split

`medulla-types` has the highest fan-in in the workspace (148k lines), so any edit to it
rebuilds everything. It splits into nine crates. Measured lines (2026-10-31):

| Crate | Level | Lines | Holds |
|---|---|---|---|
| `medulla-types` | L0 | 16.6k | contract ids and bounded collections, msgpack, wire scalars and query plans, `solve_scalar` (exact scalars, `Sha256Digest`, `digest_text`), `lease_vocab`, test-support fixtures and the small unprefixed modules |
| `medulla-identity-types` | L1 | 17.3k | identity, RBAC, tenancy, ACL and authority |
| `medulla-mutation-types` | L1 | 9.2k | `mutation_scope` (scope identity, durability domain, committed version, outbox rows), commit descriptor, governed change, capacity, consensus |
| `medulla-index-types` | L1 | 10.6k | semantic index, embedding, managed index, entity resolution |
| `medulla-result-types` | L1 | 11.6k | answers, verdicts, epistemic nodes and predicates |
| `medulla-agent-types` | L1 | 22.3k | agents (incl. `agent_guardrail`, `agent_topology`), work items, jobs, runs, `native_control`, `control_lease`, `host_start` |
| `medulla-catalog-types` | L1 | 16.5k | packs, schema, catalog, ingest, lineage, release records, attached-source readers |
| `medulla-decision-types` | L1 | 17.0k | `decision/`, `policy_evolution`, `solve`, `current_plan` |
| `medulla-protocol-types` | L2 | 22.2k | `Method`, its envelopes (`mutation_batch`, `change_envelope`, `debezium_bridge`, `result_contract`, `storage_wire`), wire-query types, `messaging_wire`, `error_registry` |

Why the hub sits on top: `Method` embeds every domain's request types and
`ChangeEnvelope` (which embeds `MutationBatch`, which embeds `Method`), so `Method`, the
batch and the envelope are one recursive type family and live in one crate above the
domains. Domain type crates never name `Method`: the two readers that matched on it
(`market_write_scope`, `run_completion_payload`) are functions in `protocol`.

Cycle cuts made at item level (MEDULLA-CONTRACT-R056.11):

- `mutation_batch` → `mutation_scope`: the Method-free scope identity, durability
  domain, committed version and outbox rows (index and agent types name them).
- `decision::topology` → `agent_topology`, `decision::guardrail` → `agent_guardrail`
  (agent graphs and components validate against them); their decision-record tests
  stay in `decision`.
- `solve::scalar` and `decision::digest::{digest_text, DIGEST_TEXT_PREFIX}` →
  `solve_scalar` (L0).
- `contract::{declared_error_code, classify_refusal}` → `error_registry` (hub), since the
  registry names every domain's closed codes.
- `control_lease::{MAX_CONTROL_LEASE_SPAN_MS, RBAC_ELEVATION_KIND}` → `lease_vocab` (L0).
- `result_contract::cluster::{is_valid_server_name, MAX_REGISTERED_SERVER_NAME_BYTES}`
  → `fleet_catalog`.
- The governed-change wire-code test moved to `protocol`.

Still open for the crate moves: `contract::closed_error_codes` is a `pub(crate)` macro
used by domain crates and must become exported; `test_support` (L0) names domain
modules and moves to a dev-only fixtures crate or splits per domain.

The effect: a decision-type edit rebuilds only the decision family and h-decide, and no
longer rebuilds SQL, raft or wire. The small residual modules should move to a domain
crate by name prefix the first time a lane touches one. Adding a glob to the crate's
`moves` is enough.

## Row-routing rule (no judgment needed)

For a row of spec S, apply the steps below in order. The first step that matches wins.

1. **Title keyword.** If S has `[[route]]` rules in boundaries.toml, match the row's bold
   title, lowercased, against each `match` regex in file order. The first hit sends the
   row to `crates/<crate>/<module>`. The decision engine always has a hit or falls
   through to step 3, because its rules come before paths.
2. **Named path.** Take the first source path the row names, `crates/medulla-*/src/...`
   or `src/...`, and skip any path under the skip list (method registry, error codes).
   Resolve it through the `moves` prefixes, longest prefix first. If no prefix matches,
   use the crate it is in today; for `src/`, that is placement.toml.
   - If the result is the thin `medulla-types`, use `types_home[S]` instead.
   - The target module is the same module path inside the new crate.
3. **Spec default.** Use `[defaults][S]`.

So a row for spec S or subsystem X that adds type T goes to `types_home[S]`, at
`src/<domain module of T>`. A row that adds behaviour goes to the crate that rule 1, 2 or
3 gives. A new spec adds a line to `[defaults]` and to `types_home`. A crate that goes over
budget is split by adding a crate with `moves` and, if needed, a `[[route]]`. Then re-run
the script.

## Per-crate table (generated numbers; re-run the script to refresh)

| Crate | Level | Responsibility | Current | Projected | Open rows by spec |
|---|---|---|---:|---:|---|
| `medulla-types` | L0 | Thin shared vocabulary: ids, errors, scalar values and the test-support fixtures; no domain types. | 18,355 | 18,355 |  |
| `medulla-protocol-types` | L1 | Method registry and wire-query envelope types shared by every listener and client. | 16,899 | 17,299 | federated-query-and-reasoning 2, public-engine-contract-and-release 1, unified-data-plane 1 |
| `medulla-identity-types` | L1 | Issuer, principal, scope, tenancy, ACL and authority types. | 17,243 | 18,143 | engine-identity-and-scope 9 |
| `medulla-agent-types` | L1 | Agent components, graphs, libraries, templates, work items, jobs and run history types. | 19,552 | 23,072 | orchestration-vocabulary 23, typed-packs-and-catalog 8, decision-engine 7, orchestration-competency 3 |
| `medulla-mutation-types` | L1 | Mutation, batch, change envelope, commit descriptor, outbox, lease and consensus record types. | 14,802 | 16,762 | durable-graph-kernel 10, source-changesets 4, branch-merge 3, repository-index-and-ingestion 2 |
| `medulla-index-types` | L1 | Semantic index, embedding, managed index and entity resolution types. | 10,596 | 10,596 |  |
| `medulla-result-types` | L1 | Answer, result contract, verdict, epistemic node, argument and query predicate types. | 14,725 | 18,425 | federated-query-and-reasoning 19, finance-primitives 11, orchestration-vocabulary 4, decision-engine 2 |
| `medulla-catalog-types` | L1 | Connector pack, schema, catalog, source ingestion, lineage and release record types. | 15,612 | 22,952 | typed-packs-and-catalog 32, ingest-worker 13, unified-data-plane 13, repository-index-and-ingestion 5 |
| `medulla-decision-types` | L1 | Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records. | 12,362 | 22,882 | decision-engine 101, orchestration-vocabulary 2, development-lifecycle 2, placement-and-worker-policy 1 |
| `medulla-storage` | L2 | Physical storage owner, direct state, recovery and scoped tables. | 17,295 | 18,095 | durable-graph-kernel 5, object-storage-backend 2, engine-parity 1 |
| `medulla-storage-owner` | L2 | Storage owner process: owner state machine and ownership handoff. | 9,521 | 9,521 |  |
| `medulla-numeric` | L2 | Deterministic numeric kernels: series, random, stats, clustering and the Python bridge. | 7,437 | 9,037 | finance-primitives 14, federated-query-and-reasoning 2 |
| `medulla-transaction` | L2 | Transaction graft, outbox and change capture. | 20,979 | 21,079 | finance-primitives 1 |
| `medulla-rdf` | L2 | RDF model, SPARQL algebra and update, and the serialisations (JSON-LD, RDF/XML, GeoSPARQL). | 10,782 | 11,562 | federated-query-and-reasoning 5, cross-source-competency 3, source-authority 1 |
| `medulla-graph-core` | L3 | In-memory property graph, its indexes, paths, projections and JSONPath. | 17,382 | 22,942 | connector-competency 74, durable-graph-kernel 3, typed-packs-and-catalog 3, cross-source-competency 2 |
| `medulla-isolation` | L3 | Tenant isolation, RBAC persistence and dependency scopes. | 9,292 | 11,992 | engine-identity-and-scope 25, decision-engine 2 |
| `medulla-core` | L3 | Engine core services that remain: broker, registry, lifecycle, durable apply and result cache. | 9,428 | 11,328 | typed-packs-and-catalog 6, orchestration-vocabulary 5, orchestration-competency 4, durable-graph-kernel 2 |
| `medulla-semantic-index` | L3 | Semantic ANN codes and the semantic index service. | 9,169 | 9,369 | federated-query-and-reasoning 1, repository-index-and-ingestion 1 |
| `medulla-reasoner` | L3 | OWL, tableau and rule reasoning plus reasoning closure. | 13,045 | 13,645 | federated-query-and-reasoning 3, durable-graph-kernel 2, typed-packs-and-catalog 1 |
| `medulla-decision-stats` | L3 | Statistical ranking for the decision ladder: fitted heads, calibration, conformal bounds, off-policy evaluation and scores. | 15,409 | 22,629 | decision-engine 60, finance-primitives 8, orchestration-vocabulary 2, placement-and-worker-policy 2 |
| `medulla-finance-market` | L3 | Market data primitives: quotes, bars, books and their kernels. | 5,458 | 5,558 | finance-primitives 1 |
| `medulla-ann-codes` | L3 | Semantic ANN code books: quantisation, encode and decode. | 15,927 | 15,927 |  |
| `medulla-sql` | L4 | SQL dialect: parse, classify, plan and execute over tables. | 21,850 | 22,850 | engine-parity 6, unified-data-plane 3, durable-graph-kernel 1 |
| `medulla-cypher` | L4 | Cypher language, its executor, graph data science calls and the edge index. | 15,469 | 15,569 | unified-data-plane 1 |
| `medulla-tables` | L4 | Table model: schema, property graph tables and ANN authority (everything but the table store). | 12,694 | 12,994 | engine-parity 2, federated-query-and-reasoning 1 |
| `medulla-table-store` | L4 | Physical table store under the table model. | 12,073 | 12,473 | engine-parity 4 |
| `medulla-query` | L4 | Removed after the split: every module moves out (tables become medulla-tables). | 0 | 0 |  |
| `medulla-virtual-graph` | L4 | Attached sources read as virtual graphs: OBDA mappings, source selection and source authority checks. | 7,822 | 11,322 | unified-data-plane 31, source-authority 4 |
| `medulla-graph-algos` | L4 | Graph algorithms and graph learning. | 15,839 | 16,339 | federated-query-and-reasoning 5 |
| `medulla-mining` | L4 | Pattern and process mining over graph records. | 9,503 | 9,603 | repository-index-and-ingestion 1 |
| `medulla-compute` | L4 | Compute expression language: parser, AST, data science operators and sketches. | 13,559 | 14,459 | source-changesets 4, decision-engine 2, ingest-worker 2, branch-merge 1 |
| `medulla-finance` | L4 | Finance kernels, backtests, risk, attribution, banking and execution signals. | 10,985 | 14,385 | finance-primitives 34 |
| `medulla-decision-policy` | L4 | Ladder stages 3-5: policy, hard constraints, knobs, admission, refusals, budgets and the bounded exact solve. | 2,413 | 16,713 | decision-engine 142, placement-and-worker-policy 1 |
| `medulla-decision-rank` | L4 | Ladder stage 6: ranking the remaining legal options with fitted statistics (scores, champions, bandits, credit). | 0 | 6,400 | decision-engine 64 |
| `medulla-uql` | L5 | UQL language: parser, binder and lowering to the executor. | 8,134 | 8,834 | federated-query-and-reasoning 7 |
| `medulla-nlq` | L5 | Natural-language questions to UQL, with regressions. | 4,515 | 5,315 | federated-query-and-reasoning 8 |
| `medulla-federation` | L5 | Federated planning across sources: federation optimizer, SSRF guard and remote engines. | 11,023 | 15,643 | connector-competency 19, federated-query-and-reasoning 15, unified-data-plane 14, cross-source-competency 7 |
| `medulla-exec` | L5 | Physical plan execution: optimizer, cost models, runtime, DAG, incremental and streamed results. | 12,687 | 14,287 | finance-primitives 10, federated-query-and-reasoning 4, unified-data-plane 2 |
| `medulla-retrieval` | L5 | Retrieval and expansion: knowledge assembly, LeanRAG, verdicts and stop rules. | 4,706 | 16,906 | federated-query-and-reasoning 122 |
| `medulla-plan` | L5 | Cross-modal plan tests and the planner entry that composes UQL, federation, exec and retrieval. | 9,849 | 13,549 | federated-query-and-reasoning 34, unified-data-plane 3 |
| `medulla-decision-plan` | L5 | Ladder stages 6-7 and task planning: AgentAssemble, bounded task graphs, delegation, harness and skill selection. | 5,032 | 10,332 | decision-engine 51, typed-packs-and-catalog 2 |
| `medulla-packs` | L5 | Typed pack import, validation and roll-up to core classes. | 715 | 10,315 | typed-packs-and-catalog 95, federated-query-and-reasoning 1 |
| `medulla-catalog` | L5 | The catalog over packs, sources and schemas: lookup, lineage and discovery. | 0 | 500 | typed-packs-and-catalog 4, unified-data-plane 1 |
| `medulla-vocab` | L5 | Ontology modules and shapes for the data-platform, media, supply-chain and orchestration vocabularies. | 0 | 3,600 | orchestration-vocabulary 35, media-and-personal-vocabulary 24, supply-chain-vocabulary 1 |
| `medulla-ingest` | L5 | Ingest worker loop, lanes and queues. | 0 | 5,000 | ingest-worker 50 |
| `medulla-decision-records` | L6 | Decision records: receipts, ledger, the revision lifecycle (R805), provenance, replay and feedback. | 0 | 6,900 | decision-engine 69 |
| `medulla-decision-eval` | L6 | Decision evaluation: served proof suites, quality and relevance scoring, drift and nightly evaluation. | 0 | 10,200 | decision-engine 102 |
| `medulla-decision-memory` | L6 | Decision context and memory: session memory, retrieval for decisions, temporal and indexless recall. | 0 | 5,200 | decision-engine 52 |
| `medulla-competency` | L6 | Connector competency questions answered through the served query path. | 0 | 1,860 | connector-competency 31 |
| `medulla-competency-xsrc` | L6 | Cross-source and orchestration competency questions. | 0 | 2,940 | orchestration-competency 28, cross-source-competency 16, connector-competency 5 |
| `medulla-repo-index` | L6 | Repository indexing, source changesets and branch merge as graph records. | 0 | 4,500 | repository-index-and-ingestion 32, source-changesets 9, branch-merge 4 |
| `medulla-sdlc` | L6 | Graph-native development lifecycle: spec rows, gates, verdicts and releases as records. | 0 | 5,500 | development-lifecycle 55 |
| `medulla-parity` | L6 | Engine compatibility features and their parity proofs against other engines' behaviour. | 0 | 2,500 | engine-parity 25 |
| `medulla-placement` | L6 | Worker admission and placement policy. | 0 | 1,400 | placement-and-worker-policy 14 |
| `medulla-kernel` | F0 | Persist, store authority, mutation apply and channels. | 8,766 | 14,466 | durable-graph-kernel 56, engine-identity-and-scope 1 |
| `medulla-store` | F1 | redb store core, layout, sidecar and shards. | 22,318 | 22,718 | durable-graph-kernel 2, engine-identity-and-scope 1, ingest-worker 1 |
| `medulla-work-store` | F2 | Durable work items and their capability records. | 13,853 | 14,653 | durable-graph-kernel 7, unified-data-plane 1 |
| `medulla-resource-store` | F2 | Resource reservations, capacity leases and enrichment budgets. | 11,078 | 11,778 | durable-graph-kernel 7 |
| `medulla-lane-store` | F2 | Development lane records in the store. | 10,319 | 10,319 |  |
| `medulla-server-api` | F3 | Server API surface shared by handlers: api types, blob, kv, CDC and CEP. | 22,290 | 22,590 | federated-query-and-reasoning 1, engine-identity-and-scope 1, typed-packs-and-catalog 1 |
| `medulla-server-auth` | F4 | Request authentication, access checks and the SQL catalog ACL. | 12,546 | 13,046 | decision-engine 2, durable-graph-kernel 1, finance-primitives 1, engine-identity-and-scope 1 |
| `medulla-server-mutation` | F4 | Served mutation, mutation batches and transactions. | 16,520 | 17,020 | finance-primitives 2, engine-identity-and-scope 2, decision-engine 1 |
| `medulla-server-blob` | F4 | Served blob and key-value storage. | 7,993 | 8,393 | durable-graph-kernel 2, source-changesets 1, typed-packs-and-catalog 1 |
| `medulla-persistence` | F5 | Persistence backend: redb backend, durable stores, node info and cost accounting. | 17,856 | 18,456 | decision-engine 2, durable-graph-kernel 2, engine-parity 2 |
| `medulla-persistence-records` | F6 | Persisted agent, decision, connector pack and write-back records. | 15,521 | 16,621 | typed-packs-and-catalog 6, decision-engine 5 |
| `medulla-persistence-ops` | F6 | Backup, cold offload, online reshard, shard migration and store upgrade. | 10,066 | 10,266 | durable-graph-kernel 1, typed-packs-and-catalog 1 |
| `medulla-raft` | F7 | Raft consensus: command log, network, store, multi-raft and placement. | 19,843 | 20,243 | durable-graph-kernel 3, typed-packs-and-catalog 1 |
| `medulla-raft-xshard` | F8 | Cross-shard transactions and reads, elasticity and the raft test harnesses. | 17,453 | 17,553 | durable-graph-kernel 1 |
| `medulla-wire-core` | F8 | Shared wire listener framework. | 7,519 | 8,419 | durable-graph-kernel 9 |
| `medulla-wire` | F9 | SQL and graph wire listeners: pgwire, MySQL, MSSQL, SQLite and Bolt. | 11,072 | 11,572 | engine-parity 3, durable-graph-kernel 1, unified-data-plane 1 |
| `medulla-wire-msg` | F9 | Messaging and object listeners: AMQP, MQTT, STOMP, Redis, DDS, S3 and the KV cache endpoint. | 13,251 | 13,451 | durable-graph-kernel 2 |
| `medulla-h-query` | F9 | UnifiedQuery and knowledge stream handlers. | 15,188 | 22,628 | connector-competency 42, federated-query-and-reasoning 38, engine-parity 4, unified-data-plane 3 |
| `medulla-h-sparql` | F9 | SPARQL/RDF and federation handlers. | 5,514 | 6,134 | federated-query-and-reasoning 5, connector-competency 2 |
| `medulla-h-retrieve` | F9 | Discover, semantic index and reasoning projection handlers. | 9,087 | 9,447 | federated-query-and-reasoning 2, media-and-personal-vocabulary 1, unified-data-plane 1 |
| `medulla-h-mining` | F9 | Mining handlers. | 8,209 | 8,489 | supply-chain-vocabulary 2, orchestration-competency 1, repository-index-and-ingestion 1 |
| `medulla-h-admin` | F9 | Administrative handlers. | 15,577 | 17,537 | typed-packs-and-catalog 17, connector-competency 1, durable-graph-kernel 1, federated-query-and-reasoning 1 |
| `medulla-h-packs` | F9 | ConnectorPack.Import, schema sources and pack projection handlers. | 4,258 | 6,438 | typed-packs-and-catalog 15, connector-competency 2, decision-engine 2, federated-query-and-reasoning 1 |
| `medulla-h-decide` | F9 | Decide, AgentAssemble, DecisionCommit and DecisionLog handlers (thin: logic lives in the decision family). | 15,090 | 18,610 | decision-engine 21, orchestration-vocabulary 6, placement-and-worker-policy 4, typed-packs-and-catalog 3 |
| `medulla-h-compute` | F9 | Compute, finance, quantum and visualisation handlers. | 4,849 | 6,649 | finance-primitives 16, federated-query-and-reasoning 2 |
| `medulla-h-ingest` | F9 | SourceIngest, modality and repository enrichment handlers. | 8,606 | 9,686 | ingest-worker 4, media-and-personal-vocabulary 3, decision-engine 2, unified-data-plane 2 |
| `medulla-h-graph` | F9 | Graph operation and pipeline handlers. | 10,006 | 11,686 | orchestration-vocabulary 8, federated-query-and-reasoning 6, durable-graph-kernel 2, decision-engine 1 |
| `medulla-h-jobs` | F9 | Job, delegation and work item handlers. | 9,619 | 11,219 | durable-graph-kernel 4, placement-and-worker-policy 4, typed-packs-and-catalog 3, unified-data-plane 3 |
| `medulla-h-txn` | F9 | Transaction, SQLite file, timeseries and distributed compute handlers. | 10,630 | 10,830 | finance-primitives 2 |
| `medulla-server-graph` | F10 | Graph pipeline dispatch, SPARQL update dispatch and change envelopes. | 10,724 | 14,564 | ingest-worker 12, federated-query-and-reasoning 5, repository-index-and-ingestion 4, unified-data-plane 4 |
| `medulla-server-consensus` | F10 | Consensus dispatch. | 4,634 | 4,834 | ingest-worker 1, typed-packs-and-catalog 1 |
| `medulla-server-obs` | F10 | Observability, traces, telemetry and QoS. | 8,749 | 8,749 |  |
| `medulla-server-lake` | F10 | Lake serving, GraphQL subscriptions and policy export. | 7,663 | 8,363 | federated-query-and-reasoning 5, unified-data-plane 2 |
| `medulla-server-identity` | F10 | Identity store dispatch, request boundary and policy evolution dispatch. | 6,090 | 6,390 | decision-engine 1, durable-graph-kernel 1, engine-identity-and-scope 1 |
| `medulla-server-router` | F10 | Method router dispatch table. | 4,655 | 6,355 | decision-engine 5, engine-identity-and-scope 4, ingest-worker 4, durable-graph-kernel 1 |
| `medulla-server` | F11 | Server assembly: router, startup, transport and the remaining dispatch. | 18,695 | 19,355 | decision-engine 2, durable-graph-kernel 2, orchestration-vocabulary 1, repository-index-and-ingestion 1 |
| `medulla-engine` | F12 | The one server executable and the embedded engine. | 7,820 | 10,620 | public-engine-contract-and-release 28 |

Unchanged leaf crates receiving rows: `medulla-ann` (1), `medulla-capabilities` (27), `medulla-document` (8), `medulla-durable` (3), `medulla-enrich` (6), `medulla-epistemic` (5), `medulla-jobs` (3), `medulla-kvcache` (2), `medulla-lake` (11), `medulla-modality` (2), `medulla-program` (1), `medulla-resource` (1), `medulla-shacl` (8), `medulla-sqlite-format` (1), `medulla-statechart` (2), `medulla-tsdb` (7), `medulla-viz-core` (1)
