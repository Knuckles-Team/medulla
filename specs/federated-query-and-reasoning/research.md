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

## Context bundle compile decisions (R650.1, R650.2, R652.2, DECISION-ENGINE R137.8)

- One selection kernel: `select` in `medulla-decision-policy` is the only knapsack; `compile` and the package decision family both call it. A solver result that is not a verified optimum is refused, so a greedy fallback never qualifies.
- A candidate is one resolution of one hit. Its value is the fused value times its token share of the widest resolution, so a smaller resolution is a real trade-off rather than a free choice.
- Fusion is `medulla_text::rrf_fuse`, the function the plan executor already uses; `medulla-text` is always linked, its Tantivy index stays behind the `text` feature.
- `certificate` is absent only for an empty candidate set; every other bundle carries the verified selection.
- Calibration consumes a pinned curve when the source has one and falls back to min-max scaling with a `no_calibration` reason; fitting curves is `MEDULLA-DECISION-ENGINE-R822`.

## Served context compile decisions (R650.3 to R650.6)

- The wire records move below the protocol crate (`medulla-decision-types`, beside the package records) so `Method::ContextCompile` can carry the request and answer the bundle; `medulla-plan` re-exports them and keeps only the compile. One definition, no mirror.
- The engine measures tokens itself: bytes divided by the registered knob `context_facts.bytes_per_token`, rounded up and at least one, the same estimator `ContextFacts` uses. The request's `tokenizer_id` and `tokenizer_version` name the tokenizer the caller's model uses; they are carried into every candidate and the bundle key, and an empty one refuses `CONTEXT_BUDGET_MISSING` together with a zero window.
- Resolutions are real text spans of a node: the name, the name with the first 240 bytes of description, and the name with the whole description. Repeated spans are not offered twice. The citation is the covered byte range of the node text at the graph revision read, with the SHA-256 of that text.
- Arms are the lexical keyword overlap and the semantic store search over the caller's row-projected core, bounded by the lexical scan cap of `Discover`; `hybrid` runs both, a missing embedding degrades `hybrid` to lexical, and `semantic` without an embedding is `INVALID_ARGUMENT`.
- `CONTEXT_ITEM_COST_INVALID` is refused when a query-embedding component or an arm score is non-finite: the item's inclusion value would otherwise reach the solver as an invalid cost.
- `CONTEXT_SCOPE_FOREIGN` is refused when `agent_ref` names no node in the caller's tenant graph; the projected core holds no other tenant's rows, so a foreign agent is indistinguishable from an absent one and both refuse.
- `as_of` is accepted only at the served revision; the engine keeps no older read view, so any other value is `INVALID_ARGUMENT` rather than a silent read of the current state.
- A hit whose node declares `access_mode = checked_at_read` goes through `filter_hits` before any fusion. With no registered port the check is unavailable and the hit is dropped, listed in `evidence.unchecked_dropped` and excluded as `denied`. A bundle that needed any such check is never cached.
- The served answer is cached under the caller-scoped hash of the `BundleKey`, invalidated by graph version; the policy digest in the key is the caller-scoped hash, so two callers with different visibility never share a bundle.

## Served retrieval round decisions (R081.11, R081.3, R082.1, R082.2)

- The round's request and result records live in `medulla-result-types`, not `medulla-types`: the parts embed `CalibrationTable`, `SufficiencyVerdict` and the other decision contracts, which sit above `medulla-types`. One definition, no mirror.
- A part joins `RoundRequest` with the row that serves it. The sufficiency part ships with R081.3; the other parts join with their wiring rows. No part exists in the request before something runs it, so no part can be accepted and ignored.
- `run_round` takes the `PlanCtx` the handler binds, so the plan runs on the same row-level-security snapshot, tenant narrowing and legs as `UnifiedQuery`. The method writes and caches nothing.
- A caller's round limit may lower the knob value and never raise it, for sufficiency as for stop.
- `decide_stop` stays pure: the two progress checks arrive as a measured `RoundProgress` built by `round_progress`, and apply only when the context says the round supplied measured evidence. A confidence below the calibrated baseline reports margin 0 so the verdict stays in [0, 1].

## Corpus retrieval decisions (R653.2, R653.3, R653.8 to R653.10)

- The wire types sit in `medulla-types` (the protocol crate carries the request) and `medulla-plan` re-exports them, as the bundle records did. `expand_hops` moves to `medulla-graph-core`: `medulla-h-graph` and `medulla-h-query` are siblings, so a shared walk has to sit below both; there is still exactly one.
- The mode functions read the core only through the `CorpusView` trait; the handler fills it from the row-projected core, so no mode can reach an unreadable row. The request has no `hops` field, so the entity mode walks 2 hops (the spec maximum) and reports that depth.
- Lineage edges run answer to source (`DERIVED_FROM`, `GENERATED_BY`, `CITES` leave the answer); each reached node yields one shortest path, breadth first, ordered by node id, bounded by `retrieval.lineage_depth`.
- A transcript turn is a graph node of type `TranscriptTurn` with `session_id`, `ordinal`, `role` and `content`.
- Grep scans the string properties of each node (optionally only one `type`), line by line; `start` and `end` are byte offsets within the line.
- R703.5 and R722.5 are not in this change: the denied and timeout reads of R703.5 need the registered access port of R703.3, and R722.5 needs the foreign provenance columns; both stay queued.

## Selection and hierarchy parts of the served round (R081.7.3, R081.7.5, R081.9.3, R081.9.5, R081.9.7.3 to R081.9.7.5)

- Decision: the reranker is a server-bound port named by config key `retrieval.reranker_model`, read from the environment like the text embedder. `builtin:lexical-overlap` is the only built-in binding. Any other value names a model artifact and binds nothing until the cross-encoder provider of `MEDULLA-FEDERATED-QUERY-R652.7` registers it; a selection request then refuses `RETRIEVAL_SCORE_PAIR_REQUIRED` instead of scoring with a stand-in.
- Decision: the embedding score of a chunk is the score of its executed row, so selection needs a ranked plan. A row with no score or no `text` property refuses `RETRIEVAL_SCORE_PAIR_REQUIRED`. Only the first `retrieval.rerank_pool` rows are scored; the receipt premise lists the rest as `unscored`.
- Decision: activation evidence is a measured pair of outcomes, not a bare digest. The gate passes the benchmark digest to `check_activation` only when nDCG at 10 lost at most `retrieval.selection_quality_floor` and the mean selected count is at most `retrieval.selection_cost_ratio_max` of the baseline's. `check_activation` stays the only digest comparison; it runs before any reranker call, so a refused activation spends no inference.
- Decision: the hierarchy sources live in `medulla-plan` beside `run_round`, not in `medulla-h-query`, because `medulla-h-query` depends on `medulla-plan` and `run_round` must call them.
- Decision: the snapshot is narrowed to the caller's tenant before the round runs, so every visible community, entity and section belongs to the caller. A node outside the caller's scope is absent, so naming it refuses `RETRIEVAL_HIERARCHY_INVALID` as a dangling reference.
- Decision: a community descent scores a summary or entity by the cosine of the query vector and its stored embedding. A node with no stored embedding scores 0. Each score charges the node's word count as tokens and 1 micro-unit of cost, so the policy's token and cost limits are measured.
- Decision: the flat baseline of a descent premise counts every distinct entity the stored communities name, each visited once. `flat_visits` and `flat_candidates` are therefore equal; they differ only when a later baseline prunes.
- Decision: a section limit refusal returns no receipt. `retrieve_vector_free` stops before it reads past a limit and returns no partial path, so the refusal detail carries `incomplete: true`.
- The section tree is read from stored `SectionNode` graph nodes. The round never takes inline nodes: a request with non-empty `section_nodes` refuses `RETRIEVAL_CONTRACT_INVALID`.

## Viewport queries (R081.103, R725)

- One method pair serves both rows: `ViewportQuery` and `ResolveViewportSelection`. R725 named a second pair (`ViewportBins`, `GraphSelectionResolve`) over the same kernels and handler; two pairs would be two doors for one capability, so R725 now names the one pair and adds its independent-reference test.
- Sources are scoped time series. A `tsdb_series` answers line queries; a `point_table` is a series with two named numeric fields that answers scatter queries. The graph has no maintained x/y index for a scatter source, and the maintained spatial index holds geometry boxes, not table points, so a series with a bounded range read is the simplest source that is already tenant scoped, versioned and durable.
- The source revision is the series store scope version of the caller's tenant and graph. It moves on any append in that scope, so a pin can only be too strict, never too loose. A request that pins nothing reads the current revision and the result pins it.
- Bins are sparse: only non-empty cells and columns return. The scatter cap bounds the non-empty cell count, so an overflowing grid refuses `VIEWPORT_CAP_EXCEEDED` instead of truncating. The byte ceiling is checked on the encoded result; bins serialize as arrays so one stays under the 96-byte allowance.
- Cell and column formulas multiply before they divide (`(v - lo) * n / (hi - lo)`), which keeps integer-valued sources exact; line columns use integer arithmetic on nanosecond timestamps.
- `filter_digest` is part of the viewport identity and is not applied by the engine: a caller filters by choosing the source it reads.
- Block aggregates for the 1,000,000,000-point nightly are a separate row (R081.103.5): the range scan serves the 1,000,000-point proof, and the nightly needs a store change.
