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
F11a server-served-tests | server-store-tests | server-cluster-tests                        (test crates, siblings)
F11 medulla-server (composition root)
F10 server-obs | server-lake < server-graph < server-consensus | server-identity < server-router
F9  h-sparql | h-retrieve | h-admin | h-packs | h-decide | h-compute | h-ingest | h-graph
    | h-jobs | h-txn | wire | wire-msg                                                      (siblings, no edges)
    < h-query (F9a) < h-mining (F9b)
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
L3  semantic-index < query-cache < graph-core < ann-codes < isolation < core (one chain, `[core_split]`) | reasoner | decision-stats | finance-market
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
rebuilds everything. It is split into nine crates (done 2026-10-31, each compiled on its own). Measured lines:

| Crate | Level | Lines | Holds |
|---|---|---|---|
| `medulla-types` | L0 | 21.4k | contract ids and bounded collections, msgpack, wire and wire-query types, ACL claims, series expressions, `rdf_report`, `solve_scalar` (exact scalars, `Sha256Digest`, `digest_text`), `lease_vocab`, test-support fixtures and the small unprefixed modules |
| `medulla-identity-types` | L1 | 16.9k | identity, RBAC, tenancy, ACL and authority |
| `medulla-mutation-types` | L1 | 9.4k | `mutation_scope` (scope identity, durability domain, committed version, outbox rows), commit descriptor, governed change, capacity, consensus |
| `medulla-index-types` | L1 | 10.6k | semantic index, embedding, managed index, entity resolution |
| `medulla-result-types` | L1 | 11.2k | answers, verdicts, epistemic nodes and predicates |
| `medulla-agent-types` | L1 | 22.5k | agents (incl. `agent_guardrail`, `agent_topology`), work items, jobs, runs, `native_control`, `control_lease`, `host_start` |
| `medulla-catalog-types` | L1 | 17.2k | packs, schema, catalog, ingest, lineage, release records, attached-source readers |
| `medulla-decision-types` | L1 | 18.1k | `decision/`, `policy_evolution`, `solve`, `current_plan` |
| `medulla-protocol-types` | L2 | 18.7k | `Method`, its envelopes (`mutation_batch`, `change_envelope`, `debezium_bridge`, `result_contract`, `storage_wire`), wire-query types, `messaging_wire`, `error_registry` |

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

How the moves ran: top-down along `[types_split].chain` (protocol hub first, identity
last), so every intermediate state compiled; `move_leaf_modules.py` allows an edge to any
crate earlier in the chain and stops on one that runs up it. `closed_error_codes!` and
`keyset_tenant_limit_methods!` are `#[macro_export]`ed. Test fixtures split per domain
(`<domain>_fixtures`, gated `any(test, feature = "test-support")`, moved with their
crate), because a crate's own unit tests use them and a separate fixtures crate would
hand those tests a second copy of every type; the seeded helpers stay in
`medulla_types::test_support`.

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
| `medulla-types` | L0 | Thin shared vocabulary: contract ids and bounded collections, msgpack limits, the wire and wire-query types, ACL request claims, series expressions, solver scalars and text digests, lease bounds, and the test-support fixtures; no domain types. | 21,440 | 21,440 |  |
| `medulla-protocol-types` | L2 | Protocol hub: the Method registry, its request/response envelopes (mutation batch, change envelope, result contract), the wire-query types and the published refusal-code registry. | 18,708 | 19,588 | repository-index-and-ingestion 2, durable-graph-kernel 1, federated-query-and-reasoning 1, ingest-worker 1, media-and-personal-vocabulary 1, orchestration-competency 1, orchestration-vocabulary 1, public-engine-contract-and-release 1, unified-data-plane 1 |
| `medulla-identity-types` | L1 | Issuer, principal, scope, tenancy, ACL and authority types. | 16,950 | 18,050 | engine-identity-and-scope 9, engine-parity 1, placement-and-worker-policy 1 |
| `medulla-agent-types` | L1b | Agent components, graphs, libraries, templates, work items, jobs and run history types. | 13,042 | 14,942 | orchestration-vocabulary 13, typed-packs-and-catalog 8, decision-engine 2, orchestration-competency 2 |
| `medulla-mutation-types` | L1 | Mutation scope, commit descriptor, outbox, governed change, lease capacity and consensus record types (Method-free). | 9,392 | 10,892 | durable-graph-kernel 8, source-changesets 4, branch-merge 3 |
| `medulla-index-types` | L1 | Semantic index, embedding, managed index and entity resolution types. | 10,631 | 10,631 |  |
| `medulla-result-types` | L1 | Answer, result contract, verdict, epistemic node, argument and query predicate types. | 11,243 | 14,843 | federated-query-and-reasoning 20, finance-primitives 11, orchestration-vocabulary 5, decision-engine 2 |
| `medulla-work-types` | L1a | Work items, work market, control leases, host start, jobs, worker registry, run history and orchestration record types. | 9,476 | 11,256 | orchestration-vocabulary 11, decision-engine 5, unified-data-plane 3, durable-graph-kernel 1, media-and-personal-vocabulary 1, orchestration-competency 1, placement-and-worker-policy 1 |
| `medulla-catalog-types` | L1c | Connector pack, schema, catalog, source ingestion, lineage and release record types. | 12,311 | 19,251 | typed-packs-and-catalog 32, unified-data-plane 14, ingest-worker 8, development-lifecycle 5, repository-index-and-ingestion 5, cross-source-competency 2, media-and-personal-vocabulary 2, durable-graph-kernel 1, federated-query-and-reasoning 1, source-authority 1 |
| `medulla-source-types` | L1d | Source ingestion, source documents and transcripts, ingest jobs and development-lifecycle record types (changeset review, pre-merge tests, standards drift, live artifacts, supply-chain reports). | 4,926 | 5,526 | ingest-worker 5, placement-and-worker-policy 1 |
| `medulla-decision-types` | L1e | Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records. | 11,937 | 22,157 | decision-engine 98, development-lifecycle 2, orchestration-vocabulary 2, placement-and-worker-policy 1 |
| `medulla-decision-outcome-types` | L1f | Decision outcome and governance types: answer ranking, guards, trust, triage, red team, promotion, recommendations, policy evolution and the current plan. | 6,166 | 6,766 | decision-engine 5, durable-graph-kernel 1 |
| `medulla-storage` | L2b | Physical storage owner, recovery, kernel, capabilities and scoped tables (one cycle; kept together). | 17,426 | 19,026 | durable-graph-kernel 13, object-storage-backend 2, engine-parity 1 |
| `medulla-direct-state` | L2a | Direct state: the storage owner's in-place state tables and their codecs. | 9,421 | 9,421 |  |
| `medulla-numeric` | L2 | Deterministic numeric kernels: series, risk bounds, stats, calibration, evaluation, clustering and the Python bridge. | 10,948 | 13,048 | finance-primitives 19, federated-query-and-reasoning 2 |
| `medulla-transaction` | L2 | Transaction graft, outbox and change capture. | 21,063 | 21,663 | durable-graph-kernel 5, finance-primitives 1 |
| `medulla-rdf` | L2 | RDF model, SPARQL algebra and update, and the serialisations (JSON-LD, RDF/XML, GeoSPARQL). | 11,772 | 12,272 | federated-query-and-reasoning 5 |
| `medulla-graph-core` | L3 | In-memory property graph, its indexes, paths, projections and JSONPath. | 18,614 | 24,074 | connector-competency 74, typed-packs-and-catalog 3, cross-source-competency 2, decision-engine 2, durable-graph-kernel 2, federated-query-and-reasoning 1, placement-and-worker-policy 1 |
| `medulla-isolation` | L3 | Tenant isolation, RBAC persistence and dependency scopes. | 8,219 | 11,519 | engine-identity-and-scope 30, decision-engine 2, federated-query-and-reasoning 1 |
| `medulla-core` | L3 | Engine core services that remain: broker, registry, lifecycle, durable apply and result cache. | 7,446 | 9,346 | typed-packs-and-catalog 6, orchestration-vocabulary 5, orchestration-competency 4, development-lifecycle 2, durable-graph-kernel 2, unified-data-plane 2, ingest-worker 1, media-and-personal-vocabulary 1 |
| `medulla-semantic-index` | L3 | Semantic ANN codes and the semantic index service. | 5,324 | 5,524 | federated-query-and-reasoning 1, repository-index-and-ingestion 1 |
| `medulla-reasoner` | L3 | OWL, tableau and rule reasoning, proof contract and reasoning closure. | 13,340 | 14,040 | federated-query-and-reasoning 3, durable-graph-kernel 2, typed-packs-and-catalog 2 |
| `medulla-decision-stats` | L3 | Statistical ranking for the decision ladder: fitted heads, calibration, conformal bounds, off-policy evaluation and scores. | 7,907 | 14,727 | decision-engine 63, orchestration-vocabulary 2, placement-and-worker-policy 2, development-lifecycle 1, typed-packs-and-catalog 1 |
| `medulla-finance-market` | L3 | Market data primitives: quotes, bars, books, calendars, corporate actions, indicators, finance signals, backtest validation and the special functions. | 5,937 | 6,037 | finance-primitives 1 |
| `medulla-finance-quant` | L4a | Finance quantitative math: pricing, derivatives, state-space filters, regimes, signal models, factors, optimizer, risk, leverage, execution kernels and sizing. | 5,054 | 6,454 | finance-primitives 14 |
| `medulla-ann-codes` | L3 | Semantic ANN code books: quantisation, encode and decode. | 20,229 | 20,229 |  |
| `medulla-sql` | L4d | SQL execution: DataFusion context, table providers, catalogs, ANN pushdown, spill and Iceberg federation scans. | 10,731 | 11,831 | engine-parity 7, unified-data-plane 3, durable-graph-kernel 1 |
| `medulla-sql-tests` | L4e | Integration tests of the SQL engine over tables, user tables, UDFs, PGQ and federation scans. | 7 | 7 |  |
| `medulla-cypher` | L4d | Cypher language, its executor, graph data science calls and the edge index. | 15,515 | 15,815 | unified-data-plane 3 |
| `medulla-tables` | L4a | Table schema model: schema, secondary index and migration definitions. | 3,780 | 3,780 |  |
| `medulla-table-store` | L4b | Physical table store, property-graph tables, the ANN authority and plan types, embedding bindings and plan scopes. | 18,822 | 19,922 | engine-parity 7, federated-query-and-reasoning 4 |
| `medulla-sql-udfs` | L4c | SQL scalar, numeric, sketch, window and table functions registered into the SQL engine. | 3,254 | 3,254 |  |
| `medulla-sql-lang` | L4c | SQL language front end: statement classification, SQL/PGQ graph tables, PL/pgSQL bodies, vector and hypertable DDL, function expansion and the typed result shapes. | 10,097 | 10,097 |  |
| `medulla-query` | L4 | Removed after the split: every module moves out; leftovers go to medulla-table-store. | 0 | 0 |  |
| `medulla-virtual-graph` | L4a | Attached sources read as virtual graphs: OBDA, source selection and supply-chain impact. | 6,580 | 10,960 | unified-data-plane 36, source-authority 6, cross-source-competency 3 |
| `medulla-graph-algos` | L4b | Graph algorithms and graph learning. | 16,375 | 16,875 | federated-query-and-reasoning 5 |
| `medulla-mining` | L4b | Pattern and process mining over graph records. | 9,302 | 9,702 | durable-graph-kernel 3, repository-index-and-ingestion 1 |
| `medulla-compute` | L4a | Compute expression language: parser, AST, data science operators and sketches. | 13,244 | 14,144 | source-changesets 4, decision-engine 2, ingest-worker 2, branch-merge 1 |
| `medulla-finance` | L4b | Finance portfolio layer: lot accounting, exchange records, imports, forensics, strategies, attribution, banking and the served finance entry points. | 3,995 | 8,095 | finance-primitives 41 |
| `medulla-decision-policy` | L3b | Ladder stages 3-5: policy, hard constraints, knobs, admission, refusals, budgets and the bounded exact solve. | 2,449 | 16,749 | decision-engine 142, placement-and-worker-policy 1 |
| `medulla-decision-rank` | L4 | Ladder stage 6: ranking the remaining legal options with fitted statistics (scores, champions, bandits, credit). | 8 | 6,408 | decision-engine 64 |
| `medulla-uql` | L5c | UQL syntax: lexer, grammar, parser, printer and diagnostics. | 4,499 | 5,199 | federated-query-and-reasoning 7 |
| `medulla-nlq` | L5e | Natural-language questions to UQL, with regressions. | 4,541 | 5,441 | federated-query-and-reasoning 9 |
| `medulla-federation` | L5d | Federated planning across sources: federation optimizer, SSRF guard and remote engines. | 10,582 | 15,262 | connector-competency 19, federated-query-and-reasoning 15, unified-data-plane 14, cross-source-competency 8, durable-graph-kernel 1, orchestration-competency 1 |
| `medulla-exec` | L5a | Physical plan execution: optimizer, cost models, runtime, DAG, incremental and streamed results. | 13,553 | 17,753 | federated-query-and-reasoning 27, finance-primitives 12, unified-data-plane 2, decision-engine 1 |
| `medulla-retrieval` | L5b | Retrieval and expansion: knowledge assembly, LeanRAG, verdicts and stop rules. | 5,711 | 19,611 | federated-query-and-reasoning 139 |
| `medulla-plan` | L5f | Planner entry: UQL serving, cross-modal plan tests and the engine-peer federation tests. | 12,296 | 15,596 | federated-query-and-reasoning 29, unified-data-plane 4 |
| `medulla-decision-plan` | L5 | Ladder stages 6-7 and task planning: AgentAssemble, bounded task graphs, delegation, harness and skill selection. | 5,141 | 10,501 | decision-engine 51, typed-packs-and-catalog 2, orchestration-vocabulary 1 |
| `medulla-packs` | L5 | Typed pack import, validation and roll-up to core classes. | 726 | 12,226 | typed-packs-and-catalog 114, federated-query-and-reasoning 1 |
| `medulla-catalog` | L5 | The catalog over packs, sources and schemas: lookup, lineage and discovery. | 8 | 508 | typed-packs-and-catalog 4, unified-data-plane 1 |
| `medulla-vocab` | L5 | Ontology modules and shapes for the data-platform, media, supply-chain and orchestration vocabularies. | 8 | 4,508 | orchestration-vocabulary 47, media-and-personal-vocabulary 27, supply-chain-vocabulary 1 |
| `medulla-ingest` | L5 | Ingest worker loop, lanes and queues. | 8 | 5,408 | ingest-worker 54 |
| `medulla-decision-records` | L6 | Decision records: receipts, ledger, the revision lifecycle (R805), provenance, replay and feedback. | 8 | 9,108 | decision-engine 91 |
| `medulla-decision-eval` | L6 | Decision evaluation: served proof suites, quality and relevance scoring, drift and nightly evaluation. | 8 | 10,208 | decision-engine 102 |
| `medulla-decision-memory` | L6 | Decision context and memory: session memory, retrieval for decisions, temporal and indexless recall. | 8 | 5,208 | decision-engine 52 |
| `medulla-competency` | L6 | Connector competency questions answered through the served query path. | 8 | 4,508 | connector-competency 75 |
| `medulla-competency-xsrc` | L6 | Cross-source and orchestration competency questions. | 8 | 2,948 | orchestration-competency 28, cross-source-competency 16, connector-competency 5 |
| `medulla-repo-index` | L6 | Repository indexing, source changesets and branch merge as graph records. | 8 | 4,908 | repository-index-and-ingestion 36, source-changesets 9, branch-merge 4 |
| `medulla-sdlc` | L6 | Graph-native development lifecycle: spec rows, gates, verdicts and releases as records. | 8 | 5,708 | development-lifecycle 57 |
| `medulla-parity` | L6 | Engine compatibility features and their parity proofs against other engines' behaviour. | 8 | 2,908 | engine-parity 29 |
| `medulla-placement` | L6 | Worker admission and placement policy. | 8 | 2,008 | placement-and-worker-policy 20 |
| `medulla-kernel` | F0 | Persist, store authority, mutation apply and channels. | 8,818 | 13,918 | durable-graph-kernel 50, engine-identity-and-scope 1 |
| `medulla-store` | F1 | redb store core, layout, sidecar and shards. | 8,751 | 8,751 |  |
| `medulla-work-store` | F2a | Durable work items, their capability records and graph-row tables. | 10,878 | 11,678 | durable-graph-kernel 7, unified-data-plane 1 |
| `medulla-resource-store` | F2a | Resource reservations and capacity leases. | 5,938 | 6,638 | durable-graph-kernel 7 |
| `medulla-lane-store` | F2c | Development lane records in the store. | 6,724 | 6,724 |  |
| `medulla-store-apply` | F2b | Store apply composition: native apply dispatch over work and resource rows, control, cleanup, checkpoints and the whole-batch store tests. | 24,076 | 24,076 |  |
| `medulla-server-api` | F3 | Server state, seam traits, request context, transactions and the shared served-API helpers. | 15,458 | 15,458 |  |
| `medulla-server-auth` | F4a | Request authentication, access checks, the SQL catalog ACL and SQL tables. | 12,446 | 12,946 | decision-engine 2, durable-graph-kernel 1, engine-identity-and-scope 1, finance-primitives 1 |
| `medulla-server-cdc` | F4b | Change data capture hub, CEP, materialised views, reasoning cascade and impact watch. | 4,664 | 4,664 |  |
| `medulla-server-mutation` | F4c | Served mutation and mutation batches. | 8,822 | 9,322 | engine-identity-and-scope 2, finance-primitives 2, decision-engine 1 |
| `medulla-server-blob` | F4d | Served blob, key-value and SPARQL HTTP endpoints. | 10,046 | 10,446 | durable-graph-kernel 2, source-changesets 1, typed-packs-and-catalog 1 |
| `medulla-server-ops` | F4e | Served API handler seams: the admin, decide, ingest, graph, jobs, compute, query, transaction and wire seam traits, their typed accessors and their support modules. | 5,753 | 5,753 |  |
| `medulla-persistence` | F5 | Persistence backend: redb backend, cold offload and tiering, read-through, cost accounting, node info and durable stores. | 24,520 | 24,520 |  |
| `medulla-persistence-records` | F6a | Persisted agent, decision, connector pack, guardrail and write-back records. | 16,344 | 17,544 | typed-packs-and-catalog 7, decision-engine 5 |
| `medulla-persistence-ops` | F6b | Backup, online reshard, shard migration and store upgrade. | 2,750 | 2,950 | durable-graph-kernel 1, typed-packs-and-catalog 1 |
| `medulla-raft` | F7 | Raft consensus: command log, network, store, multi-raft and placement. | 16,254 | 16,454 | durable-graph-kernel 2 |
| `medulla-raft-xshard` | F8 | Cross-shard transactions, elasticity, exchange, pregel, drain and the raft test harnesses. | 11,649 | 11,749 | durable-graph-kernel 1 |
| `medulla-wire-core` | F8 | Shared wire listener framework: session and outcome traits, broker queue primitives and SQL wire authentication. | 5,928 | 6,928 | durable-graph-kernel 9, engine-parity 1 |
| `medulla-wire` | F9 | SQL and graph wire listeners: pgwire, MySQL, MSSQL, SQLite and Bolt. | 10,121 | 10,221 | engine-parity 1 |
| `medulla-wire-msg` | F9 | Messaging and object listeners: AMQP, MQTT, STOMP, Redis, DDS/ROS 2, S3 and the KV cache endpoint. | 13,861 | 14,061 | durable-graph-kernel 2 |
| `medulla-h-query` | F9a | Query handlers: UnifiedQuery, SQL, Cypher, GraphQL, NL query, explain, request cancel, streaming and the knowledge stream. | 13,954 | 13,954 |  |
| `medulla-h-sparql` | F9 | SPARQL/RDF, SPARQL service and federated-search handlers, including foreign source registration. | 6,424 | 7,244 | federated-query-and-reasoning 6, connector-competency 2, typed-packs-and-catalog 1 |
| `medulla-h-retrieve` | F9 | Semantic, text, spatial and secondary index runtimes and the reasoning projection handlers. | 8,683 | 9,043 | federated-query-and-reasoning 2, media-and-personal-vocabulary 1, unified-data-plane 1 |
| `medulla-h-mining` | F9b | Mining handlers over the served query path. | 9,091 | 9,371 | supply-chain-vocabulary 2, orchestration-competency 1, repository-index-and-ingestion 1 |
| `medulla-h-admin` | F9 | Administrative handlers. | 17,460 | 17,460 |  |
| `medulla-h-packs` | F9 | ConnectorPack.Import, schema sources and pack projection handlers. | 8 | 2,588 | typed-packs-and-catalog 18, connector-competency 2, decision-engine 2, federated-query-and-reasoning 2, finance-primitives 1, orchestration-vocabulary 1, source-authority 1 |
| `medulla-h-decide` | F9 | Decide, AgentAssemble, DecisionCommit and DecisionLog handlers (thin: logic lives in the decision family). | 15,401 | 15,401 |  |
| `medulla-h-compute` | F9 | Compute, finance, quantum and visualisation handlers. | 4,986 | 4,986 |  |
| `medulla-h-ingest` | F9 | SourceIngest, modality and repository enrichment handlers. | 8,731 | 8,731 |  |
| `medulla-h-graph` | F9 | Graph operation and pipeline handlers. | 9,325 | 9,325 |  |
| `medulla-h-jobs` | F9 | Job, delegation and work item handlers. | 9,985 | 9,985 |  |
| `medulla-h-txn` | F9 | Transaction, SQLite file, timeseries and distributed compute handlers. | 10,730 | 10,730 |  |
| `medulla-server-graph` | F10b | Graph-op dispatch pipeline: graph pipeline, change envelopes, SPARQL update, policy evolution, request payload preflight and the shared dispatch helpers. | 12,549 | 16,589 | ingest-worker 12, federated-query-and-reasoning 5, repository-index-and-ingestion 4, unified-data-plane 4, decision-engine 3, durable-graph-kernel 3, orchestration-vocabulary 3, branch-merge 2, typed-packs-and-catalog 2, engine-identity-and-scope 1, finance-primitives 1, media-and-personal-vocabulary 1, source-changesets 1 |
| `medulla-server-consensus` | F10c | Consensus dispatch: replicated apply, native proposal routing, transactions, job publication, server registry and fleet catalog. | 4,726 | 4,926 | ingest-worker 1, typed-packs-and-catalog 1 |
| `medulla-server-obs` | F10a | Observability serving: logs, metrics, PromQL, OTLP traces and QoS classes. | 8,260 | 8,260 |  |
| `medulla-server-lake` | F10a | Lake serving, GraphQL subscriptions and policy export. | 7,764 | 8,464 | federated-query-and-reasoning 5, unified-data-plane 2 |
| `medulla-server-identity` | F10c | Identity dispatch: identity stamping, signing secrets, resolution context, request elevation and governed actors. | 1,494 | 1,594 | engine-identity-and-scope 1 |
| `medulla-server-router` | F10d | Request entry and method router: request boundary, method tables, telemetry methods, the dispatch seam and the end-to-end dispatch tests. | 11,488 | 13,288 | decision-engine 5, engine-identity-and-scope 4, ingest-worker 4, durable-graph-kernel 2, development-lifecycle 1, engine-parity 1, federated-query-and-reasoning 1 |
| `medulla-server` | F11 | Server composition root: startup, seam installation, transport, issuer trust, replica, registry reaper, retention and CDC sink. | 6,002 | 6,002 |  |
| `medulla-server-served-tests` | F11a | End-to-end served tests: the full dispatch chain, handler and wire protocol round trips through the assembled server. | 12,932 | 12,932 |  |
| `medulla-server-store-tests` | F11a | Served mutation, batch, saga, persistence and store run-owner tests across the server API and store crates. | 7,347 | 7,347 |  |
| `medulla-server-cluster-tests` | F11a | Cluster tests: raft clusters and commands, cross-shard harness, placement and resource acceptance. | 10,069 | 10,069 |  |
| `medulla-engine` | F12 | The one server executable and the embedded engine. | 7,513 | 10,313 | public-engine-contract-and-release 28 |

Unchanged leaf crates receiving rows: `medulla-ann` (1), `medulla-capabilities` (27), `medulla-document` (8), `medulla-durable` (3), `medulla-enrich` (6), `medulla-epistemic` (5), `medulla-jobs` (3), `medulla-kvcache` (2), `medulla-lake` (11), `medulla-modality` (2), `medulla-program` (1), `medulla-resource` (1), `medulla-shacl` (8), `medulla-sqlite-format` (1), `medulla-statechart` (2), `medulla-tsdb` (7), `medulla-viz-core` (1)

## G7 split of the large crates

Every crate that is over 25,000 lines today, or projected over it, splits per one manifest in
`g7/` (see `g7/README.md`). Levels with a letter suffix order crates inside one level band
(`L4a` < `L4b`); a crate depends only on strictly lower levels. Deps below are the
in-family edges; lines are measured and projected by `route_rows.py --update`.

| Source | Target | Level | Responsibility | In-family deps | Today | Projected |
|---|---|---|---|---|---:|---:|
| `medulla-agent-types` | `medulla-work-types` | L1a | Work items, work market, control leases, host start, jobs, worker registry, run history and orchestration record types. | - | 9,390 | 10,970 |
| `medulla-agent-types` | `medulla-agent-types` | L1b | Agent components, graphs, libraries, templates, work items, jobs and run history types. | `medulla-work-types` | 13,118 | 15,118 |
| `medulla-catalog-types` | `medulla-catalog-types` | L1c | Connector pack, schema, catalog, source ingestion, lineage and release record types. | - | 12,346 | 19,286 |
| `medulla-catalog-types` | `medulla-source-types` | L1d | Source ingestion, source documents and transcripts, ingest jobs and development-lifecycle record types (changeset review, pre-merge tests, standards drift, live artifacts, supply-chain reports). | `medulla-catalog-types` | 4,881 | 5,481 |
| `medulla-compute` | `medulla-reasoner` | L3 | OWL, tableau and rule reasoning, proof contract and reasoning closure. | - | 13,290 | 13,890 |
| `medulla-compute` | `medulla-decision-policy` | L3b | Ladder stages 3-5: policy, hard constraints, knobs, admission, refusals, budgets and the bounded exact solve. | - | 2,428 | 16,728 |
| `medulla-compute` | `medulla-compute` | L4a | Compute expression language: parser, AST, data science operators and sketches. | `medulla-decision-policy` | 13,285 | 14,185 |
| `medulla-compute` | `medulla-finance-market` | L3 | Market data primitives: quotes, bars, books, calendars, corporate actions, indicators, finance signals, backtest validation and the special functions. | - | 5,645 | 5,745 |
| `medulla-compute` | `medulla-graph-algos` | L4b | Graph algorithms and graph learning. | `medulla-compute` | 16,128 | 16,628 |
| `medulla-compute` | `medulla-finance-quant` | L4a | Finance quantitative math: pricing, derivatives, state-space filters, regimes, signal models, factors, optimizer, risk, leverage, execution kernels and sizing. | `medulla-finance-market` | 5,308 | 6,708 |
| `medulla-compute` | `medulla-mining` | L4b | Pattern and process mining over graph records. | - | 9,511 | 9,611 |
| `medulla-compute` | `medulla-finance` | L4b | Finance portfolio layer: lot accounting, exchange records, imports, forensics, strategies, attribution, banking and the served finance entry points. | - | 4,008 | 8,108 |
| `medulla-decision-types` | `medulla-decision-types` | L1e | Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records. | - | 11,976 | 21,996 |
| `medulla-decision-types` | `medulla-decision-outcome-types` | L1f | Decision outcome and governance types: answer ranking, guards, trust, triage, red team, promotion, recommendations, policy evolution and the current plan. | `medulla-decision-types` | 6,098 | 6,698 |
| `medulla-numeric` | `medulla-numeric` | L2 | Deterministic numeric kernels: series, risk bounds, stats, calibration, evaluation, clustering and the Python bridge. | - | 10,958 | 12,958 |
| `medulla-numeric` | `medulla-decision-stats` | L3 | Statistical ranking for the decision ladder: fitted heads, calibration, conformal bounds, off-policy evaluation and scores. | `medulla-numeric` | 7,910 | 14,330 |
| `medulla-numeric` | `medulla-finance-quant` | L4a | Finance quantitative math: pricing, derivatives, state-space filters, regimes, signal models, factors, optimizer, risk, leverage, execution kernels and sizing. | `medulla-numeric` | 5,308 | 6,708 |
| `medulla-numeric` | `medulla-finance` | L4b | Finance portfolio layer: lot accounting, exchange records, imports, forensics, strategies, attribution, banking and the served finance entry points. | `medulla-numeric` | 4,008 | 8,108 |
| `medulla-persistence` | `medulla-persistence` | F5 | Persistence backend: redb backend, cold offload and tiering, read-through, cost accounting, node info and durable stores. | - | 18,112 | 18,112 |
| `medulla-persistence` | `medulla-persistence-records` | F6a | Persisted agent, decision, connector pack, guardrail and write-back records. | - | 16,299 | 17,399 |
| `medulla-persistence` | `medulla-persistence-ops` | F6b | Backup, online reshard, shard migration and store upgrade. | `medulla-persistence`, `medulla-persistence-records` | 9,162 | 9,362 |
| `medulla-plan` | `medulla-exec` | L5a | Physical plan execution: optimizer, cost models, runtime, DAG, incremental and streamed results. | - | 13,336 | 15,436 |
| `medulla-plan` | `medulla-retrieval` | L5b | Retrieval and expansion: knowledge assembly, LeanRAG, verdicts and stop rules. | `medulla-exec` | 5,262 | 18,262 |
| `medulla-plan` | `medulla-uql` | L5c | UQL syntax: lexer, grammar, parser, printer and diagnostics. | `medulla-exec` | 4,491 | 5,191 |
| `medulla-plan` | `medulla-federation` | L5d | Federated planning across sources: federation optimizer, SSRF guard and remote engines. | `medulla-exec`, `medulla-uql` | 10,925 | 15,545 |
| `medulla-plan` | `medulla-nlq` | L5e | Natural-language questions to UQL, with regressions. | `medulla-exec`, `medulla-federation`, `medulla-uql` | 4,191 | 4,991 |
| `medulla-plan` | `medulla-plan` | L5f | Planner entry: UQL serving, cross-modal plan tests and the engine-peer federation tests. | `medulla-exec`, `medulla-federation`, `medulla-nlq`, `medulla-retrieval`, `medulla-uql` | 12,819 | 15,919 |
| `medulla-query` | `medulla-tables` | L4a | Table schema model: schema, secondary index and migration definitions. | - | 3,770 | 3,770 |
| `medulla-query` | `medulla-table-store` | L4b | Physical table store, property-graph tables, the ANN authority and plan types, embedding bindings and plan scopes. | `medulla-tables` | 19,953 | 20,653 |
| `medulla-query` | `medulla-sql-udfs` | L4c | SQL scalar, numeric, sketch, window and table functions registered into the SQL engine. | `medulla-tables` | 3,220 | 3,320 |
| `medulla-query` | `medulla-cypher` | L4d | Cypher language, its executor, graph data science calls and the edge index. | `medulla-table-store` | 15,474 | 15,574 |
| `medulla-query` | `medulla-sql` | L4d | SQL dialect: parse, classify, plan and execute over tables, including the table providers. | `medulla-sql-udfs`, `medulla-table-store`, `medulla-tables` | 19,744 | 20,644 |
| `medulla-raft` | `medulla-raft` | F7 | Raft consensus: command log, network, store, multi-raft and placement. | - | 16,306 | 16,506 |
| `medulla-raft` | `medulla-raft-xshard` | F8 | Cross-shard transactions, elasticity, exchange, pregel, drain and the raft test harnesses. | `medulla-raft` | 11,543 | 11,643 |
| `medulla-rdf` | `medulla-rdf` | L2 | RDF model, SPARQL algebra and update, and the serialisations (JSON-LD, RDF/XML, GeoSPARQL). | - | 11,828 | 12,328 |
| `medulla-rdf` | `medulla-reasoner` | L3 | OWL, tableau and rule reasoning, proof contract and reasoning closure. | `medulla-rdf` | 13,290 | 13,890 |
| `medulla-rdf` | `medulla-virtual-graph` | L4a | Attached sources read as virtual graphs: OBDA, source selection and supply-chain impact. | `medulla-rdf` | 6,556 | 10,636 |
| `medulla-server-api` | `medulla-server-api` | F3 | Server state, seam traits, request context, transactions and the shared served-API helpers. | - | 8,925 | 8,925 |
| `medulla-server-api` | `medulla-server-auth` | F4a | Request authentication, access checks, the SQL catalog ACL and SQL tables. | `medulla-server-api` | 13,107 | 13,607 |
| `medulla-server-api` | `medulla-server-cdc` | F4b | Change data capture hub, CEP, materialised views, reasoning cascade and impact watch. | `medulla-server-api`, `medulla-server-auth` | 4,491 | 4,491 |
| `medulla-server-api` | `medulla-server-mutation` | F4c | Served mutation and mutation batches. | `medulla-server-api`, `medulla-server-auth`, `medulla-server-cdc` | 12,245 | 12,745 |
| `medulla-server-api` | `medulla-server-blob` | F4d | Served blob, key-value and SPARQL HTTP endpoints. | `medulla-server-api`, `medulla-server-auth`, `medulla-server-mutation` | 9,993 | 10,393 |
| `medulla-server-api` | `medulla-server-ops` | F4e | Served API seams: admin, ingest, query-transaction and re-entry seams and their registry. | `medulla-server-api`, `medulla-server-auth`, `medulla-server-blob`, `medulla-server-mutation` | 7,804 | 7,804 |
| `medulla-storage` | `medulla-direct-state` | L2a | Direct state: the storage owner's in-place state tables and their codecs. | - | 9,410 | 9,410 |
| `medulla-storage` | `medulla-storage` | L2b | Physical storage owner, recovery, kernel, capabilities and scoped tables (one cycle; kept together). | `medulla-direct-state` | 17,427 | 18,227 |
| `medulla-store` | `medulla-store` | F1 | redb store core, layout, sidecar and shards. | - | 21,529 | 21,529 |
| `medulla-store` | `medulla-resource-store` | F2a | Resource reservations and capacity leases. | `medulla-store` | 9,512 | 10,212 |
| `medulla-store` | `medulla-work-store` | F2a | Durable work items, their capability records and graph-row tables. | `medulla-store` | 12,446 | 13,246 |
| `medulla-store` | `medulla-store-apply` | F2b | Store apply composition: native apply dispatch over work and resource rows, control, cleanup, checkpoints and the whole-batch store tests. | `medulla-resource-store`, `medulla-store`, `medulla-work-store` | 2,447 | 2,447 |
| `medulla-store` | `medulla-lane-store` | F2c | Development lane records in the store. | `medulla-store` | 10,383 | 10,383 |

Finance family: `medulla-finance-market` (L3, market data, calendars, signals, backtest
validation) < `medulla-finance-quant` (L4a, pricing, filters, regimes, factors, risk, sizing,
execution kernels) < `medulla-finance` (L4b, accounting, portfolio, attribution, served entry
points). `medulla-numeric` keeps the shared risk bounds, calibration and evaluation kernels.
`medulla-tsdb` reads finance-market and so sits above L3.

`medulla-storage-owner` is dropped: owner, physical, recovery, kernel, capability and scoped
form one cycle, so `direct_state` (no dependencies) moves out instead.

### The last four crates over the limit (G7b)

`medulla-server` (82,114 lines), `medulla-h-query` (38,209), `medulla-sql` (30,091 with its
integration tests) and `medulla-wire` (29,901) split per `g7/medulla-{server,h-query,sql,wire}.toml`.
Every target is at most 18,000 lines today. Ranks come from `g7/modgraph.py` on the real module
tree: `#[path]` files, inline modules, `super::` chains at any depth, globs (only the names the
importing code uses) and re-exports. `check.py` reports 0 violations on each manifest. "Today"
is the manifest's projection after its cuts; "Projected" adds the open spec rows routed there.

| Source | Target | Level | Responsibility | In-family deps | Today | Projected |
|---|---|---|---|---|---:|---:|
| `medulla-h-query` | `medulla-h-sparql` | F9 | SPARQL/RDF, SPARQL service and federated-search handlers, including foreign source registration. | - | 6,398 | 7,218 |
| `medulla-h-query` | `medulla-h-retrieve` | F9 | Semantic, text, spatial and secondary index runtimes and the reasoning projection handlers. | - | 8,660 | 9,020 |
| `medulla-h-query` | `medulla-h-query` | F9a | Query handlers: UnifiedQuery, SQL, Cypher, GraphQL, NL query, explain, request cancel, streaming and the knowledge stream. | `medulla-h-retrieve`, `medulla-h-sparql` | 14,001 | 14,001 |
| `medulla-h-query` | `medulla-h-mining` | F9b | Mining handlers over the served query path. | `medulla-h-query` | 9,078 | 9,358 |
| `medulla-h-query` | `medulla-server` | F11 | Server composition root: startup, seam installation, transport, issuer trust, replica, registry reaper, retention and CDC sink. | `medulla-h-mining`, `medulla-h-query`, `medulla-h-retrieve` | 72 | 72 |
| `medulla-server` | `medulla-server-lake` | F10a | Lake serving, GraphQL subscriptions and policy export. | - | 7,721 | 8,421 |
| `medulla-server` | `medulla-server-obs` | F10a | Observability serving: logs, metrics, PromQL, OTLP traces and QoS classes. | - | 8,205 | 8,205 |
| `medulla-server` | `medulla-server-graph` | F10b | Graph-op dispatch pipeline: graph pipeline, change envelopes, SPARQL update, policy evolution, request payload preflight and the shared dispatch helpers. | - | 12,460 | 16,500 |
| `medulla-server` | `medulla-server-consensus` | F10c | Consensus dispatch: replicated apply, native proposal routing, transactions, job publication, server registry and fleet catalog. | `medulla-server-graph` | 4,665 | 4,865 |
| `medulla-server` | `medulla-server-identity` | F10c | Identity dispatch: identity stamping, signing secrets, resolution context, request elevation and governed actors. | `medulla-server-graph` | 1,439 | 1,539 |
| `medulla-server` | `medulla-server-router` | F10d | Request entry and method router: request boundary, method tables, telemetry methods, the dispatch seam and the end-to-end dispatch tests. | `medulla-server-consensus`, `medulla-server-graph`, `medulla-server-identity`, `medulla-server-obs` | 11,175 | 12,975 |
| `medulla-server` | `medulla-server` | F11 | Server composition root: startup, seam installation, transport, issuer trust, replica, registry reaper, retention and CDC sink. | `medulla-server-consensus`, `medulla-server-graph`, `medulla-server-lake`, `medulla-server-obs`, `medulla-server-router` | 6,240 | 6,240 |
| `medulla-server` | `medulla-server-store-tests` | F11a | Served mutation, batch, saga, persistence and store run-owner tests across the server API and store crates. | `medulla-server-router` | 7,305 | 7,305 |
| `medulla-server` | `medulla-server-cluster-tests` | F11a | Cluster tests: raft clusters and commands, cross-shard harness, placement and resource acceptance. | `medulla-server`, `medulla-server-router` | 10,031 | 10,031 |
| `medulla-server` | `medulla-server-served-tests` | F11a | End-to-end served tests: the full dispatch chain, handler and wire protocol round trips through the assembled server. | `medulla-server`, `medulla-server-router` | 12,873 | 12,873 |
| `medulla-sql` | `medulla-sql-lang` | L4c | SQL language front end: statement classification, SQL/PGQ graph tables, PL/pgSQL bodies, vector and hypertable DDL, function expansion and the typed result shapes. | - | 10,057 | 10,057 |
| `medulla-sql` | `medulla-sql` | L4d | SQL execution: DataFusion context, table providers, catalogs, ANN pushdown, spill and Iceberg federation scans. | `medulla-sql-lang` | 10,764 | 11,864 |
| `medulla-sql` | `medulla-sql-tests` | L4e | Integration tests of the SQL engine over tables, user tables, UDFs, PGQ and federation scans. | `medulla-sql`, `medulla-sql-lang` | 9,270 | 9,270 |
| `medulla-wire` | `medulla-wire-core` | F8 | Shared wire listener framework: session and outcome traits, broker queue primitives and SQL wire authentication. | - | 5,919 | 6,919 |
| `medulla-wire` | `medulla-wire` | F9 | SQL and graph wire listeners: pgwire, MySQL, MSSQL, SQLite and Bolt. | `medulla-wire-core` | 10,163 | 10,263 |
| `medulla-wire` | `medulla-wire-msg` | F9 | Messaging and object listeners: AMQP, MQTT, STOMP, Redis, DDS/ROS 2, S3 and the KV cache endpoint. | `medulla-wire-core` | 13,819 | 14,019 |

Server shape. `medulla-server` keeps only the composition root (startup, seam installation,
transport, issuer trust, replica, registry reaper, retention, CDC sink). The dispatch tree splits
by served domain: graph-op pipeline (`server-graph`), consensus and fleet registry
(`server-consensus`), identity and elevation (`server-identity`), and the request entry plus
method router on top (`server-router`). Observability and lake serving are leaves (F10a).
The three production re-entries below the router (`consensus/replicated.rs`, two in
`sparql_update.rs`) go through the server-api `DispatchApi` seam; that is the one
behaviour-sensitive cut, and every test state that reaches them installs the `DispatchSeam`.
The 30k lines of cross-crate tests (`split_tests/`, the lib.rs `mod tests`) leave the root for
three test crates, and the dispatch tests that drive the full chain move up to `server-router`.

Query handlers. `h-sparql` and `h-retrieve` stay F9 siblings; `h-query` (F9a) uses them, and
`h-mining` (F9b) runs the served query path. `QuerySeam` moves to the composition root.

SQL. The language front end (`sql-lang`: classify, PGQ, PL/pgSQL, DDL, typed results) sits
under execution (`sql`); `crates/medulla-sql/tests` becomes `medulla-sql-tests`. Wire: the shared
framework (`wire::`, broker primitives, SQL wire auth) moves to the existing `wire-core`
scaffold; the messaging and object listeners fill `wire-msg`; `wire` keeps the SQL and Bolt
listeners.

Regenerate: `modgraph.py <crate> <expand...>` into `g-server`/`g-hquery`/`g-sql`/`g-wire`,
then `gen_manifests.py ... <graph dir> medulla-server medulla-h-query medulla-sql medulla-wire`,
`apply_boundaries.py medulla-server medulla-h-query medulla-sql medulla-wire` (scoped: earlier
lanes' hand-applied deviations stay) and `route_rows.py --update --check`.
