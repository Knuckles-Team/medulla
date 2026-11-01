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
| `medulla-protocol-types` | L2 | Protocol hub: the Method registry, its request/response envelopes (mutation batch, change envelope, result contract), the wire-query types and the published refusal-code registry. | 18,727 | 19,447 | repository-index-and-ingestion 2, durable-graph-kernel 1, federated-query-and-reasoning 1, orchestration-competency 1, orchestration-vocabulary 1, public-engine-contract-and-release 1, unified-data-plane 1 |
| `medulla-identity-types` | L1 | Issuer, principal, scope, tenancy, ACL and authority types. | 16,950 | 17,950 | engine-identity-and-scope 9, placement-and-worker-policy 1 |
| `medulla-agent-types` | L1 | Agent components, graphs, libraries, templates, work items, jobs and run history types. | 22,508 | 26,028 | orchestration-vocabulary 23, typed-packs-and-catalog 8, decision-engine 7, orchestration-competency 3, durable-graph-kernel 2, media-and-personal-vocabulary 1, placement-and-worker-policy 1, unified-data-plane 1 |
| `medulla-mutation-types` | L1 | Mutation scope, commit descriptor, outbox, governed change, lease capacity and consensus record types (Method-free). | 9,392 | 11,692 | durable-graph-kernel 16, source-changesets 4, branch-merge 3 |
| `medulla-index-types` | L1 | Semantic index, embedding, managed index and entity resolution types. | 10,631 | 10,731 | unified-data-plane 1 |
| `medulla-result-types` | L1 | Answer, result contract, verdict, epistemic node, argument and query predicate types. | 11,243 | 14,683 | federated-query-and-reasoning 19, finance-primitives 11, orchestration-vocabulary 4, decision-engine 2 |
| `medulla-catalog-types` | L1 | Connector pack, schema, catalog, source ingestion, lineage and release record types. | 17,227 | 25,567 | typed-packs-and-catalog 32, unified-data-plane 20, ingest-worker 13, development-lifecycle 5, repository-index-and-ingestion 5, source-authority 3, cross-source-competency 2, media-and-personal-vocabulary 2, durable-graph-kernel 1, federated-query-and-reasoning 1, placement-and-worker-policy 1 |
| `medulla-decision-types` | L1 | Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records. | 18,074 | 28,794 | decision-engine 102, development-lifecycle 2, orchestration-vocabulary 2, durable-graph-kernel 1, placement-and-worker-policy 1 |
| `medulla-storage` | L2 | Physical storage owner, direct state, recovery and scoped tables. | 17,310 | 18,410 | durable-graph-kernel 8, object-storage-backend 2, engine-parity 1 |
| `medulla-storage-owner` | L2 | Storage owner process: owner state machine and ownership handoff. | 9,535 | 9,535 |  |
| `medulla-numeric` | L2 | Deterministic numeric kernels: series, random, stats, clustering and the Python bridge. | 7,440 | 9,040 | finance-primitives 14, federated-query-and-reasoning 2 |
| `medulla-transaction` | L2 | Transaction graft, outbox and change capture. | 21,063 | 21,763 | durable-graph-kernel 6, finance-primitives 1 |
| `medulla-rdf` | L2 | RDF model, SPARQL algebra and update, and the serialisations (JSON-LD, RDF/XML, GeoSPARQL). | 10,783 | 11,563 | federated-query-and-reasoning 5, cross-source-competency 3, source-authority 1 |
| `medulla-graph-core` | L3 | In-memory property graph, its indexes, paths, projections and JSONPath. | 17,409 | 22,969 | connector-competency 74, durable-graph-kernel 3, typed-packs-and-catalog 3, cross-source-competency 2, decision-engine 2, federated-query-and-reasoning 1, placement-and-worker-policy 1 |
| `medulla-isolation` | L3 | Tenant isolation, RBAC persistence and dependency scopes. | 9,316 | 12,116 | engine-identity-and-scope 26, decision-engine 2 |
| `medulla-core` | L3 | Engine core services that remain: broker, registry, lifecycle, durable apply and result cache. | 9,447 | 11,347 | typed-packs-and-catalog 6, orchestration-vocabulary 5, orchestration-competency 4, development-lifecycle 2, durable-graph-kernel 2, unified-data-plane 2, ingest-worker 1, media-and-personal-vocabulary 1 |
| `medulla-semantic-index` | L3 | Semantic ANN codes and the semantic index service. | 9,212 | 9,412 | federated-query-and-reasoning 1, repository-index-and-ingestion 1 |
| `medulla-reasoner` | L3 | OWL, tableau and rule reasoning plus reasoning closure. | 13,060 | 14,360 | durable-graph-kernel 9, federated-query-and-reasoning 3, typed-packs-and-catalog 1 |
| `medulla-decision-stats` | L3 | Statistical ranking for the decision ladder: fitted heads, calibration, conformal bounds, off-policy evaluation and scores. | 9,928 | 17,148 | decision-engine 60, finance-primitives 8, orchestration-vocabulary 2, placement-and-worker-policy 2, development-lifecycle 1 |
| `medulla-finance-market` | L3 | Market data primitives: quotes, bars, books and their kernels. | 5,466 | 5,566 | finance-primitives 1 |
| `medulla-ann-codes` | L3 | Semantic ANN code books: quantisation, encode and decode. | 16,021 | 16,021 |  |
| `medulla-sql` | L4 | SQL dialect: parse, classify, plan and execute over tables. | 21,862 | 22,862 | engine-parity 6, unified-data-plane 3, durable-graph-kernel 1 |
| `medulla-cypher` | L4 | Cypher language, its executor, graph data science calls and the edge index. | 15,474 | 15,974 | engine-parity 4, unified-data-plane 1 |
| `medulla-tables` | L4 | Table model: schema, property graph tables and ANN authority (everything but the table store). | 12,702 | 13,002 | engine-parity 2, federated-query-and-reasoning 1 |
| `medulla-table-store` | L4 | Physical table store under the table model. | 12,123 | 12,523 | engine-parity 4 |
| `medulla-query` | L4 | Removed after the split: every module moves out (tables become medulla-tables). | 0 | 0 |  |
| `medulla-virtual-graph` | L4 | Attached sources read as virtual graphs: OBDA mappings, source selection and source authority checks. | 7,831 | 15,531 | unified-data-plane 66, source-authority 11 |
| `medulla-graph-algos` | L4 | Graph algorithms and graph learning. | 15,853 | 16,353 | federated-query-and-reasoning 5 |
| `medulla-mining` | L4 | Pattern and process mining over graph records. | 9,511 | 9,611 | repository-index-and-ingestion 1 |
| `medulla-compute` | L4 | Compute expression language: parser, AST, data science operators and sketches. | 13,560 | 14,460 | source-changesets 4, decision-engine 2, ingest-worker 2, branch-merge 1 |
| `medulla-finance` | L4 | Finance kernels, backtests, risk, attribution, banking and execution signals. | 10,995 | 14,495 | finance-primitives 35 |
| `medulla-decision-policy` | L4 | Ladder stages 3-5: policy, hard constraints, knobs, admission, refusals, budgets and the bounded exact solve. | 2,428 | 16,828 | decision-engine 143, placement-and-worker-policy 1 |
| `medulla-decision-rank` | L4 | Ladder stage 6: ranking the remaining legal options with fitted statistics (scores, champions, bandits, credit). | 8 | 6,408 | decision-engine 64 |
| `medulla-uql` | L5 | UQL language: parser, binder and lowering to the executor. | 8,142 | 8,842 | federated-query-and-reasoning 7 |
| `medulla-nlq` | L5 | Natural-language questions to UQL, with regressions. | 4,532 | 5,332 | federated-query-and-reasoning 8 |
| `medulla-federation` | L5 | Federated planning across sources: federation optimizer, SSRF guard and remote engines. | 11,046 | 15,766 | connector-competency 19, federated-query-and-reasoning 15, unified-data-plane 14, cross-source-competency 7, durable-graph-kernel 2, orchestration-competency 1 |
| `medulla-exec` | L5 | Physical plan execution: optimizer, cost models, runtime, DAG, incremental and streamed results. | 12,725 | 14,325 | finance-primitives 10, federated-query-and-reasoning 4, unified-data-plane 2 |
| `medulla-retrieval` | L5 | Retrieval and expansion: knowledge assembly, LeanRAG, verdicts and stop rules. | 4,730 | 19,130 | federated-query-and-reasoning 144 |
| `medulla-plan` | L5 | Cross-modal plan tests and the planner entry that composes UQL, federation, exec and retrieval. | 9,849 | 13,549 | federated-query-and-reasoning 34, unified-data-plane 3 |
| `medulla-decision-plan` | L5 | Ladder stages 6-7 and task planning: AgentAssemble, bounded task graphs, delegation, harness and skill selection. | 5,143 | 11,043 | decision-engine 57, typed-packs-and-catalog 2 |
| `medulla-packs` | L5 | Typed pack import, validation and roll-up to core classes. | 726 | 15,026 | typed-packs-and-catalog 142, federated-query-and-reasoning 1 |
| `medulla-catalog` | L5 | The catalog over packs, sources and schemas: lookup, lineage and discovery. | 8 | 908 | typed-packs-and-catalog 8, unified-data-plane 1 |
| `medulla-vocab` | L5 | Ontology modules and shapes for the data-platform, media, supply-chain and orchestration vocabularies. | 8 | 3,908 | orchestration-vocabulary 36, media-and-personal-vocabulary 24, supply-chain-vocabulary 5 |
| `medulla-ingest` | L5 | Ingest worker loop, lanes and queues. | 8 | 5,808 | ingest-worker 58 |
| `medulla-decision-records` | L6 | Decision records: receipts, ledger, the revision lifecycle (R805), provenance, replay and feedback. | 8 | 9,308 | decision-engine 93 |
| `medulla-decision-eval` | L6 | Decision evaluation: served proof suites, quality and relevance scoring, drift and nightly evaluation. | 8 | 10,308 | decision-engine 103 |
| `medulla-decision-memory` | L6 | Decision context and memory: session memory, retrieval for decisions, temporal and indexless recall. | 8 | 5,208 | decision-engine 52 |
| `medulla-competency` | L6 | Connector competency questions answered through the served query path. | 8 | 5,948 | connector-competency 99 |
| `medulla-competency-xsrc` | L6 | Cross-source and orchestration competency questions. | 8 | 3,548 | orchestration-competency 30, cross-source-competency 24, connector-competency 5 |
| `medulla-repo-index` | L6 | Repository indexing, source changesets and branch merge as graph records. | 8 | 5,708 | repository-index-and-ingestion 44, source-changesets 9, branch-merge 4 |
| `medulla-sdlc` | L6 | Graph-native development lifecycle: spec rows, gates, verdicts and releases as records. | 8 | 5,508 | development-lifecycle 55 |
| `medulla-parity` | L6 | Engine compatibility features and their parity proofs against other engines' behaviour. | 8 | 3,408 | engine-parity 34 |
| `medulla-placement` | L6 | Worker admission and placement policy. | 8 | 2,308 | placement-and-worker-policy 23 |
| `medulla-kernel` | F0 | Persist, store authority, mutation apply and channels. | 8,815 | 24,915 | durable-graph-kernel 160, engine-identity-and-scope 1 |
| `medulla-store` | F1 | redb store core, layout, sidecar and shards. | 22,273 | 22,673 | durable-graph-kernel 2, engine-identity-and-scope 1, ingest-worker 1 |
| `medulla-work-store` | F2 | Durable work items and their capability records. | 13,962 | 14,762 | durable-graph-kernel 7, unified-data-plane 1 |
| `medulla-resource-store` | F2 | Resource reservations, capacity leases and enrichment budgets. | 11,113 | 12,313 | durable-graph-kernel 12 |
| `medulla-lane-store` | F2 | Development lane records in the store. | 10,340 | 10,340 |  |
| `medulla-server-api` | F3 | Server API surface shared by handlers: api types, blob, kv, CDC and CEP. | 22,376 | 22,776 | engine-identity-and-scope 1, federated-query-and-reasoning 1, typed-packs-and-catalog 1, unified-data-plane 1 |
| `medulla-server-auth` | F4 | Request authentication, access checks and the SQL catalog ACL. | 12,565 | 13,065 | decision-engine 2, durable-graph-kernel 1, engine-identity-and-scope 1, finance-primitives 1 |
| `medulla-server-mutation` | F4 | Served mutation, mutation batches and transactions. | 16,550 | 17,050 | engine-identity-and-scope 2, finance-primitives 2, decision-engine 1 |
| `medulla-server-blob` | F4 | Served blob and key-value storage. | 8,020 | 8,420 | durable-graph-kernel 2, source-changesets 1, typed-packs-and-catalog 1 |
| `medulla-persistence` | F5 | Persistence backend: redb backend, durable stores, node info and cost accounting. | 17,889 | 18,489 | decision-engine 2, durable-graph-kernel 2, engine-parity 2 |
| `medulla-persistence-records` | F6 | Persisted agent, decision, connector pack and write-back records. | 15,767 | 16,867 | typed-packs-and-catalog 6, decision-engine 5 |
| `medulla-persistence-ops` | F6 | Backup, cold offload, online reshard, shard migration and store upgrade. | 10,259 | 10,459 | durable-graph-kernel 1, typed-packs-and-catalog 1 |
| `medulla-raft` | F7 | Raft consensus: command log, network, store, multi-raft and placement. | 19,132 | 19,532 | durable-graph-kernel 3, typed-packs-and-catalog 1 |
| `medulla-raft-xshard` | F8 | Cross-shard transactions and reads, elasticity and the raft test harnesses. | 12,589 | 12,689 | durable-graph-kernel 1 |
| `medulla-wire-core` | F8 | Shared wire listener framework. | 7,420 | 8,320 | durable-graph-kernel 9 |
| `medulla-wire` | F9 | SQL and graph wire listeners: pgwire, MySQL, MSSQL, SQLite and Bolt. | 11,072 | 11,572 | engine-parity 3, durable-graph-kernel 1, unified-data-plane 1 |
| `medulla-wire-msg` | F9 | Messaging and object listeners: AMQP, MQTT, STOMP, Redis, DDS, S3 and the KV cache endpoint. | 13,259 | 13,459 | durable-graph-kernel 2 |
| `medulla-h-query` | F9 | UnifiedQuery and knowledge stream handlers. | 15,256 | 22,696 | connector-competency 42, federated-query-and-reasoning 38, engine-parity 4, unified-data-plane 3, engine-identity-and-scope 2, cross-source-competency 1, finance-primitives 1, media-and-personal-vocabulary 1 |
| `medulla-h-sparql` | F9 | SPARQL/RDF and federation handlers. | 5,543 | 6,163 | federated-query-and-reasoning 5, connector-competency 2 |
| `medulla-h-retrieve` | F9 | Discover, semantic index and reasoning projection handlers. | 9,155 | 9,515 | federated-query-and-reasoning 2, media-and-personal-vocabulary 1, unified-data-plane 1 |
| `medulla-h-mining` | F9 | Mining handlers. | 8,234 | 8,514 | supply-chain-vocabulary 2, orchestration-competency 1, repository-index-and-ingestion 1 |
| `medulla-h-admin` | F9 | Administrative handlers. | 15,012 | 16,972 | typed-packs-and-catalog 17, connector-competency 1, durable-graph-kernel 1, federated-query-and-reasoning 1 |
| `medulla-h-packs` | F9 | ConnectorPack.Import, schema sources and pack projection handlers. | 4,308 | 6,488 | typed-packs-and-catalog 15, connector-competency 2, decision-engine 2, federated-query-and-reasoning 1, finance-primitives 1, orchestration-vocabulary 1, source-authority 1 |
| `medulla-h-decide` | F9 | Decide, AgentAssemble, DecisionCommit and DecisionLog handlers (thin: logic lives in the decision family). | 15,371 | 18,891 | decision-engine 21, orchestration-vocabulary 6, placement-and-worker-policy 4, typed-packs-and-catalog 3, connector-competency 1, development-lifecycle 1, engine-identity-and-scope 1, finance-primitives 1 |
| `medulla-h-compute` | F9 | Compute, finance, quantum and visualisation handlers. | 4,863 | 6,663 | finance-primitives 16, federated-query-and-reasoning 2 |
| `medulla-h-ingest` | F9 | SourceIngest, modality and repository enrichment handlers. | 8,645 | 9,725 | ingest-worker 4, media-and-personal-vocabulary 3, decision-engine 2, unified-data-plane 2, repository-index-and-ingestion 1 |
| `medulla-h-graph` | F9 | Graph operation and pipeline handlers. | 10,076 | 11,756 | orchestration-vocabulary 8, federated-query-and-reasoning 6, durable-graph-kernel 2, decision-engine 1, development-lifecycle 1, source-authority 1, typed-packs-and-catalog 1 |
| `medulla-h-jobs` | F9 | Job, delegation and work item handlers. | 9,677 | 11,277 | durable-graph-kernel 4, placement-and-worker-policy 4, typed-packs-and-catalog 3, unified-data-plane 3, repository-index-and-ingestion 2 |
| `medulla-h-txn` | F9 | Transaction, SQLite file, timeseries and distributed compute handlers. | 10,644 | 10,844 | finance-primitives 2 |
| `medulla-server-graph` | F10 | Graph pipeline dispatch, SPARQL update dispatch and change envelopes. | 10,780 | 14,620 | ingest-worker 12, federated-query-and-reasoning 5, repository-index-and-ingestion 4, unified-data-plane 4, orchestration-vocabulary 3, branch-merge 2, decision-engine 2, durable-graph-kernel 2, typed-packs-and-catalog 2, engine-identity-and-scope 1, finance-primitives 1, media-and-personal-vocabulary 1, source-changesets 1 |
| `medulla-server-consensus` | F10 | Consensus dispatch. | 4,664 | 4,864 | ingest-worker 1, typed-packs-and-catalog 1 |
| `medulla-server-obs` | F10 | Observability, traces, telemetry and QoS. | 8,786 | 8,786 |  |
| `medulla-server-lake` | F10 | Lake serving, GraphQL subscriptions and policy export. | 7,696 | 8,396 | federated-query-and-reasoning 5, unified-data-plane 2 |
| `medulla-server-identity` | F10 | Identity store dispatch, request boundary and policy evolution dispatch. | 6,120 | 6,420 | decision-engine 1, durable-graph-kernel 1, engine-identity-and-scope 1 |
| `medulla-server-router` | F10 | Method router dispatch table. | 4,673 | 6,373 | decision-engine 5, engine-identity-and-scope 4, ingest-worker 4, development-lifecycle 1, durable-graph-kernel 1, engine-parity 1, federated-query-and-reasoning 1 |
| `medulla-server` | F11 | Server assembly: router, startup, transport and the remaining dispatch. | 25,434 | 26,094 | decision-engine 2, durable-graph-kernel 2, orchestration-vocabulary 1, repository-index-and-ingestion 1, unified-data-plane 1 |
| `medulla-engine` | F12 | The one server executable and the embedded engine. | 7,771 | 10,971 | public-engine-contract-and-release 32 |

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
| `medulla-server-api` | `medulla-server-api` | F3 | Server state with type-erased feature handles, the seam registry and re-entry dispatch seam, request context, the batch compiler, schema composition, transactions and the shared served-API helpers. | - | 8,925 | 14,557 |
| `medulla-server-api` | `medulla-server-auth` | F4a | Request authentication, access checks, the SQL catalog ACL and SQL tables. | `medulla-server-api` | 13,107 | 13,607 |
| `medulla-server-api` | `medulla-server-cdc` | F4b | Change data capture hub, CEP, materialised views, reasoning cascade and impact watch. | `medulla-server-api`, `medulla-server-auth` | 4,491 | 4,491 |
| `medulla-server-api` | `medulla-server-mutation` | F4c | Served mutation and mutation-batch commit. | `medulla-server-api`, `medulla-server-auth`, `medulla-server-cdc` | 12,245 | 9,384 |
| `medulla-server-api` | `medulla-server-blob` | F4d | Served blob, key-value and SPARQL HTTP endpoints. | `medulla-server-api`, `medulla-server-auth`, `medulla-server-mutation` | 9,993 | 10,393 |
| `medulla-server-api` | `medulla-server-ops` | F4e | Served API handler seams: the admin, decide, ingest, graph, jobs, compute, query, transaction and wire seam traits, their typed accessors and their support modules. | `medulla-server-api`, `medulla-server-auth`, `medulla-server-blob`, `medulla-server-mutation` | 7,804 | 5,533 |
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
