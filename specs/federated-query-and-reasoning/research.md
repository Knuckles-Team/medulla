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

**Built (R737, R733).** The value channels and the text columns stopped being two side tables:
both are typed views of one cell table in `RowSet`, so `GROUP` (keys as text, `COUNT` as int, the
other aggregates as float) and `RETURN` read and write the same thing. `RETURN` keeps text
columns because they are the row's source provenance, which the encoder returns beside the
channels. `label` as a GROUP key reads the node label from the same fields `Scan` reads. A
served answer with `GROUP` and no `RETURN` returns its aggregate columns, so a graph view needs
no projection to see its counts. The `uql.group.max_groups` knob reaches the executor through
`Budget::max_groups`, set by the served planner, so the executor reads no registry itself.

## Decisions: stored-query serving (MEDULLA-FEDERATED-QUERY-R708.3, R708.4, R708.8)

- **Template.** `StoredQueryPut` and `StoredQueryCall` follow the live-artifact methods end to end: handler family in `crates/medulla-h-query/src/query/stored_query.rs` routed by an `is_stored_query_method` check in `dispatch_query_family`; node writes through the staged query gateway (`is_query_gateway_method`, `GATEWAY_ROUTED`, `query_method_name`); write classification; capability descriptors with the `query:write` and `query:read` scopes; result contract entries; `ServerErrorCode` variants and the `STORED_QUERY_` method prefix; audit control line; capability snapshot lists; wire vector fixture; served Python test.
- **Where the model lives.** The wire request carries `StoredQuery`, and `medulla-protocol-types` cannot depend on `medulla-exec`. The model moves to `crates/medulla-source-types/src/stored_query.rs` beside `LiveArtifact`, which already depends on `medulla-identity-types` for the sharing enum. `medulla-exec` drops its copy; the R708.1 test binding moves with it.
- **Sharing enum.** The sharing enum is `ShareScope` in `crates/medulla-identity-types/src/identity/segment.rs`; the row text "SharingScope" names it. Visibility reuses `owner_or_matching_scope` with the author's tenant and placement stored on the node, so a stored query follows the same rule as every other unit. When MEDULLA-IDENTITY-R610 renames the values to `private`, `org` and `commons`, the codemod covers this field.
- **Languages.** The call executes UQL through `handle_uql` with typed `UqlParam` values bound by name; text never splices. SQL and SPARQL stored queries (row MEDULLA-FEDERATED-QUERY-R708.10) run through the served SQL and SPARQL handlers; neither language has native parameters, so each `$name` outside quoted text and comments is replaced by one typed literal (a string quoted and escaped, a number or boolean as its canonical token), which keeps the value one literal and never query text. A put refuses text that does not parse, that names an undeclared `$name`, or that is a SQL write. Rows come back as one object per row keyed by column, as UQL rows do.
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
- Decision (R081.103.5): the aggregates live in the chunk blob as a trailer, not in a new table. A second table would need its own owner-layout declaration and a second write to keep in step; the trailer is written by the one `Chunk::encode`, so the aggregate and its points cannot disagree and commit together. The cost is a format change of stored chunks, which follows the hard-cutover rule (no reader for the old layout).
- Decision (R081.103.5): a chunk folds only when it lies wholly inside one pixel column, because folding a chunk that spans columns would move samples between them and break equality with the point scan. Non-finite samples are excluded from every aggregate, as `bin_line` skips them. Scatter is not folded: a cell needs both coordinates per point, so the nightly at 1,000,000,000 points is a line series, and scatter sources stay within the range-scan limit.

## Model scorer of the rerank stage (R652.7)

- Decision: rerank runs in one place, the `ContextCompile` pipeline, in the order fuse, `rerank_top`, calibrate, boost. `Discover` is a cheap navigation read and never reranks.
- Decision: `ModelScorer` adapts the `Reranker` the server already binds by `retrieval.reranker_model`. There is one binding and one provider seam; a second model loader would be a second path. Its scorer id version is the bound artifact revision, so a score from another artifact is never mistaken for it.
- Decision: a model that is configured but absent, or that returns no usable score for every candidate, falls back to the lexical scorer with `fallback = scorer_unavailable`. A server with the key unset runs the lexical scorer as its scorer and reports no fallback, because nothing was lost.
- Decision: a calibration curve pinned for another scorer is not applied; the scores scale min-max and the receipt carries `no_calibration`. Scores from different scorers are never mixed on one curve.
- Decision: the receipt is carried in the bundle evidence as `RerankEvidence` beside the other inline evidence; nothing is stored. Candidates past `retrieval.rerank_k` are not scored and leave the bundle as `below_gap`.
- The champion-pointer read of the calibration curve (`MEDULLA-FEDERATED-QUERY-R652.3`) waits on the pointer read of `MEDULLA-DECISION-ENGINE-R805.13`; until then the stage runs with no pinned curve.


## Stop part wiring and corpus refusals (R082.3, R082.4, R653.4, R653.5)

- Decision: the stop block is the last step of `run_round`. The completed round is the last answer in the history, else the sufficiency part's round, else 1; progress counts as measured only when the request carries prior row ids or a prior quality, so a first round never stops for "no new evidence" by vacuity.
- Decision: a terminal verdict sets the sufficiency repair step to `Stop`. That is the whole of "no continuation": the harness has no next-round description left to apply.
- Decision: an exhausted limit answers with `RETRIEVAL_BUDGET_EXCEEDED` and the serialized terminal verdict as its detail, so the dimension and the calibration marker travel with the refusal. The other terminal reasons are normal results with `stop: true`.
- Decision: an entity or session the projected core does not hold answers `CORPUS_SCOPE_DENIED`. The projected core cannot tell a row the caller may not read from a row that does not exist, and the refusal must not either.
- Decision: a lineage walk with an unseen source past `knob retrieval.lineage_depth` refuses `CORPUS_LINEAGE_TOO_DEEP` instead of returning a silently shortened set.
- Decision: corpus evidence is built in `dispatch` from the mode's own counts; the handler only names the row-level projection (`caller-row-projection`), because the handler owns the projected core.
## Chunk-encoding steps are pure functions with registry knobs (R039.2.2 to R039.2.4)
Decision: window splitting, per-window encoder calls and `mean_chunks` aggregation are pure functions in
`crates/medulla-core/src/compute/chunk_windows.rs`. Window size and overlap are registry knobs
(`retrieval.encode_window_tokens`, `retrieval.encode_window_overlap`), never environment variables or
scheduler fields; the semantic index worker reads them and passes them in. Chunk refusal codes are
`ServerErrorCode` variants so they join the one published refusal registry, routed to `SemanticIndex`.
Dimension mismatch and non-finite components are refused with `ENCODE_DIM_MISMATCH` and `ENCODE_NON_FINITE`
inside the aggregation, so a malformed chunk can never silently reach a document vector.


## R081.100 joint sparse selection

The kernel is plain cyclic coordinate descent on the nonnegative lasso, validated against an exhaustive-support KKT oracle. A caller-supplied query vector is allowed because `Op::Rank` already accepts one; the knobs bound the solver, so no new score authority is added. The mode lives on `SelectionPart` because the dual-score policy fields do not apply to it.
Chunk-encoding wiring decision (R039.2.5 to R039.2.7). The recipe has one executor: `encode` in the query crate, behind the served `EncodeDocument` method. A document vector and its receipt are graph nodes written through the staged graph of the query gateway, the same way a stored query version is, so the document, its generation head and the receipt commit together or not at all and every refusal returns before any write, which keeps the prior generation. The head is keyed by tenant and policy digest, so vectors encoded under two policies never share a ranking, and a search names its policy digest: a digest with no generation under it is refused `ENCODE_POLICY_STALE` before ranking. The generation budget is the registry knob `retrieval.encode_budget_chunks`, not a scheduler field with an environment key. The bound CI encoder is a tiny deterministic hashed bag of token ids with two poison token ids so a served test can fail one chunk.

## Low-storage ANN artifact layering (R040.5.1, R040.5.2, R040.5.5)

Decision: the compact artifact is one container in `medulla-ann` (version byte 2) wrapping the existing vectorless compact HNSW image, per-row SQ8 codes (the IVF-PQ SQ8 encoder, reused) and tombstoned stable ids, with the manifest persisted beside it. Hub pruning reuses the existing bounded `PrunePolicy` transform; the manifest records the policy. The byte ceiling and the pruning switch are `SchedulerConfig` keys (`ann_storage_ceiling_bytes`, `ann_hub_pruning`). No second store, no vector copy of source text.

- R040.5.4 decision: the embedding-provider budgets are `embed.*` registry knobs, not `SchedulerConfig` fields, so one registry owns every tunable; the provider takes an `EmbedLimits` value built from them. A run is one provider wrapper and its call counter; a refused wave has no provider effect.

## R722.5 answer provenance surface (decision)

The pack-source read already builds `connector`, `contract_iri`, `mapping_iri`, `fetch_time_ms` and `provenance` cells per row in `PageBackedSource::fetch_columns`, but the served `UqlResult::Rows` row (`UqlRow`) carries only id and score channels, so no answer row can show them. Decision: surface a source row's text channels (the `texts` of `RowSet`) as a named `provenance` map on `UqlRow`, defaulted empty so existing answers are unchanged; the web and search sources already fill the same channel. The usage counter is the existing `asset_usage` series (R764.3.1) with `ForeignScan` source names added to `query_assets`; freshness age is read at query time from the source watermark (R106.5), never copied onto the node.

## R040.5.1 proof boundary (ROOT-SLICE)

- R040.5.1 is proven by its own L1 test `crates/medulla-ann/src/low_storage.rs::tests::manifest_round_trip_and_sum`. The served `test_selectivity_bound_and_readback` is the proof of the rollup R040.5 and of R040.5.6; the traceability table no longer binds it to R040.5.1.
## R081.101 compressed document state

Decisions. The state key is one length-prefixed string, so the owner table keeps the single `&str` key shape every other owner table uses and no tuple-key support is added. The encoder is a port with no decode or generate method, so "no decoder call" holds by construction and the result reports `decoder_calls: 0`. The engine ships one deterministic encoder (signed hashed bag of tokens, 32 wide, key and value vectors); it is bound by a bool registry knob because knobs are never strings, and a model-backed encoder is a later implementation of the same port. The build job runs at the document commit that already reads the source text (`complete_sql_source_stage`), once per content digest. Query time reads only: a missing state refuses `RETRIEVAL_STATE_STALE` instead of building. The state part rides on the existing `RetrievalRound` method as an optional part, so no new public operation is added.

## R703.3 access check path (decision)
The engine never calls a connector itself, so the read-time access check rides the one
connector port operation-bound sources already use: `ConnectorRowSource` gains `access_check`
(default: cannot answer, which fails closed), and the gateway carries it as op `access_check`.
The connector server comes from the caller's registered `Mcp` source named by the hit's
`source`, resolved through the owner-scoped registry. No second driver path and no new cache.

## R700.5 digest helper (decision)
`medulla-kernel` has no SHA-256 helper; `ArgumentMap::digest` in `medulla-result-types`
hashes the canonical interchange JSON with `sha2`, the crate that already owns the map type.
## Expansion port: kinds and the single HTTP path (R651.3, R651.7)
Decision: the expansion port reuses the one chat-completion path of the NL planner client
(`UreqNlPlanner::complete`), so endpoint, credentials, headers, TLS, timeouts and the
response cap are configured once in the `nl_query` section. `expansion_model` and
`expansion_read_timeout_secs` override only the model and the read timeout. The model
answers a JSON list of strings, so the kind is assigned by the request: a request with term
feedback yields `perspective` items; a plain request yields one `hypothetical` passage first
and `sub_query` items after it. Why: one list shape, no per-item kind field to trust.
## R081.8.1 feedback part decision

`RoundFeedback` lives beside `RoundRequest` in `medulla-result-types`, because it embeds `RetrievalFeedback` (which `medulla-types` cannot see). The two limits with no earlier knob get registry knobs `retrieval.feedback_queries` (default 8) and `retrieval.feedback_terms` (default 32). A limit above its knob is refused `RETRIEVAL_BUDGET_EXCEEDED`, never silently lowered; the part is validated before any row is read.

## R651.4 refusal semantics: required versus best-effort expansions

`Discover` gains `perspectives`, the count of expansions a caller requires. Absent, the plan is best effort: `knob discover.expansions`, a failing provider degrades the plan (R090.4) and a missing provider is `DISCOVER_PLANNER_UNCONFIGURED`. Present, the caller depends on the expansions, so a missing provider is `EXPANSION_PROVIDER_UNCONFIGURED`, a failing provider is `EXPANSION_PROVIDER_FAILED` with its error text, and a count above 8 is `EXPANSION_FANOUT_EXCEEDED`, all before any retrieval. Why: the knob is bounded to 1 to 8, so without a request field the fan-out bound and the port-level codes could never be reached from a served call, and a silent degrade is wrong for a caller that asked for a number. `NlQuery` keeps `NL_PLANNER_UNCONFIGURED` (R058), because that code already names the missing endpoint; it gains the shared 2,048-byte text bound.


## R702.2 threshold comparison per detector

The exposed-constant and prior-knowledge detectors say "at least" in their row text, so a value equal to the threshold breaches. `RiskThresholds::breached` is the one comparison, shared by the refinement loop (`over`) and `RiskReport::check`; single-clue and co-coverage stay "beyond the threshold". The boundary tests sit at exactly the threshold for both detectors.
## R722.8 key pushdown of a bound pack source

Decision: a bound callback's key lookup is offered only when its registration declares `key_filter_pushdown`; the lookup rides a new `ForeignSource::fetch_keys` whose default is the full read, and the optimizer's join always intersects the answer with the local selection (the exact residual). Why: one capability source (the registration), a source that over-claims or ignores keys returns a superset and is still correct. The key batch size is the federation budget's `max_bind_keys`, not a second limit.
## R709.6 the GeoSPARQL hot-path bench is served

The bench lives in `crates/medulla-server/benches/geosparql.rs`, not in `medulla-rdf`, because the row names `Method::Sparql` and only `medulla-server` can stand up the signed request chain (FQ-R707.6). It uses its own p99 harness rather than criterion, so it needs no new dependency: it loads a 1,000 by 1,000 grid of `geo:wktLiteral` points through served SPARQL updates of 10,000 triples, asserts the polygon returns exactly its 100 by 100 cells, and compares the p99 with `latency_p99_ns_max.geosparql_within` (40 ms) and with the stored `latency_p99_ns_baseline.geosparql_within` plus 10 percent when one is recorded.
## R730 falsification gate: measurement, judgement and the freeze decision

The gate has three parts. The bench `benches/unified_vs_contracts.rs` is the measurement: it stands up the single engine of arm A and the two engines of arm B through the served dispatch (the same kit as `benches/decide_budgets.rs`), runs `planner.gate.writers` concurrent writers during the measured queries, and writes a report. Recall is measured against an exact brute-force top-k of the neighbourhood, so equal recall is checked against the same truth for both arms. A write counts as lost when its response was an error or when it is not readable through a served read afterwards.

`scripts/unified_planner_gate.py` is the judgement. Every number it compares comes from the report, and the report carries the `Fixed` knob values the bench read (`planner.gate.*`), so the script holds no threshold of its own.

The freeze decision is a `PlannerGateRun` node written through the served engine, one node per run, the latest one holding the state. The scope of the planner is the explicit state machine `PlannerScopeMachine` (`clear`, `frozen`; events `gate_failed`, `gate_passed`), described in `state-machines-v1.ttl`. `landing_refusal` is the check a landing gate calls to refuse a new R720 or R721 operator while the state is `frozen`. Why a graph node and not a file: the decision is a fact of the program that other gates read, and a committed generated artifact is not allowed. The R732 gate rerun against a new champion calls the same `record`.
## R738.16 and R738.17: state machine design calls

TraceOutcomeMachine: a `TraceOutcome` is written once, at the end of a trace, so the four
persisted statuses are all terminal. The machine needs an initial state to be a machine, so
`Pending` is the draft state: it has no wire text (`#[serde(skip)]`), a stored or decoded
record is never `Pending`, and the protocol schema is unchanged. `TraceOutcome::conclude` is
the one writer after construction.

UqlMachine: the census flagged `UqlStageReport.stage` by its name, but the field held the
rendered UQL clause, which is text and not a lifecycle. The text moves to `clause`. The real
lifecycle of a stage report is Planned (EXPLAIN estimate) to Measured (PROFILE attaches rows
and time), so `stage` becomes `UqlStage` and `rows` and `micros` are written only by
`UqlStageReport::measured`. This changes the wire shape of `UqlStageReport` (field `clause`,
enum `stage`); `medulla/generated/models.py` is edited in the same commit and the wire vector
needs a refresh (WIRE-REGEN).
## R721.2 modality fold, registration and store authority

`fold_modality` lives in `medulla-exec` and takes the modality's storage kind, because `medulla-plan` depends on `medulla-exec` and `medulla-exec` cannot name `PlanModality`; the one production caller is the SPARQL source (`sparql_source`), which folds its node bindings. It refuses a value whose storage kind is not the modality's, so one modality contributes only its own rows. Registration into `ModalityRegistry` happens once from `compose_server_state` (feature `plan-modalities`, part of `full`), with each of the seven value types `ConformanceTestable`. The store map is `modality_authority`, checked at boot by `verify_modality_authority` (an inconsistent map refuses to boot): graph, RDF and vector are authoritative in the graph shards (RDF statements are graph facts; the vector modality is the embedding projection the committed stream feeds); SQL, time series, key-value and blob each own one registered, non-retired store, and no two modalities share one.

## R739 census closure (w1-federated-query-4-9ff2f5)

- R739.1: the shard monitor machine moved to `medulla-decision-types` (`statistical::shard_monitor::SHARD_MONITOR_MACHINE`, edges S01-S03) so the ontology test reads the same table; `ShardMonitor::transition` is the only writer of `state` and refuses with the shared `TransitionRefused`. Vector refusals (width, non-finite) stay a separate typed variant (`ShardMonitorFailure::Vector`) because they are input rejections, not edges.
- R739.2: `SourceQueryStage` renamed `SourceQueryStep`, field `failed_step`.
- R739.3: decision: the `UqlProofStep` field is named `clause`, not `kind`, because the value is the canonical UQL clause text the step proves, not a variant tag. The Python mirror in `medulla/generated/models.py` was edited by hand (WIRE-REGEN).
- Rebatched to continue: R721.3 (no UQL syntax names a modality yet; the parser cannot see the encoder registry), R721.4, R736.2, R737.
## R039.1.1, R040.1.1, R040.1.2, R040.2.1: model rows built first

- `EmbeddingSpaceRef` carries `modalities` (empty for a single-modality space, serialized only when present). `with_modalities` binds all four modalities once each to a named encoder and folds the bindings into the digest; a space with no bindings keeps its existing digest and wire shape. A malformed multimodal space reports `EMBEDDING_SPACE_INVALID` in its message; the registered wire code belongs to the R039.1.4 refusal row.
- `RetrievalConstraints` lists are `BoundedVec` with named bounds (`MAX_RETRIEVAL_PREDICATES`, `MAX_PREDICATE_VALUES`); an unknown operator or quantifier fails at decode, so no later stage can weaken it.
- `compile_constraints` evaluates entity predicates with the engine's row-predicate logic (`pred_eval`), so a missing or differently typed property never satisfies a predicate, and follows edges with the traversal edge filter, so a constraint sees the same edges `Expand` does. The set is bound to the declared graph revision; refusing a stale revision belongs to R040.1.4.
- Topic backends share one weighing function (clipped cosine, normalized, cut by `TopicRankingPolicy`), so the fixture and the stored-centroid backend cannot disagree. The policy has no built-in default; the caller supplies both bounds from its resolved settings.


## R078.4 and R078.5: refusal point and trace shape

An unverified scope is refused where the scope is injected: `inject_scope` returns the typed `Unverified` error and the served `Uql` handler maps it to `PLAN_SCOPE_UNVERIFIED` before any leg binds or read runs. Any other scope that cannot bind is `PLAN_SCOPE_INVALID`. Both codes are registered server codes and routed to `Uql`. The scope evidence rides on the existing `Explain` and `Profile` results as `plan_scope_tenant` and `plan_scope_predicates` (one text per predicate of the closed set), so there is one trace shape and no second report type. A DAG program carries one linear scope and reports none per node.

## R078.6: the scope is the vector leaf's allow set
Decision: the executor's allow set is the scoped view itself. The read chokepoint (MEDULLA-IDENTITY-R610.3) already narrows the snapshot to the verified tenant's accessible graphs, so graph and edge leaves, which read only that view, are scoped by construction; re-deriving tenant placement in the executor would be a second access-check path. The vector leaf is different: the semantic store spans the whole graph, every tenant's embeddings included, and a ranking input can arrive from a maintained index outside the view. `run_unified_with` binds `Plan::scope` onto `PlanCtx::plan_scope`, and `vector_rank::constrained` (the one ranking filter, R040.1.3) drops every candidate the scope does not admit before `ranked` scores, so a denied row never competes on score. An unverified scope reaching the executor is refused `PLAN_SCOPE_UNVERIFIED`. The served adapter rewrite (`adapt_plan`) keeps the plan's scope and constraints instead of rebuilding a bare plan.

## Constraints reach every ranking leg through one filter (R040.1.3)
The request's `Plan` carries `constraints` (default none). `run_unified_with` compiles them over the caller's authorized snapshot once and binds the eligible set on `PlanCtx`; `vector_rank::constrained` is the one filter, called by the vector rank (fused rank-then-limit and whole-order alike), the server-side-embedded rank and the text rank leg, so no ranking path has its own. Because the filter sits in the ranking leaves, the optimizer cannot move a constraint after a rank. A plan with no ranking step reads no constraint (they bound what ranking may rank).

## Column advice receipt (R077.6.5)
The receipt is one typed struct on each advice row, not a second relation: the relation row and the receipt cannot disagree because the row holds no count, window or revision outside it. The policy revision is the policy name with every weight and floor, so a changed knob is a changed revision without a stored counter.

## Column advice settings and counters (R077.6.2 to R077.6.4)

Decision: every tunable of the advice is a registry knob in `knob_config.rs` (`sql_advice.window_hours`, `.min_samples`, `.w_demand`, `.w_length`, `.w_distinct`, `.length_norm`, `.max_columns`, `.max_rows`, `.stats_sample_rows`), never a `SchedulerConfig` field. Why: one tunable mechanism. The table-store crate cannot see the registry, so the caller passes resolved values (`AdvicePolicy`, `UsageBounds`).

Decision: the text-search counters are process-local statistics on the tenant's cached `TableStore` handle, bounded by `sql_advice.max_columns`. Why: the advice is a ranking over recent traffic, not a fact a reader must find after a restart; a restart opens a new window and advice reports `insufficient_observations` until it fills. Each tenant has its own handle, so counters are keyed by tenant and never shared.

Decision: refusals scope to the question. A read that pins one column on `information_schema.vector_advice` is admitted for that column (type, schema revision, model capability); a listing never refuses on account of another column. The retired `recommend_vectorization` (R077.4) is deleted; `is_embeddable` is the one type test.

LAYOUT-REGEN: not needed (no stored layout changed). WIRE-REGEN: not needed (no wire type changed).

## Shortfall answer and constraint refusals (R039, R040.1.4)
Decision: a rank-then-limit step records its `Shortfall` on the `RowSet` (`limit` keeps it, a step that rebuilds rows starts without one), and a `UnifiedQuery` whose plan sets `report_shortfall` is answered as `UnifiedQueryShortfall` (`RankedRows`), the same way `code_map` selects its own answer. Why: the row list answer of `UnifiedQuery` stays one shape for every existing caller, and no second method or untyped JSON is added. The flag is read only by the served handler; its answer is not cached.

Decision: `k` above the plan's result-row budget is capped and reported as `budget_exhausted`; the walk's widening rounds do not produce a shortfall because the exact top-up (EH-564) always follows them. `missing_embedding` is reported when any eligible candidate holds no embedding, else `eligible_exhausted`. The replenish loop is a bounded pure computation with a closed outcome, not a persisted lifecycle, so it carries no state machine. The rounds and first oversample are the registry knobs `retrieval.replenish_max_rounds` and `retrieval.oversample_factor`; the executor receives them as `ReplenishPolicy` on `PlanCtx` because the executor crate cannot read the registry.

Decision: the revision a constraint is checked against is the graph's version when the served indexes are opened (`ServedIndexes.graph_revision`). Revision `0` is no pin, because a client has no read of the current revision to pin to. Any other pin must match exactly: an older pin is stale, and a pin with no revision bound (a plan run outside a served read) is `CONSTRAINT_UNEVALUABLE`.

WIRE-REGEN needed: `Plan.report_shortfall`, the `UnifiedQueryShortfall` result and `RankedRows`/`Shortfall` (models.py edited by hand). LAYOUT-REGEN: not needed.

## Decision: the query encoder registry lives beside the encoder trait (R039.1.3)

`ModalityEncoderRegistry` is in `medulla-exec` (`exec/embed_registry.rs`), next to
`TextEmbedder`, not in `medulla-plan`: the served query handler reaches the planner
crate only under optional features, and the encoder binding must exist on every
served query path. Entries are encoder ids pinned to one space identity tuple; a
multimodal space names its encoders by id, so the lookup is by id and the tuple
must match. The engine registers the built-in deterministic `builtin:hash` encoder;
no entry calls an external service. The registry only binds query text today;
the other modality bindings are recorded on the space and compared by R039.1.2.
`EMBEDDING_SPACE_INCOMPATIBLE` (R039.1.2) is checked at rank time against the
store's declared space, so even an encoder bound some other way cannot rank
vectors of another space. No served method declares a space yet; the served test
declares it as a fixture step on the resident store.
## Constraint evidence (R040.1.5)
Decision: the constraint trace is computed by `compile_constraints` together with the eligible set and stored in the `ConstraintSet`, because that set is the one binding every ranking step (vector, text, fused branches, replenishment rounds) filters by; reporting it from the set makes "identical across replenishment and fusion" true by construction rather than by comparison. No `RetrievalReceipt` existed, so it is new, with the constraint trace as its first field, and it rides on the existing ranked answer (`RankedRows.receipt`, a plan with `report_shortfall`) instead of a second answer type. `predicate_id` is the predicate's list and position (`entity:<i>`, `relationship:<i>`); `outcome` counts the caller's authorized nodes the predicate held for and excluded, each predicate evaluated on its own so the counts do not depend on order. The digest is SHA-256 over the JSON encoding of the declared constraints. WIRE-REGEN needed: `RankedRows.receipt`, `RetrievalReceipt`, `TraceConstraint`, `PredicateTrace`, `PredicateOutcome` (models.py edited by hand).
- Topic fusion wiring (R040.2.2, R040.2.3): the request carries `Plan.topic` (backend kind plus topic query vector), beside `Plan.scope`, so no `Method::UnifiedQuery` literal changes. `execute` strips the plan's final `Limit`, fuses the whole ranking, then truncates, so the topic score can change which rows survive the limit. The dense score of a row is the score its ranking carried; a row with no stored embedding is refused `TOPIC_SPACE_MISMATCH` instead of being ranked without its topic contribution. A store with a declared embedding space answers in it; a legacy raw store declares none, so its topics live in the space its vector width defines. The topic weight and the policy bounds are registry knobs the served planner resolves; `medulla-exec` takes them as values (the registry stays in `medulla-server-api`). `TOPIC_SPACE_MISMATCH` and `TOPIC_WEIGHT_INVALID` are registered with the function that raises them; the other two R040.2.4 codes and the corpus-staleness and space-tuple checks belong to that row.

## R040.2.5: the topic receipt and where its numbers come from

`fuse_topic_dense_scored` (`crates/medulla-exec/src/exec/topic.rs`) is the one scorer; `fuse_topic_dense` is its projection to `(id, score)`. It keeps each row's min-max normalized dense part and its weighted centroid similarity, so the receipt reports exactly the numbers the score was computed from and a caller recomputes `score = (1 - weight) * dense + weight * topic` from the receipt alone. The plan-level fusion step in `execute` attaches the `TopicReceipt` to the final `RowSet` after the limit cut, so `component_scores` lists exactly the returned rows in result order; `execute_ranked` carries it into `RetrievalReceipt.topic` beside the constraint trace. `backend_revision` is the revision of the stored-centroid scoring contract (`STORED_CENTROID_BACKEND_REVISION`), raised when the weighing rule changes; `centroid_generation` is the corpus revision the centroids describe; `source_vector_generation` is the store's embedded-through version the candidate vectors were read at. `RetrievalReceipt` loses `Eq` because the receipt carries scores. WIRE-REGEN needed (`RetrievalReceipt.topic` and the `Topic*` receipt types; models.py edited by hand). LAYOUT-REGEN: not needed.

Topic backend interchange (R040.2.6): the second backend is the fixture promoted to a production backend, `in_memory`: the request declares its centroids in `TopicFusion.centroids` (bounded by the registry knob `retrieval.topic_max_declared`, refused `TOPIC_BACKEND_UNAVAILABLE` when absent, duplicated, non-finite or over the bound; a `stored_centroid` plan that declares centroids is refused the same way). The backend is selected by the plan field that already selects the stored backend, never by a test-only hook. Both backends share `weigh`, so the served journey selects each backend through `UnifiedQuery` over one corpus with identical centroids and compares every row and receipt component score; the L1 `two_backends_one_result_contract` runs the same comparison without the wire. Cross-tenant: a second tenant fusing topics on the same graph name sees none of the first tenant's stored centroids.
## R053.701 job-form submission: where the served path lives

A `Trino` or `Spark` job-form submission is served on the existing host-driven foreign-read lifecycle, not a new method: `ForeignReadOpen` gains an optional `submission`, and the open result returns the fence fields. The approved registration has no preset or snapshot catalog, so the server derives both from what the admission already pins: the one offered preset is the registration's approved operation, and the one known snapshot is the admitted pack digest (the host can choose neither). The submission id is `SubmissionId::derive` only. An identical resubmission by the same owner and effective actor resumes the live execution (one run); the lookup is keyed by owner scope, so another owner or tenant never resumes it. The shared snapshot check moved from `federation_trino.rs` to `federation_spark.rs`, because the trino module sits behind the `federation-sql` feature and the served handler needs it with plain `federation`. WIRE-REGEN needed (open request and result gain `submission`); `medulla/generated/models.py` is edited by hand in the same commit.
## R081.8.4 to R081.8.100: refusal order, feedback premise and strategy binding

The stop rule's limits hook (round, then budget) reads only the round, the usage and the limits, so `run_round` evaluates it before the plan runs; a spent continuation returns its terminal verdict and reads no row. Document and evidence ids are validated against the rows the round returned, which cannot be known earlier; the feedback shape, revisions, limits and the critique's model revision are refused before any row is read, and the expansion provider is reached only after every check passes. A critique made against another model revision is `RETRIEVAL_FEEDBACK_INVALID`. The feedback premise is built where the outcome is built and rides beside the stop premise; its `cost` is the usage the stop part reported, since medulla runs no model. A reused strategy rides in `RoundFeedback.strategy`; the target's corpus and model revisions are the round's own, so a request cannot name a looser binding. The retrieval-strategy refusal is `RETRIEVAL_STRATEGY_UNVERIFIED`, registered in `RetrievalErrorCode` and the `RetrievalRound` error routing, and `RETRIEVAL_FEEDBACK_INVALID` is registered there too. The receipt threshold floor is the registry knob `retrieval.transfer_min_threshold`. Wire models are not mirrored in `medulla/generated/models.py` for the retrieval round, so no hand edit applies; WIRE-REGEN: wire vectors refresh at the gate.

**R089.3 policy node.** `quality_policy_ref` names a `RetrievalQualityPolicy` node read from the caller's row-projected core. Its `min_composite_quality` and `min_population_ratio` (each in [0, 1]) replace the registry knobs for that call and its `revision` is the verdict's `policy_revision`; with no reference the revision is `default`. A reference to no visible node of that type, or a policy with a missing or out-of-range property, refuses `INVALID_ARGUMENT`. There is no policy cache.

**R091.1 plan-evolution store.** `ObservationStore` sits in the leaf crate `medulla-federation`, keyed by (tenant, shape digest). The crate cannot see the knob registry, so the composition root passes `federation.observations_per_shape` and `federation.shapes_per_tenant` in as `StoreLimits`. Eviction uses a per-tenant logical clock, so it is deterministic.
## R608.3 where CausalVerify is served

`Method::CausalVerify` is served by the query family beside `CausalEstimate` and `CausalCounterfactual`, not by the RDF handlers. It is a pure read of a request-carried structural model, the same shape those two already build into a `CausalGraph`, and the causal kernel is already a feature of `medulla-h-query` (`epistemic-causal`). Serving it from `medulla-h-sparql` would add a second path to the same kernel plus a new crate dependency and feature chain. The scope is `reasoning:run`, as the row names, and the read class is the epistemic-causal pure-compute reason. The fit limit is read from `reasoning_knobs().causal_nrmse_max` at request time. `CAUSAL_OBSERVATIONS_INSUFFICIENT` and `CAUSAL_MODEL_INVALID` are registered with the method (the first row that needs a refusal registers it) and routed by the `CausalVerify` method prefix. The Python `RuleReasonResponse` model gains the R608.1 fields (`complete`, `steps`, `rules_fired`, `bound_code`), which the served RunRules reply already carried. WIRE-REGEN needed: the query wire vectors gain `CausalVerify`.
## R083.2 and R085.4: arbitration support rule and the certified SQL audit

Arbitration classifies support first. A candidate with no claims has nothing backing it and is `unsupported`; any missing or invalid citation makes it `unsupported` even when another citation is outside the snapshot; otherwise a citation outside the snapshot makes it `unknown`; only an all-valid, non-empty claim set is `supported`. The span verifier is a trait so the served wiring (R083.3) implements it over `VerifySpan` of R144.1 on the request snapshot. A gap exactly equal to the margin abstains. `retrieval.arbitration_margin` is registered with R083.2.

The read-only SQL audit is one function. Both the federated SQL source and the approved source query call `audit_read_only_sql`; only approved queries use the closed function allowlist and refuse table-valued functions. Only Postgres and MySQL are certified dialects, because those are the two the visitor and the database read-only transaction are proven for; the other declared dialects refuse `unsupported_dialect` until a visitor is proven for them. The descriptor's `read_only_transaction` flag is not a Medulla record field: Medulla binds `ReadSql` as the only allowed operation and cannot enforce the driver transaction. LAYOUT-REGEN and WIRE-REGEN are not needed.

## R083.3: the arbitration part verifies citations on the round's snapshot

The arbitration part is the last block of `run_round`. The span verifier reads the cited document's `text` property through the round's bound view, so a document the caller cannot read is outside the snapshot and its citation classifies `unknown`, the same way a missing document does; it never confirms that another tenant's document exists. A span checks with the shared `verify_span` of the indexless-retrieval family (character boundaries plus SHA-256 of the slice), so there is one span check in the engine. The prior must be grounded `parametric` and the retrieved candidate `retrieved`; a part that swaps them refuses `RETRIEVAL_CONTRACT_INVALID`. The verdict's `graph_snapshot_revision` is the round's `corpus_revision`. The margin is the registry knob `retrieval.arbitration_margin`, read in the handler.

## R081.8 and R083.3: foreign-graph proofs use the door shape

A deployment admits one tenant, so a served call for another graph is refused at the door. The retrieval-round served tests assert that refusal (no write lands, no verdict returns) instead of seeding a second tenant's graph through the one-tenant harness. The two-tenants-in-one-store isolation case belongs in `crates/medulla-server-served-tests` (decision TENANT-SCOPE-TEST-SHAPE) and is owed there.
