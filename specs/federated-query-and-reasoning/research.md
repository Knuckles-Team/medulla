# MEDULLA-FEDERATED-QUERY — Research

Governing plan: [plan.md](plan.md). Each entry describes a capability that rows `MEDULLA-FEDERATED-QUERY-R058.6` to `MEDULLA-FEDERATED-QUERY-R058.9` adopt. The entries describe techniques, not products.

## 1. How an NL answer stays consistent with approved meaning

**Decision.** A governed term carries one certified definition and its lineage. The engine checks each candidate query against the certified definitions of the terms it uses before it runs the query. The result lists those terms, so the caller sees which approved meaning produced the answer (`MEDULLA-FEDERATED-QUERY-R058.6`).

**Technique.** Treat the check as one more typed feedback step in the existing bounded correction loop. A mismatch costs one planner step and never a separate service.

**Rejected.** A second metadata store for definitions. The definitions are graph facts on `:GlossaryTerm`, in the one engine.

## 2. How approved corrections improve later answers

**Decision.** A certifier records a correction as a fact: the question, the rejected query and the approved query (`MEDULLA-FEDERATED-QUERY-R058.7`). Planning context includes matching approved pairs as examples.

**Technique.** Corrections ground planning; they never edit a certified definition. A definition changes only by its own certification.

**Rejected.** Learning from every answer the caller did not reject. Silence is not approval.

## 3. How a multi-turn question becomes plannable

**Decision.** A follow-up is rewritten into one standalone question before planning, and the result discloses the rewrite (`MEDULLA-FEDERATED-QUERY-R058.8`).

**Technique.** One extra planner call, bounded by a turn count. Without history the step is skipped.

## 4. How two sources that disagree on a term are handled

**Decision.** The engine records a conflict fact and treats the term as uncertified until a certifier resolves it (`MEDULLA-FEDERATED-QUERY-R058.9`).

**Rejected.** Automatic merging of definitions. A silent merge produces answers whose meaning nobody approved.

## Evidence forests for context selection

Rows `MEDULLA-FEDERATED-QUERY-R736` to `MEDULLA-FEDERATED-QUERY-R736.5` adopt this technique.

**Decision.** The engine builds a candidate graph from the seed memories. It follows schema relations and semantic relations for a small number of hops. It scores each candidate twice. The first score is direct support: how well the candidate answers the query. The second score is verification cost: how costly it is to check the relation that links the candidate to its parent. The engine keeps the two scores apart so a cheap, well supported candidate wins over a costly one.

**Technique.** Selection is greedy by gain per token. Gain is support minus verification cost. The engine lowers the gain of a candidate that repeats chosen evidence. It zeroes the gain of a candidate that conflicts with chosen evidence. Each chosen candidate hangs from one chosen parent. The result is a forest, and each tree shows how its evidence connects to a seed. Selection stops at the token budget.

**Why.** A flat top-k list repeats near-duplicate facts. It also hides how a fact connects to the query. A forest is compact and explains itself. The sufficiency judge of `MEDULLA-FEDERATED-QUERY-R081` reads the same rows, so no second consumer is needed.

**Rejected.**
- A flat top-k list with a larger k. It spends the budget on duplicates.
- A model call to verify every relation. It adds latency and cost per candidate.
- A second retrieval store for forests. The candidates are graph facts in the one engine.

**Values.** Two hops, a 2000-candidate cap and the penalty weights are starting defaults. The nightly benchmark group `retrieval/evidence_forest` replaces them with measured values. The target is at least 20 percent fewer tokens than top-k at equal recall, p99 150 ms and 128 MiB.

## Decision: One result shape for GROUP and projections

**Decision.** RowSet gains a typed tabular channel (named variables plus typed values per row) used by GROUP and every projection (`MEDULLA-FEDERATED-QUERY-R737`). Id and score stay as ordinary columns. A parallel group-result type would give every consumer two shapes to read.

## Decisions: stored-query serving (MEDULLA-FEDERATED-QUERY-R708.3, R708.4, R708.8)

- **Template.** `StoredQueryPut` and `StoredQueryCall` follow the live-artifact methods end to end: handler family in `crates/medulla-h-query/src/query/stored_query.rs` routed by an `is_stored_query_method` check in `dispatch_query_family`; node writes through the staged query gateway (`is_query_gateway_method`, `GATEWAY_ROUTED`, `query_method_name`); write classification; capability descriptors with the `query:write` and `query:read` scopes; result contract entries; `ServerErrorCode` variants and the `STORED_QUERY_` method prefix; audit control line; capability snapshot lists; wire vector fixture; served Python test.
- **Where the model lives.** The wire request carries `StoredQuery`, and `medulla-protocol-types` cannot depend on `medulla-exec`. The model moves to `crates/medulla-source-types/src/stored_query.rs` beside `LiveArtifact`, which already depends on `medulla-identity-types` for the sharing enum. `medulla-exec` drops its copy; the R708.1 test binding moves with it.
- **Sharing enum.** The sharing enum is `ShareScope` in `crates/medulla-identity-types/src/identity/segment.rs`; the row text "SharingScope" names it. Visibility reuses `owner_or_matching_scope` with the author's tenant and placement stored on the node, so a stored query follows the same rule as every other unit. When MEDULLA-IDENTITY-R610 renames the values to `private`, `org` and `commons`, the codemod covers this field.
- **Languages.** The call executes UQL through `handle_uql` with typed `UqlParam` values bound by name; text never splices. A put in SQL or SPARQL refuses with `STORED_QUERY_INVALID` until row MEDULLA-FEDERATED-QUERY-R708.10 makes them callable.
- **Materialized reads.** `MEDULLA-FEDERATED-QUERY-R708.6` benches a materialized result, so it follows `MEDULLA-FEDERATED-QUERY-R708.2` (materialization and refresh) and `MEDULLA-FEDERATED-QUERY-R708.5` (staleness evidence).
- **A call can write.** `StoredQueryCall` rewrites the materialized result when it recomputes, so it is a gateway-routed, write-classified method with scope `query:write`, like `LiveArtifactRefresh`; it is not a read method. The record is one node per (version, argument binding): `stored_query_result:<name>:<version>:<args digest>`.
- **Reuse and recompute.** `serve_materialized` (`crates/medulla-source-types/src/stored_query.rs`) is the one decision. The input generation is the snapshot revision the UQL result reports. `OnChange` runs the query on every call through `handle_uql`; the dependency-keyed result cache (MEDULLA-FEDERATED-QUERY-R017) answers without work while no input changed and reports the old revision, and the record is rewritten only when the revision differs. A graph-wide revision would rewrite the result on every call, because the handler's own result write advances it. `Period` returns the stored record untouched until the period has elapsed. The engine has no maintenance tick, so due-ness is decided on the call; a scheduled job that calls the same function can be added without changing the model.
- **Versions.** Every put publishes one immutable version (`stored_query:<name>:<version>`); a head node (`stored_query_head:<name>`) holds the first author and the highest version. Only the first author publishes later versions of a name (`ACCESS_DENIED` otherwise). A call without a version reads the head. An unknown name or version refuses `STORED_QUERY_NOT_FOUND`.
- **UQL form.** `CALL STORED name@version(arg = literal, ...)` is a statement of its own, not a pipeline stage: `medulla_uql::parse_call_stored` recognises it before `parse_statement`, and `handle_uql` hands it to the same `handle_stored_call` as the method. Arguments are string, number or boolean literals, never text spliced into a query.
- **Retrieval chain.** `MEDULLA-FEDERATED-QUERY-R653.8` needs `expand_hops` below `medulla-plan`: it moves from `crates/medulla-h-graph/src/graph_ops/discover_plan.rs` to `crates/medulla-graph-core/src/hop_expand.rs` (both crates depend on graph-core), and its R090.8 unit test moves with it. Its L4 test needs the served `CorpusRetrieve` method of R653.3. `MEDULLA-FEDERATED-QUERY-R650.6` and `MEDULLA-FEDERATED-QUERY-R703.5` need the served `ContextCompile` method and `context_compile.rs`.
