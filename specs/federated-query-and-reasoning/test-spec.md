# Test specification and acceptance evidence

All tests run from the repository root with local fixtures, mock servers, or isolated ephemeral stores by default. Live foreign services are an optional integration profile with explicit provisioning. A cloud contributor can execute parser, optimizer, security, differential and contract gates without private infrastructure. For each accepted slice, record the exact commit, command, and exit status. Also record the fixture version and relevant assertion count in a reviewed evidence entry.

| Case | Requirements | Fixture and action | Expected result |
|---|---|---|---|
| Q-01 grammar exhaustiveness | FQR-01/02 | Every `OpKind`/`PredKind` plus 512 generated valid programs; parse → canonical print → parse. Run generated EBNF/dispatch check. | Same typed plan; no uncovered production variant. Unsupported feature gives `UQL_FEATURE_NOT_IN_BUILD` or `UQL_UNSUPPORTED`, with span. |
| Q-02 typed bindings | FQR-01/10 | Values with quotes, Unicode, JSONPath, arrays; unbound, mistyped and unused params; injection strings. | One bind result per declared type; no text interpolation; exact typed refusal for each invalid case. |
| Q-03 read-only/snapshot | FQR-02 | All UQL op variants, `LET`/`JOIN`, concurrent write between plan and execution. | No mutation-capable method; single authorized snapshot; budgets refuse at boundary without partial rows. |
| Q-03a reconciliation | MEDULLA-FEDERATED-QUERY-R007 | Interrupt a semantic-index source scan before complete-snapshot proof, then replay a complete scan. | The partial scan creates no tombstone; the complete scan tombstones exactly the absent sources. The focused `missing_sources_become_tombstones_only_after_complete_snapshot_proof` regression passes repeatedly. |
| Q-03b graph-free SQL | FQR-02 | DDL, DML, `COPY`, external-table statement and multi-statement payload through direct graph-free SQL and observability search. | Read-only guard refuses each mutation before execution; ordinary SELECT succeeds. |
| Q-04 proof/trace | FQR-02/08/10 | SPARQL/rule derivation, `WITH PROOF`, nested `LET`/`JOIN`, source with secret-like label. | Proof premises verify against generation; channels survive wire; trace has remote/local rows and no secret/URL/DSN. |
| F-01 result equivalence | FQR-03/06 | SQL, HTTP, RemoteEngine, SPARQL and OBDA mocks cover duplicate rows, repeated predicates, nulls, types and filter/order/limit/aggregate/join cases. Run optimized and naive full fetch. | Equal multisets, or equal ordered rows where ordered. No first-page truncation. Mock records lower or equal transfer when pushdown is declared. |
| F-02 budget and key source | FQR-03/04 | Set each request, row, key and wall limit one below needed work. Request full scan of `RequiresKeys`. | `FEDERATION_BUDGET_EXCEEDED:<dimension>` or `FEDERATION_SOURCE_REQUIRES_KEYS`. No partial success. |
| F-03 outbound policy | FQR-04 | Local/private/CGNAT/6to4/Teredo, redirect, DNS rebind, host placeholder, SQL injection, ungranted source, cross-owner key. | Refused before request or redirected hop; zero cross-tenant rows, credential bytes or network access. |
| F-04 capability fallback | FQR-03/05 | Source reports `Inexact` filter, lacks top-N, errors on batch, or has unknown stats; retry under rate limit. | Exact local residual; bounded fallback; trace names capability and estimate provenance. |
| F-05 cache/freshness | FQR-05 | Same canonical fragment across owners; watermark advance, grant revoke, missing watermark, schema change. | Only same-owner/current-watermark hit; all other cases miss or refuse. |
| F-06 OBDA/SERVICE/Iceberg | FQR-06 | Two predicate maps sharing rows, nonunique subject, `a`/`aa` template suffix, multivalued RDF predicate, partitioned Iceberg, SERVICE bind variables. | Differential answer equality; unsupported ORDER/aggregation falls back; manifest and request count prove pruning. |
| S-01 filtered search | FQR-07 | BM25 and ANN fixture with 10 permitted matches below unfiltered top-k; two-hop traversal and parallel edges. | Exactly k permitted IDs where available, stable edge identity; otherwise typed shortfall, not silent 7/10. |
| S-02 durable generation | FQR-07/08 | Crash during backfill, restart after activation, delete/replay, skipped reasoning outbox event, revoked tenant. | Atomic previous/new generation, correct scan fallback, rebuild on gap, no forbidden row. |
| S-03 bounded reopen | FQR-07 | Reopen graphs at increasing document counts with persisted index state and a deliberately incomplete state. | Complete state reopens without per-document rebuild; incomplete state returns typed partial-materialization until repair, then correct reads. |
| R-01 ontology corpus | FQR-08 | Shipped core schema plus minimal disjointness and deliberately inconsistent corpus. | Bounded compose with no stack overflow; EL/RL and tableau agree on consistency; unsatisfiable classes named; proof premises verifiable. |
| N-01 series and stats | FQR-09 | Two series at same timestamp, f64 values, constant/alternating/large windows, reference recompute. | No row collision or f32 truncation; flat window variance/z-score exactly zero; O(1) update measured independent of window length. |
| N-02 advanced kernels | FQR-09 | Exact small Shapley oracle, seeded sampled CI, DAG/cycle impact, barrier sigma→0, motif brute-force, high-tail literal. | Exact results where specified; seeded repeatability and CI coverage; high-tail relative error ≤1e-12; budget refusal is typed. |
| C-01 generated client | FQR-10 | Regenerate and check Rust method/error/receipt contracts; invoke UQL through Python generated sender. | Exact shape/code parity and served behavior; generated files show no manual drift. |
| F-07 ontology source selection | FQR-11 | Ontology with labels, a subclass mapping, two sources, an unapproved mapping and a disconnected class pair. | Path and mapping per class with premises; subclass mapping serves the superclass; unapproved class uncovered; disconnected pair answers no path. |
| F-08 operation-bound sources | FQR-04/11 | Recording MCP and GraphQL mocks with and without key-filter capability; budget one below need; ungranted source. | Equal rows with and without pushdown; residual always applied; typed budget and outbound refusals. |

## Quality and release gates

### MEDULLA-FEDERATED-QUERY-R045 / MEDULLA-FEDERATED-QUERY-R052 focused differential tests

| ID | Positive proof | Negative and boundary proof |
|---|---|---|
| MEDULLA-FEDERATED-QUERY-R045 | Mock SQL/HTTP/RemoteEngine rows include duplicate names, nulls, numeric/date values and a foreign-only filter. Exact push and inexact push plus local residual equal full-fetch ordered/multiset oracles. Projection retains hidden residual columns until evaluated. | Unsupported filters, ambiguous schemas, and changed types refuse or fall back. An inexact push missing a matching row also refuses or falls back. Projection dropping an authorization or ordering column refuses or falls back. The system never silently returns partial rows. |
| MEDULLA-FEDERATED-QUERY-R052 | A recording second Medulla instance receives canonical `Method::Uql` with only authorized keys and a sound LIMIT, returns the same ordered result as no push, and reports lower transferred row count. | Residual filter/top-N interaction, pagination, missing UQL capability, invalid signature, ungranted key, redirect/private target and exhausted request/row budget refuse or use a safe bounded fallback; no secret appears in trace. |

Run changed-file CCCC, Dupehound, KISS and differential jscpd from `.config/pre-commit.yaml`; the configured CCCC threshold is no new function above cyclomatic 10 or cognitive 15 and no regression. Differential jscpd must show zero newly introduced clone pairs. KISS and Dupehound use their checked-in configuration, with zero attributable violations. Run `cargo fmt --all -- --check`, targeted `cargo test --locked` for each changed crate, full-feature Clippy with `-D warnings`, generated-contract checks, and the repository's hosted CI matrix for the final PR head. `bash scripts/ci_parity.sh` is a local CI preview. Record any unavailable live-service profile separately; it cannot be replaced by a fabricated pass. Every new public source kind needs a mock conformance suite before a live environment is considered.

## Evidence record format

For each accepted stable ID: `ID | public merged SHA | test command/profile | expected vs actual | quality gate | client/contract digest | reviewer/date`. A task checkbox, source branch, historical log or local-only build never substitutes for exact-head evidence. If a test is omitted, state the risk and named replacement before the ID can be accepted.

## MEDULLA-FEDERATED-QUERY-R056 focused acceptance

| Requirement | Proving test | Coverage and result |
|---|---|---|
| MEDULLA-FEDERATED-QUERY-R056 | `crates/medulla-mining/src/mining/causal_impact.rs::tests::two_sided_p_keeps_relative_accuracy_in_the_far_tail` | Passed against exact production modules and real libm: independent literal probabilities for z = ±3, ±6, ±8, ±10, ±20 at relative error < 1e-12. Z = 0 and +infinity boundaries. All five causal-impact tests passed. |

Canonical crate command: `cargo test -p medulla-compute --lib mining::causal_impact::tests`.
The bounded local run compiled the exact module with `rustc
--test --edition=2021`. It imported production numeric modules by path and linked
the cached real libm. This is pure-module acceptance, not a full-crate or hosted
release result. See the owning plan's audit record for revision and limits.

### NL cache source guard (MEDULLA-FEDERATED-QUERY-R058.3.1)

Base: `f154c5b4a115713ab8ff3e1d9643e2295c4ebe43`. Extract the exact `nl.rs`
test module into a standalone Rust test crate. Keep the original `nl.rs`
alongside it to preserve `include_str!` resolution. The old guard fails
(exit 101, one failed test). All three repaired tests pass (exit 0).
The real NL source remains uncached. The guard rejects injected `ResultCache`
references and direct or generic cache-aware response calls. Missing
production/test boundaries and the uncached response path fail closed.

The probe uses `rustc --edition=2021 --test --cfg 'feature="nl-query"' probe.rs`.
It excludes production compilation and proves the source guard only. Native
facade execution was not repeated in this repair; the separate native failure
and resource limits remain outside this evidence. Rust formatting, diff
whitespace, and public-spec checks pass. No production API or grammar changes.

Column vectorization (R077): a bound text column answers a nearest-neighbour query and EXPLAIN names the binding. An unbound column and a non-text column are refused. One row write queues exactly one re-embed.

## MEDULLA-FEDERATED-QUERY-R078 acceptance

| Scenario | Requirement | Fixture |
|---|---|---|
| Out-of-tenant isolation | R078.6 | Two tenants hold nodes, edges and vectors. A query as tenant A returns only A rows. |
| Unverified caller | R078.4 | A statement without a verified tenant returns `PLAN_SCOPE_UNVERIFIED` and reads nothing. |

## MEDULLA-FEDERATED-QUERY-R079 and R080 acceptance

These are required acceptance cases, not claims of completed tests.

| Scenario | Requirement | Fixture |
|---|---|---|
| Publication model | R079.1 | Round-trip version, tenant, schema, snapshot, locations and digests; reject missing binding and unsupported format. |
| Manifest writing compatibility | R079.2, R079.6 | Authoritative served graph append, compaction and copy-on-write deletion produce v2 manifests. A pinned independent reader queries their Parquet data through the authenticated catalog. Check counts, paths, sizes, partition/schema IDs and sequence metadata. |
| Schema evolution and time travel | R079.2, R079.3, R079.6 | Authoritative graph schema changes add, drop, rename and reorder. The external reader resolves current and retained snapshots, stable field IDs and expected rows. Identify the real graph handler, publisher and REST routes. |
| Atomic publication failure | R079.2, R079.4 | Fail artifact durability before metadata visibility. The previous snapshot remains readable. The catalog advertises no incomplete snapshot. Retry cannot expose duplicate committed state. |
| Unapproved drop and invalid publication | R079.4 | Unapproved drop, unknown column, absent graph binding and unsupported manifest features return their typed codes. Visible versions remain unchanged. |
| Correlated publication evidence | R079.5 | Match committed graph version and LSN to schema/snapshot IDs, artifact digests, outcome and time-travel resolution. Record reader version and feature matrix. |
| Projection stays read-only | R080.2, R080.4, R080.6 | Reject create, data commit, metadata/schema commit, rename, drop and unknown mutation through the projection. Assert HTTP 405, `PROJECTION_READ_ONLY`, no graph change and no publication call. |
| Boundary preservation | R079.3, R080.6 | Graph write becomes externally readable through the engine publisher without a separate copy job. No external-source adapter effect or credentials are invoked by manifest publication; requested source effects require the existing Graft SDK-governed contract. |

### MEDULLA-FEDERATED-QUERY-R081 tests

- `MEDULLA-FEDERATED-QUERY-R081.1`: `crates/medulla-result-types/src/retrieval_verdict.rs::tests::verdict_round_trips_and_rejects_unknown`.
- `MEDULLA-FEDERATED-QUERY-R081.2`: `crates/medulla-retrieval/src/retrieval_verdict.rs::tests::repair_order_ends_at_round_limit`.
- `MEDULLA-FEDERATED-QUERY-R081.3`: `crates/medulla-exec/src/exec.rs::tests::execute_returns_verdict_for_rule_plan`.
- `MEDULLA-FEDERATED-QUERY-R081.4`: `crates/medulla-retrieval/src/retrieval_verdict.rs::tests::strict_insufficient_abstains`.
- `MEDULLA-FEDERATED-QUERY-R081.5`: `crates/medulla-retrieval/src/retrieval_verdict.rs::tests::verdict_premise_lists_all_fields`.
- `MEDULLA-FEDERATED-QUERY-R081.6`: `tests/retrieval/test_sufficiency_served.py::test_insufficient_returns_boost_k`.

### MEDULLA-FEDERATED-QUERY-R082 tests

- `MEDULLA-FEDERATED-QUERY-R082.1`: `crates/medulla-result-types/src/retrieval_stop.rs::tests::table_and_verdict_round_trip`.
- `MEDULLA-FEDERATED-QUERY-R082.2`: `crates/medulla-plan/src/retrieval_stop.rs::tests::repeat_and_margin_stops`.
- `MEDULLA-FEDERATED-QUERY-R082.3`: `crates/medulla-exec/src/exec.rs::tests::execute_returns_stop_verdict_with_history`.
- `MEDULLA-FEDERATED-QUERY-R082.4`: `crates/medulla-plan/src/retrieval_stop.rs::tests::uncalibrated_round_refuses`.
- `MEDULLA-FEDERATED-QUERY-R082.5`: `crates/medulla-plan/src/retrieval_stop.rs::tests::stop_premise_lists_margin_and_reason`.
- `MEDULLA-FEDERATED-QUERY-R082.6`: `tests/retrieval/test_stop_rule_served.py::test_repeat_with_margin_stops`.

### MEDULLA-FEDERATED-QUERY-R083 tests

- `MEDULLA-FEDERATED-QUERY-R083.1`: `crates/medulla-result-types/src/retrieval_arbitration.rs::tests::verdict_carries_both_candidates`.
- `MEDULLA-FEDERATED-QUERY-R083.2`: `crates/medulla-plan/src/retrieval_arbitration.rs::tests::margin_picks_or_abstains`.
- `MEDULLA-FEDERATED-QUERY-R083.3`: `crates/medulla-exec/src/exec.rs::tests::execute_returns_arbitration_for_prior_candidate`.
- `MEDULLA-FEDERATED-QUERY-R083.4`: `crates/medulla-plan/src/retrieval_arbitration.rs::tests::abstain_returns_both_candidates`.
- `MEDULLA-FEDERATED-QUERY-R083.5`: `crates/medulla-plan/src/retrieval_arbitration.rs::tests::arbitration_premise_lists_both_classes`.
- `MEDULLA-FEDERATED-QUERY-R083.6`: `tests/retrieval/test_arbitration_served.py::test_small_gap_abstains_with_both`.

## R081.7 through R081.9 served acceptance

These cases specify future evidence. The real served tests and activation benchmark must satisfy their acceptance criteria.

| Scenario | Requirements | Positive and negative acceptance |
|---|---|---|
| Typed contracts | R081.7.1, R081.8.1, R081.9.1 | Round-trip revisions, IDs, budgets and receipts. Refuse missing provenance, invalid limits and non-finite scores. |
| Dual-score golden cases | R081.7.2, R081.7.4, R081.7.6 | With fixture thresholds s=0.7 and r=0.6, rescue (0.2,0.9) and reject (0.2,0.1). Retain dual-high (0.9,0.9). For w=0.3 and residual threshold 0.4, reject (0.9,0.1), whose fused score is 0.34. |
| Adaptive count and activation | R081.7.3, R081.7.5, R081.7.6 | Two corpus queries return different accepted counts. Compare the fixed-count baseline with identical scoring calls. Selection adds zero inference. Refuse missing or mismatched benchmark approval. |
| Empty and constant scores | R081.7.2, R081.7.4 | Empty accepted sets return insufficiency. Constant-score normalization follows its declared rule or refuses. Never pad with dual-low evidence. |
| Feedback and perspectives | R081.8.2, R081.8.3, R081.8.6 | Successive served rounds produce diverse queries and exact reinforce/missing/suppress tiers. Corpus feedback raises useful term weights and lowers misleading ones. |
| Feedback refusal and critique | R081.8.4, R081.8.5, R081.8.6 | Refuse forged or stale feedback. Record critique and its effect; hostile instructions cannot change measured scores, scope or exhausted budgets. |
| Hierarchy descent | R081.9.2, R081.9.3, R081.9.6 | Assert summary filtering, community refinement and entity search in order. Compare selected paths and final IDs to an independent corpus oracle. |
| Hierarchy refusals and costs | R081.9.4, R081.9.5, R081.9.6 | Refuse cycles, stale summaries and missing entities. Assert exact visited-node and scoring counters, plus recorded measured cost and bounded stopping. |
| Scope and evidence | R081.7.5, R081.8.5, R081.9.5 | Two-tenant fixtures prove every stage applies scope before truncation. Independently read receipts and trace; no forbidden entity, summary or feedback is consumed. |

Fixture thresholds above are deterministic test values, not production defaults or claimed paper parameters. The benchmark defines production thresholds before activation. Use actual served operators and pinned production inference artifacts; helper-only results and mocked terminal responses do not establish acceptance.

### R081.8 / R082 uncalibrated continuation acceptance

`tests/retrieval/test_feedback_served.py::test_uncalibrated_continuation_precedence` covers R081.8.1, R081.8.3 through R081.8.6 and R082.1 through R082.6. Start an authenticated served query against a deterministic corpus and pinned production inference artifacts. Obtain Medulla continuations from actual completed rounds; do not inject terminal responses or replace production operators. Omit the calibration row for the next round. Retain the same tenant, corpus revision and policy. (planned)

| Completed round / remaining limit | Required served result |
|---|---|
| New accepted documents, positive quality gain, all limits available | `stop=false`, `continue_uncalibrated`, `ROUND_NOT_CALIBRATED`; one bounded next continuation. |
| Zero new accepted document IDs, limits available | `stop=true`, `no_new_evidence`, `ROUND_NOT_CALIBRATED`. |
| New documents with quality gain at the declared saturation threshold for the declared window | `stop=true`, `quality_saturated`, `ROUND_NOT_CALIBRATED`. |
| Round limit reached and progress also saturated | `stop=true`, `round_limit`, `RETRIEVAL_BUDGET_EXCEEDED`, `ROUND_NOT_CALIBRATED`. |
| Each query, term, token or inference limit exhausted; saturation also present | `stop=true`, `budget_exhausted`, exact exhausted dimension, both refusal codes. |
| Forged, stale or cross-tenant saturation evidence | `RETRIEVAL_FEEDBACK_INVALID`; no continuation or successful confidence claim. |

For every missing-calibration case, assert absent margin/threshold and preserved R081 insufficiency. Read the decision trace and dispatch counters independently; terminal and invalid continuations issue zero subsequent retrieval, generation or assessment calls. Repeating a terminal continuation cannot reset budgets. A calibrated control still stops only when the repeated answer exceeds its margin threshold. Equality continues when no earlier terminal condition applies. These are acceptance requirements, not executed runtime evidence.

## Retrieval policy extensions: served acceptance

These acceptance scenarios have not been executed.
All test paths below are planned; this definitions batch adds no runtime test or implementation.

The harness must start an authenticated engine with disposable persistent storage and two independent tenant clients.
It must pin corpus, policy, model artifacts, preprocessing, tokenizer, graph and index generations.
It must invoke real transport, authorization, production dispatch, planner, model bindings and index reads.
It must not mock those components or inject finished rankings, feedback, topic scores or receipts.
A deterministic hash embedder cannot establish semantic or cross-modal acceptance.
The evidence reader must use a separate authorized client and read the actual trace and persisted records.

| Planned test | Positive acceptance | Negative acceptance |
|---|---|---|
| `tests/retrieval/test_multimodal_served.py` | Issue all sixteen text/image/audio/video direction pairs through UnifiedQuery. Read expected permitted matches and shared space identity. Verify actual model invocations and maintained index reads. | Delete one authorized target embedding and assert exact missing_embedding shortfall. Delete query binding and refuse before rank. Reject same-width incompatible spaces, unavailable modalities and cross-tenant inputs. (planned) |
| `tests/retrieval/test_constraints_served.py` | Load a real relationship fixture where the best similarity violates a required edge. Require permitted lower-scored results and exact replenishment. Compare optimized and unoptimized results with a separate exhaustive graph oracle. | Contradictory predicates produce zero eligible results and typed shortfall. Unsupported operators and stale graph revisions refuse. Prohibited candidates remain absent from text, vector and topic branches. (planned) |
| `tests/retrieval/test_section_tree_served.py` | Ingest a three-level section tree without vectors. Invoke approved production reasoning and read complete ordered paths, spans and pruning evidence. Assert zero embedding/index calls. | Reject cycles, multiple roots, dangling parents, invalid spans and stale summaries. Exhaust each limit separately and together. Refuse before further work; preserve completed incomplete paths and tenant isolation. (planned) |
| `tests/retrieval/test_chunk_encoding_served.py` | Governed ingestion must encode the same long document with local-reset and absolute-offset policies. Capture actual token ranges and independent model calls. Recompute mean and final normalization independently, then query the single published vector through UnifiedQuery. | Reject overlap at least size, unsupported offsets, non-finite or mismatched vectors and failed chunks. Empty input cannot publish a vector. Policy-generation mismatch must refuse; failed replacement must retain the prior complete generation. (planned) |
| `tests/retrieval/test_column_advice_served.py` | Create two equally distributed text columns. Issue column-specific text-search queries, then select from column_vector_advice through Sql. Reverse observed demand and assert reversed recommendations. Independently verify recorded workload contributions. | Reject unembeddable types before recommendation scoring. Cover no observations, stale schema, denied tables and unavailable encoders. Assert zero new bindings, embedding calls and write-side effects. (planned) |
| `tests/retrieval/test_topic_fusion_served.py` | Bind two production topic backends sequentially to the same corpus and space. Query at weights 0, 0.5 and 1. Independently reproduce normalized scores and stable ordering from receipts. | Reject mismatched dimensions, model revision, preprocessing, space tuple and corpus generation. Reject invalid responsibilities, zero vectors and missing backends. Hidden-topic data must never affect ranking or evidence. (planned) |
| `tests/retrieval/test_mixed_traversal_topk_served.py` — MEDULLA-FEDERATED-QUERY-R039.4 | Reuse the S-01 two-hop traversal and parallel-edge fixture. Include ten permitted matches below unfiltered top-k. Run authenticated mixed traversal-plus-vector queries against that reference corpus and require stable edge identity. Include a query whose first k similarity candidates fail the traversal filter but later candidates survive. The independent oracle must confirm exactly k permitted results when available. Otherwise, require an explicit typed shortfall naming the permitted match count. | Fail acceptance for a silent undercount or premature truncation when enough permitted matches exist. Reject a wrong shortfall count or an unauthorized result or count. Test fewer than k surviving candidates and zero survivors. |
| `tests/retrieval/test_filtered_scan_benchmark_served.py` — MEDULLA-FEDERATED-QUERY-R040.4 | Benchmark filtered ANN and BM25 latency before and after the filter-aware scan change. Use the same fixed corpus and query workload. Run the independent S-01 filtered-search correctness oracle alongside both measurements. Read actual plans and candidate traces to prove pre-filtered candidate sets or bitmap filters reach index scans. | Fail acceptance if either search mode lacks a measured before/after result or correctness proof. Reject a changed corpus or workload presented as a comparison. Reject a plan that scans the full unfiltered index and discards non-matching rows afterward. |

The residual cases above retain the complete original R039 and R040 acceptance, independently of the new policy extensions.
Their served entry is UnifiedQuery through `crates/medulla-h-query/src/query/planning.rs::run_unified_with` and `crates/medulla-exec/src/exec.rs::execute`.
The mixed query must reach `crates/medulla-exec/src/exec.rs::traverse_op` and `crates/medulla-exec/src/exec/vector_rank.rs::rank_top`.
The filtered scan benchmark must reach that vector path and `crates/medulla-exec/src/exec.rs::rank_text` with real maintained indexes.
Record corpus digest, query workload, source revisions, index generations, benchmark environment, measured latency and oracle results.
The original R040 contract requires the comparison; it supplies no numerical latency target or claimed speedup.
These residual acceptance scenarios have not been executed. Existing parent receipts do not establish acceptance for the new leaf IDs.

For chunk numerical comparison, use absolute tolerance 1e-6 and relative tolerance 1e-5 for f32 aggregate outputs.
Pin the backend numerical mode and record any stricter tolerance required by its contract.
Exact token boundaries, counts, identities and generations permit no tolerance.
The fixture must include partial final chunks and nonzero overlap.
Different position modes require independently checked position inputs; an assumed difference in ranking is not proof.

For constraints, test required and forbidden directed edges plus typed entity predicates.
Set k above the number of surviving candidates and verify typed constraint-exhaustion shortfall.
For multimodal shortfall, include both missing vectors and unauthorized vectors.
Unauthorized items must contribute neither identifiers nor counts to the response.

For section traversal, rerun the existing community journey in `tests/retrieval/test_hierarchy_served.py`. (planned)
Document-section selection must not alter community stages or entity paths.
Rerun `tests/retrieval/test_feedback_served.py::test_uncalibrated_continuation_precedence`. (planned)
Invalid feedback, round exhaustion, other budget exhaustion, no new evidence and saturation must retain their existing precedence.
Neither section selection nor new fusion may claim sufficiency from an uncalibrated terminal result.

Restart before independent readback of published encoding generations, section revisions and retained receipts.
Record exact source revision, enabled features, test command, policy revisions, measured work and result.
Unavailable production bindings prevent served acceptance. Helper-only checks do not establish served acceptance.

## R084 planned ShEx acceptance

These are proposed tests in tests/shex_served.rs. None supplies execution evidence in this definition batch.
Each child has a focused case. The rollup requires every child and the served journey.

| Requirement | Planned proving test |
|---|---|
| MEDULLA-FEDERATED-QUERY-R084 | `served_shex_contract_journey` |
| MEDULLA-FEDERATED-QUERY-R084.1 | `wire_report_preserves_shape_map` |
| MEDULLA-FEDERATED-QUERY-R084.2 | `inline_and_live_data_selection` |
| MEDULLA-FEDERATED-QUERY-R084.3 | `bounded_constraints_report_results` |
| MEDULLA-FEDERATED-QUERY-R084.4 | `semantic_limits_remain_visible` |
| MEDULLA-FEDERATED-QUERY-R084.5 | `real_dispatch_reaches_shex_validator` |
| MEDULLA-FEDERATED-QUERY-R084.6 | `parse_and_dispatch_failures_are_distinct` |
| MEDULLA-FEDERATED-QUERY-R084.7 | `live_validation_obeys_read_authority` |
| MEDULLA-FEDERATED-QUERY-R084.8 | `served_shex_contract_journey` |

Start a real authenticated engine with the shex handler enabled and disposable durable storage.
Send actual ShexValidate requests through the existing native protocol.
Keep transport, authorization, dispatch, data export and medulla-shex validation real.
Observe handle_shex_validate, validate and shex_report_wire without substituting their results.
Record source SHA, binary digest, features, request identity, graph scope and terminal result.

Use a ShExJ schema with two focus nodes, one conforming and one nonconforming.
Assert result order, lexical node identities, labels, per-node reasons and the aggregate.
Repeat duplicate pairs and an empty map. Empty-map success must carry no whole-graph validation claim.
Check START and unknown-label outcomes separately from malformed JSON.
Submit malformed Turtle and an explicitly unsupported expression type.
Exercise a real live-export error or retain its missing proof as an open acceptance item.
A build without the handler must fail dispatch, not return a synthetic conforming report.
Use existing approved build infrastructure for that case; no feature change is requested here.

For live selection, write known authorized graph data and send empty and whitespace-only data_graph.
Compare with a matching inline Turtle request, then a different inline graph.
Prove inline data does not import rows or change stored schema.
Use a second graph and hidden rows to test the authority projection through dispatch.
Read graph and schema state independently before and after successful and refused requests.
A denied request must not disclose hidden triples through a reason or result.

Keep constraint tests bounded: node kinds, datatypes, a value set, facets, cardinality and closed/extra behavior.
Distinguish sequential EachOf from the first-success OneOf implementation.
Include shared-predicate, inverse and recursion-boundary fixtures as explicit semantic-limit evidence.
Record both expected semantics and actual results. Do not rewrite the oracle to hide a discrepancy.
Those cases cannot certify unsupported full-language behavior.
Crate parser tests cannot stand in for served ShExJ syntax, authorization or report projection.

## R085 planned evidence

The following are proving targets, not execution receipts. Runtime delivery requires the real producer and the linked Graft SDK consumer.

- **MEDULLA-FEDERATED-QUERY-R085:** All child tests and linked consumer evidence. Each child must pass; these definitions do not establish runtime delivery.
- **MEDULLA-FEDERATED-QUERY-R085.1:** `crates/medulla-result-types/src/source_query.rs::tests::approval_records_round_trip` (planned). Round trips preserve every binding. Invalid references, parameter types, nonfinite values, absent snapshot identity and zero/unbounded limits refuse.
- **MEDULLA-FEDERATED-QUERY-R085.2:** `crates/medulla-h-sparql/src/federation/source_query_tests.rs::approval_requires_authority_and_native_commit` (planned). A successful response follows durable issuance. Generic lease writes cannot forge approval. Issuance uses existing storage and credentials; no external SQL executes during approval.
- **MEDULLA-FEDERATED-QUERY-R085.3:** `crates/medulla-h-sparql/src/federation/source_query_tests.rs::changed_or_revoked_approval_refuses` (planned). Changing any binding, authority, source or revision refuses. Revoked, expired, consumed, unavailable or missing leases refuse. Verification grants no SQL effect and creates no bearer credential.
- **MEDULLA-FEDERATED-QUERY-R085.4:** `crates/medulla-federation/src/federation_tests.rs::approved_source_query_has_no_write_effects` (planned). Native parameter binding preserves values and query meaning. Supported projections, predicates, ordering and limits match the query oracle. Unknown dialects, functions or effects refuse without issuing approval.
- **MEDULLA-FEDERATED-QUERY-R085.5:** `tests/source_query_approval_served.rs::real_dispatch_issues_and_verifies_approval` (planned). Real authenticated dispatch reaches source validation, native issuance and fresh verification. No parallel planner, store or client-side approval authority appears. Current source has neither method.
- **MEDULLA-FEDERATED-QUERY-R085.6:** `tests/source_query_approval_served.rs::bounded_stream_preserves_approval` (planned). Oversized rows, exceeded budgets or failed revalidation stop execution and close the driver. Exactly one explicit completion or typed failure stops an execution. Missing terminal completion never means success.
- **MEDULLA-FEDERATED-QUERY-R085.7:** `tests/source_query_approval_served.rs::failure_after_three_chunks_is_terminal` (planned). Failure after three chunks stays failure with an incomplete execution identity. No successful terminal event or failed-chunk checkpoint follows. Earlier accepted pages retain their actual durable receipts.
- **MEDULLA-FEDERATED-QUERY-R085.8:** `tests/source_query_approval_served.rs::approval_to_connector_query_journey` (planned). Keep row widths, driver settings and all limits fixed while increasing cardinality by two orders of magnitude. Declare the baseline, memory envelope and tolerance before runs. Every peak must fit that fixed envelope; whole-result buffering fails. Record exact revisions, features, commands, rows/bytes, queue high-water marks, terminal outcomes and checkpoint agreement.

## Source-backed acceptance targets and explicit holds
All targets below are planned acceptance, not executed tests or receipts. Use the real producer/consumer boundary named for each row and retain existing covered behavior.

- **`MEDULLA-FEDERATED-QUERY-R006.1` / EG:11. Source `2c2ffcf18d947730bc60a935fed942e1f82e9e8547f4509fe2941a34672fb8f3`:** After owner decisions are recorded, acceptance uses two source facts with sentence and axiom origins. It exercises matching and nonmatching temporal intervals. Read back the derived fact with rule digest, graph generation and complete origin refs. Missing/stale origins, unsupported rule versions, conflicting validity windows and unauthorized writes must refuse without changing graph revision. Until then no producer test is specified as executable.

- **`MEDULLA-FEDERATED-QUERY-R040.5` / EG:20. Source `07df52b453f0f1b7be7aec7bf790f087c3fc5e13500915011382a9fcd8d9d41a`:** Blocked until the owner binds admission, relation, retention and benchmark semantics. The benchmark compares fixed query-selectivity workloads at multiple corpus sizes. It reports rows/bytes/visits and demonstrates the declared cost bound. Read back new knowledge; prove prior records and model revision remain unchanged. Missing admission authority, relation target or bound refuses. No runnable acceptance is asserted before the missing policy is supplied.

- **`MEDULLA-FEDERATED-QUERY-R083.7` / EG:22:** Extend existing R083 behavior/model/refusal acceptance with the support-margin inputs and invariant that unsupported candidates lose. Add a served counterexample with a higher-confidence unsupported candidate and an evidence-supported lower-confidence candidate. Retain current two-candidate, class, abstention and evidence obligations. Producer/consumer path: Planned MERISTEM-RETRIEVAL-R006 prior probe -> served cross-modal query -> medulla_plan::execute -> arbitrate -> ArbitrationVerdict -> harness. R083.3/.6 identify the producer path. This audit does not claim it is implemented. Source review `544b7b0054e8c7cd9a76aedd0d665589d555a0bcf20d1f00679442574936af36`. Exercise the served Medulla producer and named consumer where that owner contract exists. Assert its defined output/refusal. These are planned cases only, not test or receipt evidence.

- **`MEDULLA-FEDERATED-QUERY-R081.10` / EG:25. Source `fb8d54d8554b8158da64f7c9e8bf02ea846e5b411768a0381c2682b2e6302dac`:** Once defined, serve a fixture where all three clocks disagree and another where one clock is absent. Assert the selected basis and its provenance, and trigger exactly the configured bounded fallback only below the declared sufficiency threshold. enough first-pass context must issue no second pass. Stale or conflicting clock provenance and exhausted query budget must return a typed partial/refusal with no unbounded retry. No executable acceptance is currently authorized.

- **`MEDULLA-FEDERATED-QUERY-R028.1` / EG:29. Source `2eb8aadb1b6ccc2d5de77a4d1964fc04a4e944bccf9bcc1793e262f146af7583`:** Create one plain row and one row with resolved evidence/policy frames. Compare both with the same structured KnowledgeSet producer. Verify row id/score/projection are unchanged and both frame states are row-local. Independently omit provenance, policy and snapshot binding. Each case must return the typed refusal without a partial row. Keep R045 foreign-row pushdown behavior unchanged. Tests are acceptance targets only. No UQL implementation or served receipt is claimed.

- **`MEDULLA-FEDERATED-QUERY-R005.1` / EG:31. Source `973de1c3b38646a438d057299d8070070f0ef7a2056bb30acff6d1a3814df513`:** For asymmetry, `(a,p,b)` alone is the clean control. Adding `(b,p,a)` creates a violation. It cites the declared asymmetric axiom, both edge ids and evidence refs at pinned graph/ontology revisions. A missing reverse edge is not a violation. Serve positive/control pairs for temporal conflict, cardinality, self-loop and ontology defect. Combine two independent defects and require both diagnostics. Refuse missing kind-specific fields, unknown kinds and stale ontology revisions. Read back graph state to prove validation made no mutation. Retraction remains denied absent explicit authorization.

- **`MEDULLA-FEDERATED-QUERY-R078.7` / EG:59. Source `fd5990a7c9f30e1e1aa0eed9aa08efef4e2ebc8f5f34387598f1c9e577c458dc`:** With category mappings supplied, serve identical company-context queries as two tenants with differing clearances. Verify the authorized tenant receives each mapped category as source-bound references. Exclude denied rows before any source read and copy no source record. Join native time-series values only by the declared series/time key and snapshot. Missing/ambiguous mapping, unverified tenant, stale source revision or cross-tenant join refuses. Current scope-planning acceptance remains covered by R078.1-.6. This new composed view stays held until mappings are exact.

- **`MEDULLA-FEDERATED-QUERY-R081.9.7.7` / GRAPHOS:24. Source `b2e5de8386b3ca72b59a81ca5fa3193eaf7736af86aca472a720b9e4511961c0`:** Serve all three granularities over a nested authorized section tree. Compare selected path/span ids with an independent tree oracle. Instrument external model, embedding and vector calls and require zero calls on every positive query. Malformed or stale section records and exhausted depth/visit/token limits refuse before extra dispatch. Tenant isolation and existing community traversal outputs remain unchanged. No runtime proof is claimed.

- **`MEDULLA-FEDERATED-QUERY-R081.9.7.8` / GRAPHOS:25. Source `6797cff839c16c3b9c3877d6bbf48bf8953eae8696087abc99f2b57147daed93`:** Once the owner supplies TOC generation and versioned positive finite `max_lines` and `max_bytes`, index a multisection fixture offline. Read a bounded middle span and verify exact line numbers and source digest. Refuse stale document/TOC revision, invalid or too-wide range, a response above the bound, and whole-file requests. Verify current Arbora space/section links still resolve. Until producer and caps are named, no served acceptance is claimed.

- **`MEDULLA-FEDERATED-QUERY-R040.2.7` / GRAPHOS:28. Source `f7654cd07d4d95f32f09fec742c4172b35c689749b3228aef9424830b1f7cc1b`:** Serve the same corpus/backend with two candidates whose assignments differ. Independently recompute both topic components and final scores. Change only one candidate’s responsibility vector and prove only the expected candidate contribution/order changes. Refuse missing/duplicate document assignment, stale document or corpus revision, backend mismatch and embedding-space mismatch before truncation. Verify hard exclusions still hold and `run_unified_with` records the assignment/backend revision in the existing topic receipt. No implementation or served receipt is claimed.

## Planned mapped acceptance
No test named here has run. Names describe acceptance obligations, not evidence.

- **`MEDULLA-FEDERATED-QUERY-R081.8.100` (AU:77. Source `213700ec5b9e8648657ec47d0c587483a978db10f4661475b467be451d3bb2e1`):** Same-retriever feedback reuse succeeds. Retriever-version changes require validation. Failed, stale or missing transfer receipts refuse retrieval. Verify the receipt binds the strategy digest and target retriever. Keep feedback budgets unchanged. Rejected critiques must not change the active strategy. Producer/consumer boundary: Authenticated crossmodal -> `medulla-plan::exec` R081.8 feedback -> selected retriever. Preserve existing R081.8.1-.6 tiering, feedback and refusal behavior. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-FEDERATED-QUERY-R053.100` (EG:37. Source `803f4e4ffbcd939af750e23451469835555beaa37e14ce555ffd0a59c0a3ac48`):** Build-time test enters the Cypher query-construction path. Check generated property keys for plain, reserved, spaced, Unicode and embedded-backtick names. Parse safe output against the Cypher grammar. An unquoted or malformed generated key fails the build. Keep values parameterized, preserve ForeignSourceSpec registration and Spark batch-only capability, and add no served API or runtime receipt. Gate evidence binds renderer source revision, fixture digest and test result.

- **`MEDULLA-FEDERATED-QUERY-R032.100` (AU:43. Source `327ced0e0605f34366c662017d82ac892a481c5ca5f40bba2f3693ba6a5f7649`):** Graph fixtures cover ancestors and descendants across three layers. Include differing historical failure counts, a cycle boundary and missing-history data. Verify both directions and deterministic layer/history contribution. Refuse stale or malformed history, and compare existing R032 propagation outputs unchanged. Producer/consumer boundary: Medulla owns graph impact query/risk decision; Meristem requests and consumes the result. Existing numeric risk helpers are not this two-direction historical impact contract. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

## Final mapped planned acceptance
No cases listed here have run. They describe required future acceptance, not evidence.

- **`MEDULLA-FEDERATED-QUERY-R086.100` / EG:14. Source `7223250e7f899a187aad0b152e597c6174b772c49779ca2bdd12d3ba68f4f5ef`:** Acceptance is blocked until model, evidence-class, authorization and Meristem receipt decisions are published. A served ordered three-source fixture includes an unseen-at-fit entity. Return candidates with tuple order, graph/model/policy revisions and evidence class. Leave graph revision unchanged. Changing source order must change request digest. Refuse unary tuples, unresolved/unauthorized sources, stale revisions and malformed relation constraints. Confirm `GraphLearnPredict` binary results and write authority are unchanged. No producer test is currently defined as runnable. Parent `MEDULLA-FEDERATED-QUERY-R086` is linked to this sole required child. Both remain SPECIFIED.

- **`MEDULLA-FEDERATED-QUERY-R081.101` / AU:126. Source `deca151180a749cb3784d9946da0de17205eca89c92e5eb4f275859e1d4be165`:** Build state once, then rerank under two query contexts and reversed candidate order. Assert the state digest stays identical and per-document scores remain aligned. Assert no decoder/model-generation call. Compare measured interaction cost with the same full-token baseline. Refuse stale document/encoder revisions, malformed segment coverage or any query-time state rewrite. Producer/consumer: Medulla retrieval owns document-state construction and PlanCtx reranking. Query embeddings/context remain query inputs and do not alter the reusable state. Preserve existing FQR R081 tiering and selection bounds. Parent `MEDULLA-FEDERATED-QUERY-R081` remains intact.

- **`MEDULLA-FEDERATED-QUERY-R081.102` / GRAPHOS:27. Source `baac22c9fb42470957b5b91499c6e42e1bb2410d24446d15f6f364638c88138e`:** Distill and read back an artifact meeting held-out quality threshold. Serve queries and assert zero model-service calls plus scorer-digest binding. Below-threshold, missing held-out evidence, stale corpus/index or teacher revision all refuse activation. Verify the prior scorer remains active. Producer/consumer: Medulla owns retrieval learning/ranking and the stored scorer. Meristem may execute evaluation. Arbora catalog ranking is separate. Use the existing authenticated UnifiedQuery -> run_unified_with planning route. Do not invent a public operation name. Parent `MEDULLA-FEDERATED-QUERY-R081` remains intact.

## Mapped retrieval-selection acceptance
No case listed here has run.

- **`MEDULLA-FEDERATED-QUERY-R081.100` / EG:41. Source `bceeed106b5db2933cf5161d58b4203fc7afe9d714503a31d68323992906c23b`:** Compare served retrieval on a tiny fixture with an independent exhaustive nonnegative-sparse oracle. Include two highly correlated documents and an alternative diverse pair. Assert exact selection, coefficients, residual and deterministic order. Refuse embedding-space mismatch, invalid policy and non-convergence. Verify existing tenant and result-count filters still apply. Producer/consumer: Medulla retrieval selection owns the joint objective and set result. The existing served UnifiedQuery/retrieval path consumes it. Preserve R081.1-.9 selection and tenant bounds. No new caller-controlled score authority. Parent `R081` remains intact.

## Planned served acceptance — viewport query

No test in this section has run.

- `MEDULLA-FEDERATED-QUERY-R081.103`: serve scatter and line variants over a real indexed source of at least 1,000,000,000 points. Verify viewport-derived bin caps and `min(requested_byte_ceiling, formula_cap)` encoded-byte behavior at minimum/maximum sizes, source/cardinality/index revision and line gaps. Missing, invalid and too-small requested ceilings refuse before scanning. Pan/zoom changes query digest. Exact-point and aggregate-bin selections remain revision-bound. Stale index/source, denied selection, invalid/exceeded cap and any full-source transfer fail acceptance.
- Pan/zoom and exact/aggregate selection prove viewport-bound tokens. Missing index, stale revision, denied point/bin and invalid/exceeded caps refuse. Verify no full-source payload or silently dropped bins.

## MEDULLA-FEDERATED-QUERY-R058.4 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-FEDERATED-QUERY-R058.4.1` | A serde round trip keeps every field and rejects `max_steps = 5`. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R058.4.2` | Unit tests cover linking, each dialect choice and one corrected step. |
| Wiring | `MEDULLA-FEDERATED-QUERY-R058.4.3` | A dispatch test observes one schema read per request. |
| Refusal | `MEDULLA-FEDERATED-QUERY-R058.4.4` | Each case returns its code and no rows. |
| Evidence | `MEDULLA-FEDERATED-QUERY-R058.4.5` | A test reads the step list and the digest. |
| Hot-path proof | `MEDULLA-FEDERATED-QUERY-R058.4.6` | The served test passes, and the measured overhead meets the budget. |

Snapshot grounding acceptance for `MEDULLA-FEDERATED-QUERY-R058.4.7.1` uses the `medulla-plan` NL context suite. It covers full-snapshot discovery beyond 4096 nodes, attribution property repairs, named-source authority, JOIN scope and mixed fusion scope. Feature-enabled tests check source scope replacement and spatial/tensor layers. Served tests must show that each caller sees only names from its owner-authorized registry.

`MEDULLA-FEDERATED-QUERY-R058.4.7.2` remains open. Acceptance must cover the native identifier schemas named in that row. The parent `MEDULLA-FEDERATED-QUERY-R058.4.7` remains open until both children complete.

Ontology-name acceptance for `MEDULLA-FEDERATED-QUERY-R058.4.8` uses the mapped-property and ambiguity tests in the same suite.

`MEDULLA-FEDERATED-QUERY-R058.4.9` remains open. Its current negative test refuses an inline HTTP adapter before any source can run. Completing it requires positive tests for a schema-bound adapter and cross-owner refusal.

Hot-path proof for `MEDULLA-FEDERATED-QUERY-R058.4`: `tests/nl_query_bounded_steps.rs::second_step_corrects_parse_error`.

## MEDULLA-FEDERATED-QUERY-R088 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-FEDERATED-QUERY-R088.1` | A serde round trip keeps every field and the default row shape. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R088.2` | A unit test shows that hidden rows do not displace visible rows. |
| Wiring | `MEDULLA-FEDERATED-QUERY-R088.3` | A dispatch test observes one snapshot for rows and summaries. |
| Refusal | `MEDULLA-FEDERATED-QUERY-R088.4` | The test asserts the code and no partial rows. |
| Evidence | `MEDULLA-FEDERATED-QUERY-R088.5` | A test reads the header fields. |
| Hot-path proof | `MEDULLA-FEDERATED-QUERY-R088.6` | The served test passes, and the measured cost meets the budget. |

Hot-path proof for `MEDULLA-FEDERATED-QUERY-R088`: `tests/uql_governed_summary.rs::governed_rows_carry_summary`.

## MEDULLA-FEDERATED-QUERY-R089 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-FEDERATED-QUERY-R089.1` | A serde round trip keeps every field and rejects an unknown failure mode. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R089.2` | Unit tests cover each failure mode. |
| Wiring | `MEDULLA-FEDERATED-QUERY-R089.3` | A dispatch test reads the policy node and returns the verdict. |
| Refusal | `MEDULLA-FEDERATED-QUERY-R089.4` | Tests assert the code and a failed verdict with rows. |
| Evidence | `MEDULLA-FEDERATED-QUERY-R089.5` | A test reads the three fields. |
| Hot-path proof | `MEDULLA-FEDERATED-QUERY-R089.6` | The served test passes, and the measured cost meets the budget. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R089.7` | `crates/medulla-h-graph/src/graph_ops/discover_quality.rs::tests::drift_mode_reads_monitor_and_names_adapter` sets `drift` and names the adapter and shard generations. |

Hot-path proof for `MEDULLA-FEDERATED-QUERY-R089`: `tests/discover_quality_verdict.rs::sparse_index_fails_closed`.

## MEDULLA-FEDERATED-QUERY-R090 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-FEDERATED-QUERY-R090.1` | A serde round trip keeps every field and rejects `hops = 3`. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R090.2` | Unit tests cover fusion, decay, the neutral date and the frontier cap. |
| Wiring | `MEDULLA-FEDERATED-QUERY-R090.3` | A dispatch test observes one planner call and batched neighbor reads. |
| Refusal | `MEDULLA-FEDERATED-QUERY-R090.4` | Each case returns its code and no rows. |
| Evidence | `MEDULLA-FEDERATED-QUERY-R090.5` | A test reads each field. |
| Hot-path proof | `MEDULLA-FEDERATED-QUERY-R090.6` | The served test passes, and the benchmark receipt meets the budget. |
| Wiring | `MEDULLA-FEDERATED-QUERY-R090.7` | `tests/retrieval/test_discover_planning_served.py::test_deep_arms_use_scoped_adapter` observes one adapter application per arm and refresh provenance. |

Hot-path proof for `MEDULLA-FEDERATED-QUERY-R090`: `tests/retrieval/test_discover_planning_served.py::test_deep_recency_multihop`.

## MEDULLA-FEDERATED-QUERY-R073.2 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Evidence | `MEDULLA-FEDERATED-QUERY-R073.2` | `crates/medulla-federation/src/federation_opt/tests.rs::protocol_matrix_bounds_pushdown`: A unit test fails when a declared protocol lacks a row. It refuses a pushdown kind outside the row. |

## MEDULLA-FEDERATED-QUERY-R091 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-FEDERATED-QUERY-R091.1` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::observation_store_bounds`: A serde round trip keeps every field and rejects an unknown kind; the store evicts at its bound. |

| Behavior | `MEDULLA-FEDERATED-QUERY-R091.2` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::keep_or_revert_by_measurement`: A faster equal candidate is kept, and a slower or unequal candidate is reverted. |

| Wiring | `MEDULLA-FEDERATED-QUERY-R091.3` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::served_path_records_and_reads`: A served query records one observation, and a later query uses the kept plan. |

| Refusal | `MEDULLA-FEDERATED-QUERY-R091.4` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::evolution_refusals`: Each case returns its code, and the active plan stays the baseline. |

| Evidence | `MEDULLA-FEDERATED-QUERY-R091.5` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::receipt_names_measurements`: A served read finds the receipt linked to the shape. |

| Hot-path proof | `MEDULLA-FEDERATED-QUERY-R091.6` | `tests/federation/test_plan_evolution_served.py::test_kept_plan_measurably_faster_and_mismatch_reverts`: The kept plan improves p50 by 10% or more, the unequal candidate reverts, and the overhead meets the budget. |

Hot-path proof for `MEDULLA-FEDERATED-QUERY-R091`: `tests/federation/test_plan_evolution_served.py::test_kept_plan_measurably_faster_and_mismatch_reverts`.

## MEDULLA-FEDERATED-QUERY-R653.7 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Citation | `MEDULLA-FEDERATED-QUERY-R653.7` | `tests/retrieval/test_web_citation_served.py::test_hybrid_hit_cites_source_url`: A served fused hit cites the URL, the retrieval time and the query. |

## Context engine and governance query rows

| Row | Proving test |
|---|---|
| `MEDULLA-FEDERATED-QUERY-R703` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R703.1` | `crates/medulla-plan/tests/access_check.rs::request_round_trips` |
| `MEDULLA-FEDERATED-QUERY-R703.2` | `crates/medulla-plan/tests/access_check.rs::timeout_drops_hits` |
| `MEDULLA-FEDERATED-QUERY-R703.3` | `tests/retrieval/test_access_check_served.py::test_denied_hit_never_enters_bundle` |
| `MEDULLA-FEDERATED-QUERY-R703.4` | `tests/retrieval/test_access_check_served.py::test_missing_port_fails_closed` |
| `MEDULLA-FEDERATED-QUERY-R703.5` | `tests/retrieval/test_access_check_served.py::test_drop_reasons_read_back` |
| `MEDULLA-FEDERATED-QUERY-R703.6` | `cargo bench -p medulla-plan --bench access_check` |
| `MEDULLA-FEDERATED-QUERY-R703.100` | `scripts/tests/test_bench_gate_p99.py::test_p99_over_ceiling_fails_and_missing_sample_fails` |
| `MEDULLA-FEDERATED-QUERY-R704` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R704.1` | `crates/medulla-plan/tests/knowledge_scope.rs::scope_round_trips` |
| `MEDULLA-FEDERATED-QUERY-R704.2` | `crates/medulla-plan/tests/knowledge_scope.rs::scope_prefilters_before_truncation` |
| `MEDULLA-FEDERATED-QUERY-R704.3` | `tests/retrieval/test_knowledge_scope_served.py::test_agent_sees_only_scoped_sources` |
| `MEDULLA-FEDERATED-QUERY-R704.4` | `tests/retrieval/test_knowledge_scope_served.py::test_scope_refusals` |
| `MEDULLA-FEDERATED-QUERY-R704.5` | `tests/retrieval/test_knowledge_scope_served.py::test_scope_digest_recorded` |
| `MEDULLA-FEDERATED-QUERY-R704.6` | `cargo bench -p medulla-plan --bench knowledge_scope` |
| `MEDULLA-FEDERATED-QUERY-R705` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R705.1` | `crates/medulla-plan/tests/feedback_boost.rs::feedback_round_trips` |
| `MEDULLA-FEDERATED-QUERY-R705.2` | `crates/medulla-plan/tests/feedback_boost.rs::boost_decays_and_clamps` |
| `MEDULLA-FEDERATED-QUERY-R705.3` | `tests/retrieval/test_feedback_boost_served.py::test_upvote_raises_rank` |
| `MEDULLA-FEDERATED-QUERY-R705.4` | `tests/retrieval/test_feedback_boost_served.py::test_feedback_refusals` |
| `MEDULLA-FEDERATED-QUERY-R705.5` | `tests/retrieval/test_feedback_boost_served.py::test_boost_listed_per_hit` |
| `MEDULLA-FEDERATED-QUERY-R705.6` | `cargo bench -p medulla-plan --bench feedback_boost` |
| `MEDULLA-FEDERATED-QUERY-R706` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R706.1` | `crates/medulla-plan/tests/policy_mask.rs::policies_round_trip` |
| `MEDULLA-FEDERATED-QUERY-R706.2` | `crates/medulla-plan/tests/policy_mask.rs::one_rewrite_serves_every_language` |
| `MEDULLA-FEDERATED-QUERY-R706.3` | `tests/test_policy_mask_served.py::test_masked_column_over_sql_and_uql` |
| `MEDULLA-FEDERATED-QUERY-R706.4` | `tests/test_policy_mask_served.py::test_mask_refusals` |
| `MEDULLA-FEDERATED-QUERY-R706.5` | `tests/test_policy_mask_served.py::test_explain_lists_policies` |
| `MEDULLA-FEDERATED-QUERY-R706.6` | `cargo bench -p medulla-plan --bench policy_mask` |
| `MEDULLA-FEDERATED-QUERY-R707` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R707.1` | `crates/medulla-rdf/tests/sparql_update.rs::update_forms_parse` |
| `MEDULLA-FEDERATED-QUERY-R707.2` | `crates/medulla-rdf/tests/sparql_update.rs::multi_operation_update_is_atomic` |
| `MEDULLA-FEDERATED-QUERY-R707.3` | `tests/test_sparql_update_served.py::test_insert_then_select` |
| `MEDULLA-FEDERATED-QUERY-R707.4` | `tests/test_sparql_update_served.py::test_update_refusals` |
| `MEDULLA-FEDERATED-QUERY-R707.5` | `tests/test_sparql_update_served.py::test_update_receipt_reads_back` |
| `MEDULLA-FEDERATED-QUERY-R707.6` | `cargo bench -p medulla-rdf --bench sparql_update` |
| `MEDULLA-FEDERATED-QUERY-R708` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R708.1` | `crates/medulla-source-types/src/stored_query.rs::tests::stored_query_round_trips` |
| `MEDULLA-FEDERATED-QUERY-R708.2` | `crates/medulla-plan/tests/stored_query.rs::on_change_refreshes` |
| `MEDULLA-FEDERATED-QUERY-R708.3` | `tests/federated/test_stored_query_served.py::test_put_then_call` |
| `MEDULLA-FEDERATED-QUERY-R708.4` | `tests/federated/test_stored_query_served.py::test_stored_query_refusals` |
| `MEDULLA-FEDERATED-QUERY-R708.5` | `tests/federated/test_stored_query_served.py::test_staleness_reported` |
| `MEDULLA-FEDERATED-QUERY-R708.6` | `cargo bench -p medulla-plan --bench stored_query` |
| `MEDULLA-FEDERATED-QUERY-R709` | See each child. |
| `MEDULLA-FEDERATED-QUERY-R709.1` | `crates/medulla-rdf/tests/geosparql.rs::functions_registered` |
| `MEDULLA-FEDERATED-QUERY-R709.2` | `crates/medulla-rdf/tests/geosparql.rs::topology_matches_kernel` |
| `MEDULLA-FEDERATED-QUERY-R709.3` | `tests/federated/test_geosparql_served.py::test_within_query` |
| `MEDULLA-FEDERATED-QUERY-R709.4` | `tests/federated/test_geosparql_served.py::test_geo_refusals` |
| `MEDULLA-FEDERATED-QUERY-R709.5` | `tests/federated/test_geosparql_served.py::test_explain_names_spatial_index` |
| `MEDULLA-FEDERATED-QUERY-R709.6` | `cargo bench -p medulla-rdf --bench geosparql` |
| `MEDULLA-FEDERATED-QUERY-R709.7` | `crates/medulla-rdf/tests/geosparql.rs::indexed_and_scanned_results_are_equal` (planned) |

## Shared query workspace

| Row | Proving test |
|---|---|
| `MEDULLA-FEDERATED-QUERY-R708.7` | `tests/federated/test_stored_query_served.py::test_workspace_share_schedule_and_lint` |

## Agent-memory substrate claims
| Case | Requirement | Proving test |
|---|---|---|
| One UQL plan across graph, vector, SQL, SPARQL and epistemic stages (rollup) | `MEDULLA-FEDERATED-QUERY-R720` | Rollup: satisfied when the test of every child passes. |
| Epistemic row fields | `MEDULLA-FEDERATED-QUERY-R720.1` | `crates/medulla-exec/src/rowset.rs::tests::epistemic_fields_pass_through` |
| Confidence, bitemporal and time-decay stages | `MEDULLA-FEDERATED-QUERY-R720.2` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::confidence_asof_decay` |
| Embedding-similarity clustering stage | `MEDULLA-FEDERATED-QUERY-R720.3` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::cluster_by_embedding_is_deterministic` |
| Wiring | `MEDULLA-FEDERATED-QUERY-R720.4` | `tests/uql_single_plan_all_modalities.rs::served_program_runs_one_plan` |
| Refusal | `MEDULLA-FEDERATED-QUERY-R720.5` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::decay_and_cluster_refusals` |
| Hot-path proof | `MEDULLA-FEDERATED-QUERY-R720.6` | `tests/uql_single_plan_all_modalities.rs::five_stage_kinds_one_plan` |
| Modalities are views over one RowSet algebra in one store (rollup) | `MEDULLA-FEDERATED-QUERY-R721` | Rollup: satisfied when the test of every child passes. |
| Each modality implements the modality contract | `MEDULLA-FEDERATED-QUERY-R721.1` | `crates/medulla-modality/tests/rowset_views.rs::seven_modalities_implement_contract` |
| Wiring and store authority | `MEDULLA-FEDERATED-QUERY-R721.2` | `tests/test_store_registry_modalities.py::test_every_modality_has_one_redb_authority` |
| Refusal | `MEDULLA-FEDERATED-QUERY-R721.3` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::unknown_modality_refuses` |
| Hot-path proof: cross-modal queries with default configuration | `MEDULLA-FEDERATED-QUERY-R721.4` | `tests/advanced_crossmodal_roundtrip.rs::fresh_engine_answers_cross_modal_query` |

- `MEDULLA-FEDERATED-QUERY-R722`: See each child.
- `MEDULLA-FEDERATED-QUERY-R722.1`: `crates/medulla-types/tests/pack_source.rs::access_kinds_map_to_foreign_source_kinds`.
- `MEDULLA-FEDERATED-QUERY-R722.2`: `crates/medulla-plan/tests/pack_source.rs::held_serves_none_approved_serves_rows_and_changed_contract_returns_to_held`; `crates/medulla-h-admin/src/admin/connector_pack/tests/source_approval.rs::approve_source_requires_current_head_and_contract_digest` proves the admitted approval CAS; `tests/federated/test_pack_source_served.py::test_import_registers_through_the_one_catalog` proves the served catalog import.
- `MEDULLA-FEDERATED-QUERY-R722.4`: `tests/federated/test_pack_source_served.py::test_secret_guard_held_and_unavailable_refuse_with_named_codes`.
- `MEDULLA-FEDERATED-QUERY-R722.5`: `tests/federated/test_pack_source_served.py::test_rows_carry_contract_provenance_and_nothing_is_copied`.
- `MEDULLA-FEDERATED-QUERY-R722.6`: `tests/federated/test_pack_source_served.py::test_uql_joins_native_fact_to_live_connector_rows_with_zero_copy`.

## MEDULLA-FEDERATED-QUERY-R058.5 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Behavior | `MEDULLA-FEDERATED-QUERY-R058.5` | `nl_regressions::planner_sends_configured_extra_body_and_honours_read_timeout` sees the extra body at a stub server and a timeout inside 3 s. |
| Default | `MEDULLA-FEDERATED-QUERY-R058.5` | `nl_regressions::planner_request_defaults_disable_thinking` sees `enable_thinking=false` in the body. |
| Refusal | `MEDULLA-FEDERATED-QUERY-R058.5` | `nl_regressions::planner_refuses_invalid_request_config_with_typed_errors` returns each `NlConfigError` variant. |
| Served refusal | `MEDULLA-FEDERATED-QUERY-R058.5` | `nl_config_refusal_tests::invalid_planner_config_is_refused_with_code_and_key` sees `ENGINE_UNAVAILABLE` and the bad key in the detail. |
| Raw value | `MEDULLA-FEDERATED-QUERY-R058.5` | `nl::tests::unparseable_env_values_are_refused_with_source_and_raw_value` sees the env name or config key and the raw text in the detail. |

## MEDULLA-FEDERATED-QUERY-R040.5 planned acceptance — low-storage ANN

These are planned acceptance cases. They have not run. They establish no native, served, deployed, or benchmark evidence. Each case uses a fresh temporary durable engine and two authorized tenants. It uses the real source and index lifecycle and a separate evidence reader. Each case pins the source-text snapshot and digest. It pins the `EmbeddingSpaceRef` model, revision, tokenizer, and preprocessing identity. It also pins the `DocumentEncodingPolicy`, the compact graph policy, the lane-private build workspace, and every finite budget. Each case enters through the existing `UnifiedQuery`, `run_unified_with`, and `PlanCtx` path. No case injects a finished ranking or receipt.

| Row | Planned proving case | Required positive and negative observations |
|---|---|---|
| `MEDULLA-FEDERATED-QUERY-R040.5.1` | `crates/medulla-ann` accounting and manifest tests | The test round-trips source reference, space, policy, generation, build workspace, byte components, and ceilings. It compares the recorded sum with an independent checked-add oracle. It refuses a missing source digest, a mixed space or policy, overflow, and persisted or peak over-budget values. The active generation stays unchanged. |
| `MEDULLA-FEDERATED-QUERY-R040.5.2` | `crates/medulla-ann` compact-artifact and exact-oracle tests | The test builds one fixture in compact and compatibility modes. It decodes bounded adjacency, codes, and tombstones and keeps stable IDs. It compares candidate membership with the exact full-vector oracle. It refuses a malformed version, degree, code shape, duplicate ID, dangling edge, and source-generation mismatch. |
| `MEDULLA-FEDERATED-QUERY-R040.5.3` | Core `EmbeddingProvider::regenerate_eligible` plus embeddingless compact query and rescore integration | The test changes one source revision and verifies selective regeneration. A compact query makes no source fetch. Exact rescore stays under source, byte, and item ceilings. The provider refuses a stale digest, model space, encoding policy, generation, or failed replacement. The prior complete generation stays active. |
| `MEDULLA-FEDERATED-QUERY-R040.5.4` | Core `EmbeddingProvider` multi-source batch and cache seam | The test sends duplicate source rows and verifies one counted provider effect. It exercises bounded batches and the exact-key cache. It verifies that keys hold tenant, source digest, space, and policy. It refuses exhausted budgets, wrong vectors, provider failures, stale keys, and foreign-tenant lookups. `TextEmbedder` stays unchanged. |
| `MEDULLA-FEDERATED-QUERY-R040.5.5` | Compact graph pruning tests | With pruning disabled, the topology is byte-identical. With a pinned policy, edges are deterministic, hubs stay, no edge dangles, and the graph stays connected. The test refuses invalid bounds, missing digests, policy mismatch, and disconnected output. It reports measured recall and storage per fixture. |
| `MEDULLA-FEDERATED-QUERY-R040.5.6` | `tests/retrieval/test_low_storage_ann_served.py::test_revision_bound_low_storage_ann_path` | The test builds, updates, restarts, queries, and reads the receipt independently through authenticated `UnifiedQuery`. It compares fixed corpus and selectivity workloads with the exact full-scan oracle. It reads source-text, model, and build-space digests and the generation. It reads bytes, rows, visits, p50, p99, rescore, batch, and cache counters. It reads recall@k. It refuses budget exhaustion, a stale generation, unauthorized source, and an incomplete receipt before any effect. No external storage-savings claim serves as acceptance. |

The benchmark matrix records actual results under declared ceilings. It sets no storage or accuracy target. A deterministic hash embedder serves seam and refusal tests. Semantic quality needs a pinned real model and independent readback. These cases do not replace R039.1.1, R039.2.1, R040.3, R040.4, R059, or R077.3. Those rows stay required.

### R722.1 import-held registration slice

- `source::tests::access_kinds_map_to_foreign_source_kinds` checks the closed source kinds.
- `sources::tests::imported_contract_is_held_and_identifies_its_authoritative_body` checks source claims and absent declarations.
- `sources::tests::publisher_approval_and_literal_credentials_refuse` checks admission authority and credential references.
- `ontology::source_claims_persist_held_and_old_or_other_tenant_members_stay_unbound` exercises signed import, tenant isolation, unchanged import and store reopen.

These tests do not prove live source execution or completion of R722.2 through R722.6.

### R722.2 admitted source approval contract

- `crates/medulla-h-admin/src/admin/connector_pack/tests/source_approval.rs::approve_source_requires_current_head_and_contract_digest` sends the existing `ConnectorPackOp::ApproveSource` / `approve_source` operation with a read-only admitted context. It proves the `agent:pack-control` action, tenant/current-head/body-digest CAS, and the approved `ConnectorPackSourceApprovalResult` batch evidence.
- The same test retries an exact approved target and gets the idempotent result. It then imports a changed contract and proves the new current member is held. The old approval cannot be reused. It also refuses an absent registration, stale `PackHeadRef`, and mismatched current member digest without changing the head.
- `tests/federated/test_pack_source_served.py::test_import_registers_through_the_one_catalog` remains the served catalog proof. Arbora's R042.2 policy id/reason is recorded by Arbora; EG's approval result carries only the engine-owned registration/head/digest and durable batch evidence. (planned)

This contract does not add a human approval step, a new capability scope, a publisher-supplied approval bit, a second source registry, or live driver execution. No acceptance claim is made until native execution and the served test pass.

### R722.3 owner-bound execution seam (tracked, pending)

The following matrix names the required evidence for the first runtime binding.
The tests are planned and remain open; the existing R722.1/R722.2 tests do not
prove any of these rows.

| Case | Requirement | Planned proving test | Required observation |
|---|---|---|---|
| Registry identity and precedence | R722.3 | `crates/medulla-federation/src/federation_registry_tests.rs::eg722_bound_driver_resolves_naively_and_retains_spec`; `crates/medulla-federation/src/federation_opt/remote.rs::bound_driver_tests::bound_driver_precedes_spec_and_preserves_identity` | The bound driver retains the registered source kind, operation and fingerprint. Naive and optimized resolution select the same bound source; the spec fallback is never used. |
| Registration and context binding | R722.3/R042/R085 | `crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs::binding_requires_verified_owner_tenant_and_current_registration` | Verified context supplies tenant, owner, actor and delegation. Wrong tenant/owner, missing approval, stale head, changed body or contract digest refuse before a driver call. Revoked authority and changed plan/query digest also refuse. |
| Four-phase lifecycle | R722.3/R043.7; SDK-SOURCE-INGEST-R004.7-.10 invariants mapped by the SDK owner | `tests/federated/test_pack_source_served.py::test_approved_source_open_chunk_complete_abort_is_bounded` | `ForeignReadOpen` issues one opaque bound handle. Chunks use monotone sequence and finite row/byte/deadline caps. Complete or Abort is the only terminal transition. Replay, expiry, cancellation and over-limit cases return named errors. Existing R004.7-.10 alone do not prove a generic pack receiver. |
| Governed source call | R043.7/R722.3 | `tests/federated/test_pack_source_served.py::test_approved_source_reaches_fleet_call_once` | The served path calls `FleetSourceExecutor` through `fleet.call` with read effect and never opens a direct URL or second transport. (planned) |
| Provenance and zero copy | R722.5 | `tests/federated/test_pack_source_served.py::test_bound_source_rows_keep_provenance_without_source_storage` | Live rows carry connector, contract, mapping and fetch-time provenance. A store audit finds no source-row writes for a non-materialised source. (planned) |
| Refusal and partial availability | R722.4 | `tests/federated/test_pack_source_served.py::test_bound_source_refusals_and_unavailable_gap` | Held/unbound, forbidden destination and stale/revoked cases return stable typed errors. Source-down cases return an explicit source gap. Native rows remain correct. No silent empty result occurs. The path never falls back. (planned) |
| Cross-source hot path | R722.6 | `tests/federated/test_pack_source_served.py::test_uql_dag_joins_native_and_live_bound_rows` | Served UQL/DAG joins native facts and live connector rows. It preserves dependency concurrency and cancellation. It produces equal answers for naive and optimized resolution. (planned) |

No row above is an acceptance result yet. The named native and served tests must
run against the same reviewed source tree and record positive and refusal
evidence. R722.2 approval evidence remains separate from this runtime proof.

### R722.3 EG native leaf evidence (tracked, pending served integration)

These tests bind the current EG DTO, query-pin and mapped-cell implementation to
the R722.3 contract. They record evidence for the EG leaf only. They do not
mark the cross-owner rows above complete. They do not replace the served
`FleetSourceExecutor`/SDK tests.

| Case | Requirement | Proving test | Evidence boundary |
|---|---|---|---|
| Lifecycle DTO and bounded limits | R722.3 | `crates/medulla-catalog-types/src/foreign_read.rs::tests::lifecycle_dtos_round_trip_with_digest_and_optional_provenance`; `::tests::bounded_limits_reject_zero_and_over_cap`; `crates/medulla-server-ops/src/api/query_txn/foreign_read.rs::tests::foreign_read_rows_match_sdk_msgpack_golden_fixture` | MessagePack fields, mapped cells and provenance round-trip. Zero and over-cap limits refuse. Rust `rmp_serde` bytes and SHA match the connector SDK golden fixture. |
| Historical ExplainPlan compatibility | R722.3 | `crates/medulla-protocol-types/src/protocol/support_01.rs::explain_plan_result_tests::historical_response_without_plan_digest_still_decodes` | A response without the additive optional digest decodes as `None`; a foreign client must fail closed. |
| Canonical plan/query binding pin | R722.3 | `crates/medulla-server-ops/src/api/query_txn/foreign_read.rs::tests::canonical_plan_digest_is_the_query_binding_pin`; `crates/medulla-h-query/src/query/planning.rs::foreign_query_digest_tests::uql_query_digest_binds_text_and_typed_parameters`; `crates/medulla-h-query/src/query/uql_statement.rs::served_tests::explain_returns_server_owned_foreign_query_pins`; `crates/medulla-h-query/src/query/uql_statement.rs::served_tests::prepare_returns_pins_for_the_exact_executable_text`; `tests/advanced_crossmodal_roundtrip.rs::explain_plan_surfaces_the_optimizer_rewrite` | The server helper and ExplainPlan use identical named-MessagePack bytes. Authenticated UQL `prepare=true` returns server-owned plan/query pins for the exact executable statement. This still does not prove a served host exchange or source approval. |
| Owner/sequence/expiry refusal | R722.3/R085 | `crates/medulla-server-ops/src/api/query_txn/foreign_read.rs::tests::owner_digest_and_sequence_are_checked_before_page_acceptance`; adjacent abort/expiry tests in the same module | Wrong owner, digest or sequence refuse before page acceptance. Expired or terminal handles do likewise. Served paths must revalidate durable approval. |
| Existing mapped-cell projection | R722.3/R722.5 | `crates/medulla-server-ops/src/api/query_txn/foreign_read.rs::tests::mapped_cells_use_existing_foreign_rows_residual_and_projection` | Existing `ForeignRows`/`ColumnPlan` keeps the approved `contract_iri` mapping and provenance through residual filtering and projection. Live connector rows and zero-copy store audit remain open. |

No row in this leaf promotes R722.3--R722.6. Record positive tests and refusal
cases against the same frozen source archive before composing this evidence.

## MEDULLA-FEDERATED-QUERY-R058.6 to R058.9 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Behavior | `MEDULLA-FEDERATED-QUERY-R058.6` | `tests/nl_query_certified_definitions.rs::mismatch_is_corrected_then_disclosed`: one feedback step, the certified terms in the result, and `NL_DEFINITION_CONFLICT` when the loop ends. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R058.7` | `tests/nl_query_certified_definitions.rs::recorded_correction_grounds_the_next_plan`: the approved pair is in the next planning context; no scope returns `NL_CORRECTION_UNAUTHORIZED`. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R058.8` | `tests/nl_query_bounded_steps.rs::follow_up_is_rewritten_and_disclosed`: the rewritten question is disclosed; without history the question is unchanged. |
| Behavior | `MEDULLA-FEDERATED-QUERY-R058.9` | `tests/nl_query_certified_definitions.rs::conflicting_sources_record_one_conflict`: one conflict fact; planning does not treat the term as certified. |

## Planned bindings, range 1

Each row below names the test that proves it as `path::name`. State `exists` means the test is in the tree today and the implementation lane adds the `spec:` annotation that binds it; state `planned` means the test is written by the implementation lane at exactly this path.

| Row | Test | State |
|---|---|---|
| MEDULLA-FEDERATED-QUERY-R001 | `crates/medulla-reasoner/src/owl/budget.rs::exact_budget_completes_and_one_less_is_exhausted` | exists |
| MEDULLA-FEDERATED-QUERY-R002 | `crates/medulla-types/src/wire_query_core.rs::kernel_row_schema_carries_no_rejection_reason_field` | exists |
| MEDULLA-FEDERATED-QUERY-R003 | `crates/medulla-reasoner/src/owl/budget.rs::exact_budget_completes_and_one_less_is_exhausted` | exists |
| MEDULLA-FEDERATED-QUERY-R004 | `crates/medulla-h-retrieve/src/reasoning_projection.rs::a_dead_lettered_claim_rebuilds_the_projection_before_further_reads` | exists |
| MEDULLA-FEDERATED-QUERY-R007 | `crates/medulla-ann-codes/src/semantic_index_service/reconciliation_regression.rs::missing_sources_become_tombstones_only_after_complete_snapshot_proof` | exists |
| MEDULLA-FEDERATED-QUERY-R009 | `crates/medulla-cypher/src/edge_index/tests.rs::the_lifecycle_is_visible_in_the_index_manager_and_a_drop_is_fenced` | exists |
| MEDULLA-FEDERATED-QUERY-R010 | `crates/medulla-h-admin/src/graph_schema/world_model_tests.rs::full_corpus_stays_within_the_recorded_reasoning_budgets` | exists |
| MEDULLA-FEDERATED-QUERY-R011 | `crates/medulla-server-api/src/schema_compose.rs::shipped_core_corpus_is_coherent_under_the_el_rl_reasoner` | exists |
| MEDULLA-FEDERATED-QUERY-R012 | `crates/medulla-h-admin/src/graph_schema/world_model_tests.rs::a_taxon_is_never_an_organism` | exists |
| MEDULLA-FEDERATED-QUERY-R013 | `crates/medulla-server-served-tests/src/tests/foreign_tenancy.rs::foreign_source_resolves_only_for_the_registering_principal` | exists |
| MEDULLA-FEDERATED-QUERY-R014 | `crates/medulla-server-ops/src/api/query_txn/foreign_share.rs::a_shared_source_needs_an_exact_grant_and_revocation_stops_use` | exists |
| MEDULLA-FEDERATED-QUERY-R015 | `crates/medulla-sql/src/sql/read_only.rs::only_a_single_read_statement_passes` | exists |
| MEDULLA-FEDERATED-QUERY-R016 | `crates/medulla-plan/src/uql/tests/legacy.rs::as_of_rejects_iso_date_and_cypher_shaped_text` | exists |
| MEDULLA-FEDERATED-QUERY-R017 | `crates/medulla-core/tests/dependency_scoped_edges.rs::typed_traversal_survives_an_edge_of_another_type` | exists |
| MEDULLA-FEDERATED-QUERY-R018 | `crates/medulla-h-query/src/query/foreign_watermarks.rs::a_stale_watermark_refuses_caching` | exists |
| MEDULLA-FEDERATED-QUERY-R019 | `crates/medulla-finance-market/src/market/record_tests.rs::a_revision_appends_revision_records_and_never_rewrites` | exists |
| MEDULLA-FEDERATED-QUERY-R020 | `crates/medulla-capabilities/tests/contract_registry.rs::retired_methods_never_reappear` | exists |
| MEDULLA-FEDERATED-QUERY-R021 | `crates/medulla-exec/src/pred_eval.rs::the_sql_leg_and_the_row_evaluator_keep_identical_rows` | exists |
| MEDULLA-FEDERATED-QUERY-R022.1 | `crates/medulla-exec/src/exec.rs::scan_matches_every_label_field_writers_store` | exists |
| MEDULLA-FEDERATED-QUERY-R023 | `crates/medulla-plan/src/uql/tests/contract.rs::every_grammar_example_parses_or_names_its_missing_feature` | exists |
| MEDULLA-FEDERATED-QUERY-R024 | `crates/medulla-plan/src/uql/tests/legacy.rs::outbound_traversal_spellings_compile_to_the_same_canonical_plan` | exists |
| MEDULLA-FEDERATED-QUERY-R025 | `tests/test_uql_client_result.py::test_uql_binds_hostile_value_from_python_client_through_engine` | exists |
| MEDULLA-FEDERATED-QUERY-R026 | `crates/medulla-plan/src/uql/tests/contract.rs::every_uql_block_in_the_docs_parses` | exists |
| MEDULLA-FEDERATED-QUERY-R027 | `crates/medulla-plan/src/uql/tests/serve.rs::a_program_explains_node_by_node` | exists |
| MEDULLA-FEDERATED-QUERY-R029 | `crates/medulla-plan/src/uql/tests/scrubber.rs::the_temporal_scrubber_query_template_parses_under_the_current_grammar` | exists |
| MEDULLA-FEDERATED-QUERY-R030 | `crates/medulla-plan/src/tsdb_scan_tests.rs::tsscan_keeps_both_series_at_a_shared_timestamp` | exists |
| MEDULLA-FEDERATED-QUERY-R031 | `crates/medulla-finance/src/attribution/tests.rs::exact_matches_brute_force_permutations` | exists |
| MEDULLA-FEDERATED-QUERY-R033 | `crates/medulla-numeric/src/risk/barrier/tests.rs::closed_form_matches_bridge_corrected_monte_carlo` | exists |
| MEDULLA-FEDERATED-QUERY-R034 | `crates/medulla-numeric/src/series/motif_tests.rs::the_matrix_profile_equals_brute_force` | exists |
| MEDULLA-FEDERATED-QUERY-R035 | `crates/medulla-plan/src/text_tests.rs::filtered_rank_text_equals_the_brute_force_restricted_ranking` | exists |
| MEDULLA-FEDERATED-QUERY-R036 | `crates/medulla-server-store-tests/src/split_tests/restart_reopen_partial_materialization.rs::reads_refused_until_ready` | planned |
| MEDULLA-FEDERATED-QUERY-R037 | `crates/medulla-numeric/src/series/streaming_tests.rs::a_flat_window_reports_exactly_zero_spread_after_any_history` | exists |
| MEDULLA-FEDERATED-QUERY-R038.1 | `crates/medulla-federation/src/federation_opt/capability.rs::same_source_join_is_refused_without_an_explicit_proof` | exists |
| MEDULLA-FEDERATED-QUERY-R041 | `crates/medulla-federation/src/federation_opt/tests.rs::protocol_matrix_bounds_pushdown` | exists |
| MEDULLA-FEDERATED-QUERY-R042 | `crates/medulla-virtual-graph/src/obda/pushdown/tests.rs::the_semi_join_ships_only_anchor_keys` | exists |
| MEDULLA-FEDERATED-QUERY-R043 | `crates/medulla-federation/src/federation_ssrf.rs::sql_dsn_hosts_are_vetted_like_http_destinations` | exists |
| MEDULLA-FEDERATED-QUERY-R044 | `crates/medulla-plan/src/uql/tests/serve.rs::explain_has_redacted_remote_and_local_sections_without_running_source` | exists |
| MEDULLA-FEDERATED-QUERY-R045 | `crates/medulla-federation/src/federation_opt/columns.rs::unsupported_filter_stays_local_and_unexposed_column_fails` | exists |
| MEDULLA-FEDERATED-QUERY-R046 | `crates/medulla-virtual-graph/src/obda.rs::eh573_direct_ordered_slice_matches_materialized_results_and_ships_less` | exists |
| MEDULLA-FEDERATED-QUERY-R047 | `crates/medulla-federation/src/federation_opt/tests.rs::parallel_batches_share_a_source_cap_across_queries` | exists |
| MEDULLA-FEDERATED-QUERY-R048.1 | `crates/medulla-federation/src/federation_stats.rs::rejects_statistic_without_provenance` | exists |
| MEDULLA-FEDERATED-QUERY-R049 | `crates/medulla-federation/src/federation_opt/cache.rs::hit_miss_watermark_expiry_and_owner_are_isolated` | exists |
| MEDULLA-FEDERATED-QUERY-R050 | `crates/medulla-rdf/src/sparql/service_tests.rs::a_bind_join_ships_the_distinct_local_keys_and_matches_the_full_fetch` | exists |
| MEDULLA-FEDERATED-QUERY-R051 | `crates/medulla-sql-tests/tests/iceberg_federation_scan.rs::a_partitioned_table_is_scanned_through_the_provider_with_files_pruned` | exists |
| MEDULLA-FEDERATED-QUERY-R052 | `crates/medulla-plan/src/engine_peer_tests.rs::a_verified_pushdown_narrows_the_transfer_without_changing_the_answer` | exists |
| MEDULLA-FEDERATED-QUERY-R054 | `crates/medulla-h-sparql/src/federation/mod.rs::service_and_peer_fanout_refuse_one_disallowed_target_through_the_one_guard` | exists |
| MEDULLA-FEDERATED-QUERY-R055 | `crates/medulla-rdf/src/sparql/multivalue_tests.rs::both_values_of_one_property_bind` | exists |
| MEDULLA-FEDERATED-QUERY-R056 | `crates/medulla-mining/src/mining/causal_impact.rs::two_sided_p_keeps_relative_accuracy_in_the_far_tail` | exists |
| MEDULLA-FEDERATED-QUERY-R057 | `tests/test_federated_query_file_size.py::test_sql_provider_module_is_within_file_size_limit` | exists |
| MEDULLA-FEDERATED-QUERY-R058.1 | `crates/medulla-nlq/src/nl.rs::r058_plan_only_returns_candidate_without_executing` | exists |
| MEDULLA-FEDERATED-QUERY-R058.2.1 | `crates/medulla-nlq/src/nl.rs::r058_2_1_budget_check_refuses_over_budget_text_before_parsing` | exists |
| MEDULLA-FEDERATED-QUERY-R058.2.2.1 | `crates/medulla-server-served-tests/src/tests/nl_explain.rs::nl_query_explain_returns_planner_uql_and_nl_query_rows` | exists |
| MEDULLA-FEDERATED-QUERY-R058.2.2.2 | `crates/medulla-server-served-tests/src/tests/nl_explain.rs::nl_query_plan_returns_pins_without_executing_rows` | exists |
| MEDULLA-FEDERATED-QUERY-R058.3.1 | `crates/medulla-h-query/src/query/nl.rs::nl_query_handler_never_touches_result_cache` | exists |
| MEDULLA-FEDERATED-QUERY-R058.3.2 | `crates/medulla-nlq/src/nl_regressions.rs::nl_entry_points_refuse_mutating_statements` | exists |
| MEDULLA-FEDERATED-QUERY-R059 | `crates/medulla-sql/src/sql/ann_authority_tests.rs::a_restart_serves_the_persisted_generation_without_a_rebuild` | exists |
| MEDULLA-FEDERATED-QUERY-R060 | `tests/leiden_community_detection_served.rs::served_cpm_differs_from_modularity_on_the_same_graph` | exists |
| MEDULLA-FEDERATED-QUERY-R061 | `crates/medulla-plan/src/uql/tests/contract.rs::dispatch_table_and_grammar_name_the_same_keywords` | exists |
| MEDULLA-FEDERATED-QUERY-R062 | `crates/medulla-plan/tests/result_stream_memory.rs::million_row_scan_streams_with_flat_memory` | exists |
| MEDULLA-FEDERATED-QUERY-R063 | `crates/medulla-graph-algos/src/graph_algos/leiden/stability.rs::highest_degree_names_are_stable_across_repeated_runs` | exists |
| MEDULLA-FEDERATED-QUERY-R064 | `crates/medulla-plan/src/uql/tests/lexing.rs::scientific_notation_and_exact_text` | exists |
| MEDULLA-FEDERATED-QUERY-R065 | `tests/uql_dag_served.rs::served_dag_explain_and_profile_report_each_node` | exists |
| MEDULLA-FEDERATED-QUERY-R066 | `tests/test_federated_query_file_size.py::test_sparql_evaluator_modules_are_within_file_size_limit` | exists |
| MEDULLA-FEDERATED-QUERY-R067 | `crates/medulla-plan/src/uql/tests/serve.rs::two_scoring_stages_keep_both_channels` | exists |
| MEDULLA-FEDERATED-QUERY-R068 | `crates/medulla-plan/src/uql/tests/annotations.rs::a_sparql_source_proves_each_row_with_its_witness` | exists |
| MEDULLA-FEDERATED-QUERY-R069 | `crates/medulla-federation/src/federation_opt/sql.rs::a_statement_that_swallows_the_key_predicate_is_refused` | exists |
| MEDULLA-FEDERATED-QUERY-R070 | `crates/medulla-exec/src/budget.rs::budgets_refuse_with_a_typed_code` | exists |
| MEDULLA-FEDERATED-QUERY-R071 | `crates/medulla-federation/src/federation_opt/http.rs::keys_are_percent_encoded_and_comma_joined` | exists |
| MEDULLA-FEDERATED-QUERY-R072.1 | `crates/medulla-virtual-graph/src/source_selection/tests.rs::direct_mapping_is_covered_with_source_generation_premise` | exists |
| MEDULLA-FEDERATED-QUERY-R073.1 | `crates/medulla-federation/src/federation.rs::each_new_source_kind_wire_round_trips_with_its_declared_capability` | exists |
| MEDULLA-FEDERATED-QUERY-R073.2 | `crates/medulla-federation/src/federation_opt/tests.rs::protocol_matrix_bounds_pushdown` | exists |
| MEDULLA-FEDERATED-QUERY-R077.1 | `crates/medulla-sql/src/sql/ann.rs::r0771_column_vector_binding_round_trips_and_rejects_zero_dimension` | exists |
| MEDULLA-FEDERATED-QUERY-R077.2 | `tests/retrieval/test_column_vector_served.py::test_bind_text_refuse_integer` | planned |
| MEDULLA-FEDERATED-QUERY-R077.3 | `tests/retrieval/test_column_vector_served.py::test_write_queues_one_embed_per_bound_row` | planned |
| MEDULLA-FEDERATED-QUERY-R077.5 | `tests/retrieval/test_column_vector_served.py::test_bound_column_nearest_neighbour` | planned |
| MEDULLA-FEDERATED-QUERY-R078.1 | `crates/medulla-table-store/src/plan_scope.rs::plan_scope_round_trips_and_rejects_empty_tenant` | exists |
| MEDULLA-FEDERATED-QUERY-R078.2 | `crates/medulla-table-store/src/plan_scope.rs::three_leaf_plan_all_scoped` | planned |
| MEDULLA-FEDERATED-QUERY-R078.3 | `tests/plan_scope_tenant.rs::planned_statement_executes_only_with_predicate` | planned |
| MEDULLA-FEDERATED-QUERY-R078.4 | `tests/plan_scope_tenant.rs::unverified_tenant_refused_zero_reads` | planned |
| MEDULLA-FEDERATED-QUERY-R078.5 | `tests/plan_scope_tenant.rs::trace_and_explain_show_scope` | planned |
| MEDULLA-FEDERATED-QUERY-R078.6 | `tests/plan_scope_tenant.rs::only_own_rows_return` | planned |
| MEDULLA-FEDERATED-QUERY-R079.1 | `crates/medulla-catalog-types/src/lake_catalog.rs::manifest_publication_round_trips_every_field` | exists |
| MEDULLA-FEDERATED-QUERY-R079.3 | `tests/lake_catalog_schema_evolution.rs::graph_mutation_reaches_catalog_read` | planned |
| MEDULLA-FEDERATED-QUERY-R079.4 | `tests/lake_catalog_schema_evolution.rs::every_refusal_keeps_visible_versions` | planned |
| MEDULLA-FEDERATED-QUERY-R079.5 | `tests/lake_catalog_schema_evolution.rs::evidence_correlates_commit_publication_load` | planned |
| MEDULLA-FEDERATED-QUERY-R079.6 | `tests/lake/test_pyiceberg_reader.py::test_reader_sees_committed_rows` | planned |
| MEDULLA-FEDERATED-QUERY-R080.1 | `crates/medulla-lake/src/catalog.rs::every_registered_entry_is_read_only` | exists |
| MEDULLA-FEDERATED-QUERY-R080.2 | `crates/medulla-lake/src/catalog.rs::refuse_projection_write_classifies_each_operation` | exists |
| MEDULLA-FEDERATED-QUERY-R080.3 | `crates/medulla-server-lake/src/lake/rest/mod.rs::every_write_route_is_refused_as_projection_read_only` | planned |
| MEDULLA-FEDERATED-QUERY-R080.4 | `crates/medulla-server-lake/src/lake/rest/mod.rs::projection_write_returns_405_and_leaves_graph_version` | planned |
| MEDULLA-FEDERATED-QUERY-R080.5 | `crates/medulla-server-lake/src/lake/rest/mod.rs::refusal_and_read_leave_audit_records` | planned |
| MEDULLA-FEDERATED-QUERY-R080.6 | `tests/lake_projection_read_only.rs::projection_writes_are_refused_and_graph_writes_appear` | planned |
| MEDULLA-FEDERATED-QUERY-R081.1 | `crates/medulla-result-types/src/retrieval_verdict.rs::verdict_round_trips_and_rejects_unknown` | exists |
| MEDULLA-FEDERATED-QUERY-R081.2 | `crates/medulla-retrieval/src/retrieval_verdict_tests.rs::repair_order_ends_at_round_limit` | exists |
| MEDULLA-FEDERATED-QUERY-R081.3 | `crates/medulla-plan/src/retrieval_round.rs::run_round_returns_verdict_for_rule_plan` | planned |
| MEDULLA-FEDERATED-QUERY-R081.4 | `crates/medulla-retrieval/src/retrieval_verdict_tests.rs::strict_insufficient_abstains` | exists |
| MEDULLA-FEDERATED-QUERY-R081.5 | `tests/retrieval_round_served.rs::sufficiency_premise_lists_all_six_fields` | planned |
| MEDULLA-FEDERATED-QUERY-R081.6 | `tests/retrieval/test_sufficiency_served.py::test_insufficient_returns_boost_k` | planned |
| MEDULLA-FEDERATED-QUERY-R081.7.1 | `crates/medulla-result-types/src/retrieval_verdict.rs::selection_contract_round_trips_and_refuses_invalid_inputs` | exists |
| MEDULLA-FEDERATED-QUERY-R073.2 | `tests/query/test_virtual_mapping_pushdown_served.py::test_stream_pushes_time_range_only` | planned |
| MEDULLA-FEDERATED-QUERY-R078.2 | `tests/plan_scope_tenant.rs::only_own_rows_return` | planned |
| MEDULLA-FEDERATED-QUERY-R079.6 | `tests/lake_manifest_interoperability.rs::external_reader_loads_v2_manifests` | planned |
| MEDULLA-FEDERATED-QUERY-R081.3 | `tests/retrieval_round_served.rs::served_round_returns_verdict_beside_rows` | planned |
| MEDULLA-FEDERATED-QUERY-R081.6 | `tests/retrieval_round_served.rs::served_round_returns_verdict_beside_rows` | planned |

## Planned bindings, range 2

Each test below is planned: its path is the post-split location and its name is exact. A row binds it with a `// spec:` comment when it lands.

| Row | Planned test | Level |
|---|---|---|
| `MEDULLA-FEDERATED-QUERY-R081.7.3` | `tests/retrieval_round_served.rs::adaptive_selection_rescues_low_embedding_chunk` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.7.5` | `tests/retrieval_round_served.rs::selection_receipt_and_benchmark_gate` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.7.6` | `tests/retrieval/test_adaptive_selection_served.py::test_rescue_and_dual_low_refusal` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.7.6` | `tests/retrieval_round_served.rs::adaptive_selection_rescues_low_embedding_chunk` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.8` | `tests/retrieval_round_served.rs::feedback_continuation_is_validated_and_bounded` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.8.1` | `crates/medulla-types/src/retrieval_round.rs::tests::round_feedback_round_trips_and_clamps_limits` | L1 |
| `MEDULLA-FEDERATED-QUERY-R081.8.1` | `tests/retrieval_round_served.rs::feedback_continuation_is_validated_and_bounded` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.8.2` | `crates/medulla-plan/src/retrieval_feedback.rs::tests::terms_reinforce_suppress_and_critique_cannot_promote` | L1 |
| `MEDULLA-FEDERATED-QUERY-R081.8.3` | `tests/retrieval_round_served.rs::feedback_continuation_is_validated_and_bounded` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.8.4` | `tests/retrieval_round_served.rs::feedback_refusals_dispatch_nothing` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.8.5` | `tests/retrieval_round_served.rs::feedback_premise_links_next_query_to_feedback` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.8.6` | `tests/retrieval/test_feedback_served.py::test_uncalibrated_continuation_precedence` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.8.6` | `tests/retrieval_round_served.rs::feedback_continuation_is_validated_and_bounded` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9` | `tests/retrieval_round_served.rs::community_descent_returns_entities_with_receipt` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9` | `tests/retrieval_round_served.rs::section_traversal_returns_spans_with_paths` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.3` | `tests/retrieval_round_served.rs::community_descent_returns_entities_with_receipt` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.5` | `tests/retrieval_round_served.rs::hierarchy_premise_accounts_for_every_stage` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.6` | `tests/retrieval/test_hierarchy_served.py::test_descent_and_budget_refusals` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.9.6` | `tests/retrieval_round_served.rs::community_descent_returns_entities_with_receipt` | L3 |
| `MEDULLA-FEDERATED-QUERY-R082` | `tests/retrieval_round_served.rs::stop_precedence_follows_the_declared_order` | L3 |
| `MEDULLA-FEDERATED-QUERY-R082.1` | `crates/medulla-result-types/src/retrieval_round.rs::tests::stop_request_round_trips_and_caller_cannot_raise_limits` | L1 |
| `MEDULLA-FEDERATED-QUERY-R082.2` | `crates/medulla-plan/src/retrieval_stop.rs::tests::repeat_and_margin_stops` | L1 |
| `MEDULLA-FEDERATED-QUERY-R082.3` | `tests/retrieval_round_served.rs::stop_precedence_follows_the_declared_order` | L3 |
| `MEDULLA-FEDERATED-QUERY-R082.4` | `tests/retrieval_round_served.rs::uncalibrated_rounds_keep_the_refusal_marker` | L3 |
| `MEDULLA-FEDERATED-QUERY-R082.5` | `tests/retrieval_round_served.rs::stop_premise_lists_margin_and_reason` | L3 |
| `MEDULLA-FEDERATED-QUERY-R082.6` | `tests/retrieval/test_stop_rule_served.py::test_repeat_with_margin_stops` | L4 |
| `MEDULLA-FEDERATED-QUERY-R082.6` | `tests/retrieval/test_feedback_served.py::test_uncalibrated_continuation_precedence` | L4 |
| `MEDULLA-FEDERATED-QUERY-R083` | `tests/retrieval_round_served.rs::support_precedes_confidence_and_small_gap_abstains` | L3 |
| `MEDULLA-FEDERATED-QUERY-R083.1` | `crates/medulla-result-types/src/retrieval_arbitration.rs::tests::verdict_carries_both_candidates_and_support` | L1 |
| `MEDULLA-FEDERATED-QUERY-R083.2` | `crates/medulla-plan/src/retrieval_arbitration.rs::tests::support_precedence_precedes_confidence_margin` | L1 |
| `MEDULLA-FEDERATED-QUERY-R083.3` | `tests/retrieval_round_served.rs::arbitration_part_returns_verdict_or_none` | L3 |
| `MEDULLA-FEDERATED-QUERY-R083.4` | `tests/retrieval_round_served.rs::abstain_returns_both_candidates` | L3 |
| `MEDULLA-FEDERATED-QUERY-R083.5` | `tests/retrieval_round_served.rs::arbitration_premise_lists_both_origins` | L3 |
| `MEDULLA-FEDERATED-QUERY-R083.6` | `tests/retrieval/test_arbitration_served.py::test_small_gap_abstains_with_both` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1` | `tests/retrieval/test_multimodal_served.py::test_sixteen_modality_pairs` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1.1` | `crates/medulla-index-types/src/embedding.rs::tests::modality_bindings_round_trip` | L1 |
| `MEDULLA-FEDERATED-QUERY-R039.1.1` | `tests/retrieval/test_multimodal_served.py::test_sixteen_modality_pairs` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1.2` | `crates/medulla-exec/src/exec/vector_rank.rs::tests::sixteen_pairs_match_oracle` | L2 |
| `MEDULLA-FEDERATED-QUERY-R039.1.2` | `tests/retrieval/test_multimodal_served.py::test_sixteen_modality_pairs` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1.3` | `tests/retrieval/test_multimodal_served.py::test_registry_encoders_reach_index` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1.4` | `tests/retrieval/test_multimodal_served.py::test_missing_embedding_shortfall_and_refusals` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1.5` | `tests/retrieval/test_multimodal_served.py::test_receipt_matches_model_calls` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.1.6` | `tests/retrieval/test_multimodal_served.py::test_sixteen_modality_pairs` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.1` | `tests/retrieval/test_constraints_served.py::test_constraints_before_ranking` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.1.1` | `crates/medulla-types/src/wire_query_core.rs::tests::retrieval_constraints_round_trip_and_refuse_unknown_operator` | L1 |
| `MEDULLA-FEDERATED-QUERY-R040.1.2` | `crates/medulla-exec/src/exec/constraints.rs::tests::failed_constraint_excludes_top_candidate` | L2 |
| `MEDULLA-FEDERATED-QUERY-R040.1.2` | `tests/retrieval/test_constraints_served.py::test_constraints_before_ranking` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.1.3` | `tests/retrieval/test_constraints_served.py::test_optimized_and_unoptimized_plans_agree` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.1.4` | `tests/retrieval/test_constraints_served.py::test_refusals_rank_nothing` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.1.5` | `tests/retrieval/test_constraints_served.py::test_excluded_never_reappears` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.1.6` | `tests/retrieval/test_constraints_served.py::test_constraints_before_ranking` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7` | `tests/retrieval_round_served.rs::section_traversal_returns_spans_with_paths` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.1` | `crates/medulla-result-types/src/retrieval_verdict.rs::tests::section_tree_validation_rejects_cycles_and_bad_spans` | L1 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.1` | `tests/retrieval_round_served.rs::section_traversal_returns_spans_with_paths` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.2` | `crates/medulla-retrieval/src/leanrag.rs::tests::vector_free_matches_tree_oracle` | L1 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.3` | `tests/retrieval_round_served.rs::section_traversal_returns_spans_with_paths` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.4` | `tests/retrieval_round_served.rs::malformed_trees_and_limits_refuse` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.5` | `tests/retrieval_round_served.rs::section_receipt_reconstructs_all_paths` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.6` | `tests/retrieval/test_section_tree_served.py::test_descent_three_granularities_zero_inference` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.9.7.6` | `tests/retrieval_round_served.rs::section_traversal_returns_spans_with_paths` | L3 |
| `MEDULLA-FEDERATED-QUERY-R039.2` | `tests/retrieval/test_chunk_encoding_served.py::test_chunked_document_one_vector` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.1` | `crates/medulla-index-types/src/embedding.rs::tests::document_encoding_policy_round_trips_and_refuses_invalid` | L1 |
| `MEDULLA-FEDERATED-QUERY-R039.2.2` | `crates/medulla-core/src/compute/chunk_windows.rs::tests::golden_boundaries` | L1 |
| `MEDULLA-FEDERATED-QUERY-R039.2.2` | `tests/retrieval/test_chunk_encoding_served.py::test_chunked_document_one_vector` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.3` | `crates/medulla-core/src/compute/chunk_windows.rs::tests::one_call_per_chunk_no_shared_state` | L2 |
| `MEDULLA-FEDERATED-QUERY-R039.2.3` | `tests/retrieval/test_chunk_encoding_served.py::test_chunked_document_one_vector` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.4` | `crates/medulla-core/src/compute/chunk_windows.rs::tests::mean_matches_numeric_oracle` | L1 |
| `MEDULLA-FEDERATED-QUERY-R039.2.4` | `tests/retrieval/test_chunk_encoding_served.py::test_chunked_document_one_vector` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.5` | `tests/retrieval/test_chunk_encoding_served.py::test_ingest_and_query_share_policy_generation` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.6` | `tests/retrieval/test_chunk_encoding_served.py::test_failed_chunk_keeps_prior_generation` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.7` | `tests/retrieval/test_chunk_encoding_served.py::test_receipt_accounts_every_chunk` | L4 |
| `MEDULLA-FEDERATED-QUERY-R039.2.8` | `tests/retrieval/test_chunk_encoding_served.py::test_chunked_document_one_vector` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6` | `tests/retrieval/test_column_advice_served.py::test_workload_changes_advice` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6.1` | `crates/medulla-table-store/src/tables/embedding_binding.rs::tests::advice_round_trip` | L1 |
| `MEDULLA-FEDERATED-QUERY-R077.6.1` | `tests/retrieval/test_column_advice_served.py::test_workload_changes_advice` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6.2` | `crates/medulla-table-store/src/tables/column_advice.rs::tests::traffic_changes_rank_not_statistics` | L2 |
| `MEDULLA-FEDERATED-QUERY-R077.6.2` | `tests/retrieval/test_column_advice_served.py::test_workload_changes_advice` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6.3` | `tests/retrieval/test_column_advice_served.py::test_advice_relation_reads_production_observations` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6.4` | `tests/retrieval/test_column_advice_served.py::test_unembeddable_types_refuse_without_side_effects` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6.5` | `tests/retrieval/test_column_advice_served.py::test_receipt_matches_observations_binding_unchanged` | L4 |
| `MEDULLA-FEDERATED-QUERY-R077.6.6` | `tests/retrieval/test_column_advice_served.py::test_workload_changes_advice` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2` | `tests/retrieval/test_topic_fusion_served.py::test_two_backends_interchange` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.1` | `crates/medulla-exec/src/exec/topic.rs::tests::two_backends_one_result_contract` | L1 |
| `MEDULLA-FEDERATED-QUERY-R040.2.1` | `tests/retrieval/test_topic_fusion_served.py::test_two_backends_interchange` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.2` | `crates/medulla-exec/src/exec/topic.rs::tests::order_matches_oracle_w0_w1_mid` | L2 |
| `MEDULLA-FEDERATED-QUERY-R040.2.2` | `tests/retrieval/test_topic_fusion_served.py::test_two_backends_interchange` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.3` | `tests/retrieval/test_topic_fusion_served.py::test_dense_and_topic_both_exercised` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.4` | `tests/retrieval/test_topic_fusion_served.py::test_incompatible_topics_refuse_before_fusion` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.5` | `tests/retrieval/test_topic_fusion_served.py::test_receipt_reproduces_final_score` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.6` | `tests/retrieval/test_topic_fusion_served.py::test_two_backends_interchange` | L4 |
| `MEDULLA-FEDERATED-QUERY-R084` | `tests/shex_served.rs::served_shex_contract_journey` | L3 |
| `MEDULLA-FEDERATED-QUERY-R084.1` | `tests/shex_served.rs::wire_report_preserves_shape_map` | L3 |
| `MEDULLA-FEDERATED-QUERY-R084.3` | `tests/shex_served.rs::bounded_constraints_report_results` | L3 |
| `MEDULLA-FEDERATED-QUERY-R084.5` | `tests/shex_served.rs::real_dispatch_reaches_shex_validator` | L3 |
| `MEDULLA-FEDERATED-QUERY-R084.6` | `tests/shex_served.rs::parse_and_dispatch_failures_are_distinct` | L3 |
| `MEDULLA-FEDERATED-QUERY-R084.7` | `tests/shex_served.rs::live_validation_obeys_read_authority` | L3 |
| `MEDULLA-FEDERATED-QUERY-R084.8` | `tests/shex_served.rs::served_shex_contract_journey` | L3 |
| `MEDULLA-FEDERATED-QUERY-R085` | `tests/source_query_approval_served.rs::approval_to_verification_journey` | L3 |
| `MEDULLA-FEDERATED-QUERY-R085.2` | `crates/medulla-h-query/src/federation/source_query_tests.rs::approval_requires_authority_and_native_commit` | L3 |
| `MEDULLA-FEDERATED-QUERY-R085.3` | `crates/medulla-h-query/src/federation/source_query_tests.rs::changed_or_revoked_approval_refuses` | L3 |
| `MEDULLA-FEDERATED-QUERY-R085.4` | `crates/medulla-federation/src/federation_tests.rs::approved_source_query_has_no_write_effects` | L1 |
| `MEDULLA-FEDERATED-QUERY-R085.5` | `tests/source_query_approval_served.rs::real_dispatch_issues_and_verifies_approval` | L3 |
| `MEDULLA-FEDERATED-QUERY-R085.6` | `crates/medulla-h-query/src/federation/source_query_tests.rs::limits_clamp_to_tenant_ceiling` | L3 |
| `MEDULLA-FEDERATED-QUERY-R085.7` | `crates/medulla-result-types/src/source_query.rs::tests::errors_redact_literals_and_round_trip` | L1 |
| `MEDULLA-FEDERATED-QUERY-R085.8` | `tests/source_query_approval_served.rs::approval_to_verification_journey` | L3 |
| `MEDULLA-FEDERATED-QUERY-R006.1` | `tests/query/test_temporal_rules_served.py::test_derived_fact_origin_readback` | L4 |
| `MEDULLA-FEDERATED-QUERY-R006.1` | `tests/forward_chain_served.rs::chain_covers_model_order_origin_refusal_restart_and_trace` | L3 |
| `MEDULLA-FEDERATED-QUERY-R040.5` | `tests/retrieval/test_low_storage_ann_served.py::test_selectivity_bound_and_readback` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.10` | `tests/retrieval_round_served.rs::clock_basis_and_second_pass_are_bounded` | L3 |
| `MEDULLA-FEDERATED-QUERY-R028.1` | `tests/query/test_knowledge_set_uql_served.py::test_frames_resolved_missing_stale` | L4 |
| `MEDULLA-FEDERATED-QUERY-R005.1` | `tests/query/test_validate_shape_served.py::test_violation_matrix_five_kinds` | L4 |
| `MEDULLA-FEDERATED-QUERY-R078.7` | `tests/query/test_company_context_served.py::test_two_tenants_two_clearances` | L4 |
| `MEDULLA-FEDERATED-QUERY-R040.2.7` | `tests/retrieval/test_topic_fusion_served.py::test_document_responsibilities_change_order` | L4 |
| `MEDULLA-FEDERATED-QUERY-R081.8.100` | `crates/medulla-plan/src/retrieval_feedback.rs::tests::strategy_reuse_needs_matching_retriever_or_receipt` | L1 |
| `MEDULLA-FEDERATED-QUERY-R081.8.100` | `tests/retrieval_round_served.rs::transfer_receipt_gates_strategy_reuse` | L3 |
| `MEDULLA-FEDERATED-QUERY-R053.100` | `crates/medulla-federation/src/federation_cypher_unbound.rs::tests::key_quoting_fixtures` | L2 |
| `MEDULLA-FEDERATED-QUERY-R053.100` | `tests/query/test_foreign_kinds_served.py::test_cypher_keys_quoted` | L4 |
| `MEDULLA-FEDERATED-QUERY-R032.100` | `tests/query/test_propagate_served.py::test_historical_risk_both_directions` | L4 |
| `MEDULLA-FEDERATED-QUERY-R086` | `tests/graphlearn_nary_served.rs::nary_hypotheses_are_read_only_and_ordered` | L3 |
| `MEDULLA-FEDERATED-QUERY-R086.100` | `tests/graphlearn_nary_served.rs::nary_hypotheses_are_read_only_and_ordered` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.101` | `tests/retrieval_round_served.rs::compressed_state_rerank_is_order_independent` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.102` | `tests/retrieval_round_served.rs::distilled_head_activates_only_above_threshold` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.100` | `crates/medulla-decision-stats/src/sparse_select.rs::tests::matches_exhaustive_oracle_on_tiny_fixture` | L2 |
| `MEDULLA-FEDERATED-QUERY-R081.100` | `tests/retrieval_round_served.rs::joint_selection_prefers_diverse_pair` | L3 |
| `MEDULLA-FEDERATED-QUERY-R081.103` | `tests/viewport_served.rs::viewport_bins_are_bounded_and_pinned` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.3` | `tests/test_viz_client.py::test_render_round_trip` | L4 |
| `MEDULLA-FEDERATED-QUERY-R058.4` | `tests/nl_query_bounded_steps.rs::second_step_corrects_parse_error` | L3 |
| `MEDULLA-FEDERATED-QUERY-R058.4.1` | `crates/medulla-nlq/src/nl/context.rs::tests::context_round_trips` | L1 |
| `MEDULLA-FEDERATED-QUERY-R058.4.1` | `tests/nl_query_bounded_steps.rs::second_step_corrects_parse_error` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.1` | `tests/viz_dispatch.rs::capability_matrix_is_reachable_over_the_wire` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.2` | `tests/viz_dispatch.rs::an_identical_repeat_render_request_is_served_from_the_cache` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.2` | `tests/viz_dispatch.rs::inline_columns_scatter_resolves_at_direct_tier` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.4` | `tests/viz_dispatch.rs::invalid_spec_json_is_a_typed_error_not_a_panic` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.4` | `tests/viz_dispatch.rs::omitting_dataset_against_an_unknown_dataset_ref_is_an_explicit_unavailable_error` | L3 |
| `MEDULLA-FEDERATED-QUERY-R087.6` | `tests/viz_dispatch.rs::served_render_supports_png_svg_pdf_and_refuses_unknown_dataset` | L3 |

## Planned bindings, range 3

Proving targets for rows `MEDULLA-FEDERATED-QUERY-R058.4.2` to `MEDULLA-FEDERATED-QUERY-R005.701`. Each path is where the test will live after the crate split; none is an execution receipt.

- **MEDULLA-FEDERATED-QUERY-R058.4.2:** `crates/medulla-nlq/src/nl/context.rs::tests::linking_dialect_and_one_correction` (planned). NL planning context: behavior. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R058.4.3:** `tests/nl_query_bounded_steps.rs::one_schema_read_per_request` (planned). NL planning context: wiring. A served NlQuery with a scripted planner asserts one schema read.
- **MEDULLA-FEDERATED-QUERY-R058.4.4:** `tests/nl_query_bounded_steps.rs::refusals_return_code_and_no_rows` (planned). NL planning context: refusal. Each case returns its code and no rows.
- **MEDULLA-FEDERATED-QUERY-R058.4.5:** `tests/nl_query_bounded_steps.rs::served_response_carries_steps_and_digest` (planned). NL planning context: evidence. It reads the step list and digest from the served response.
- **MEDULLA-FEDERATED-QUERY-R058.4.7:** `tests/nl_query_grounding.rs::unknown_identifier_returns_feedback_not_execution` (planned). Schema grounding of planner output (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R058.4.7.1:** `context_tests.rs::evidence_questions_are_grounded_or_refused_over_servicenow_data` (planned), `nl_explain.rs::nl_query_grounds_names_on_the_graph_schema_before_running` (planned). Snapshot graph and authorized foreign-source grounding. A table holds the 26 validation-run questions over ingested ServiceNow fixture data. Each row asserts grounded UQL and the expected rows. A question with no graph data asserts the typed refusal (). A served test refuses an unknown property with its reason and runs a repaired name ().
- **MEDULLA-FEDERATED-QUERY-R058.4.7.2:** `tests/nl_query_grounding.rs::source_native_identifiers_grounded` (planned). Source-native identifier grounding (rollup). The child acceptance criteria apply. Root journey test.
- **MEDULLA-FEDERATED-QUERY-R058.4.9:** `tests/nl_query_grounding.rs::inline_adapter_refused_named_source_grounds` (planned). Inline foreign adapter grounding. It asserts the refusal for an inline HTTP adapter and success for a registered source.
- **MEDULLA-FEDERATED-QUERY-R058.4.10:** `tests/nl_query_sql_served.rs::sql_dialect_plans_and_runs_a_select` (planned). Natural-language to SQL planner.
- **MEDULLA-FEDERATED-QUERY-R058.4.11:** `tests/nl_query_sparql_served.rs::sparql_dialect_plans_and_runs_a_select` (planned). Natural-language to SPARQL planner.
- **MEDULLA-FEDERATED-QUERY-R088:** `tests/uql_governed_summary.rs::governed_rows_carry_knowledge_and_header` (planned). UQL rows apply visibility before limits and carry a snapshot header (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R088.1:** `crates/medulla-types/src/wire_query_core.rs::tests::uql_header_round_trips_with_defaults` (planned). Model. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R088.2:** `tests/uql_governed_summary.rs::hidden_rows_do_not_displace_visible_rows` (planned). Behavior. Runs through Method::Uql with two principals and LIMIT 5.
- **MEDULLA-FEDERATED-QUERY-R088.3:** `tests/uql_governed_summary.rs::rows_and_header_share_one_snapshot` (planned). Wiring. Writes between two reads and asserts the header matches the rows.
- **MEDULLA-FEDERATED-QUERY-R088.6:** `tests/uql_governed_summary.rs::governed_rows_carry_knowledge_and_header` (planned). Hot-path proof. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R089.2:** `crates/medulla-h-graph/src/graph_ops/discover_quality.rs::tests::each_mode_fires_on_its_trigger` (planned). Behavior: failure modes. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R089.3:** `tests/discover_quality_verdict.rs::policy_node_overrides_defaults` (planned). Wiring and policy node. Reads the policy node and returns the verdict.
- **MEDULLA-FEDERATED-QUERY-R089.4:** `tests/discover_quality_verdict.rs::unknown_policy_refuses_and_failed_verdict_keeps_rows` (planned). Refusal. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R089.5:** `tests/discover_quality_verdict.rs::verdict_carries_revision_generation_and_thresholds` (planned). Evidence. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R089.6:** `tests/test_discover.py::test_discover_returns_verdict` (planned). Hot-path proof. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R089.7:** `tests/discover_quality_verdict.rs::drift_mode_names_adapter_and_shard_generations` (planned). Behavior: drift mode. Starts a firing monitor and serves Discover.
- **MEDULLA-FEDERATED-QUERY-R090.1:** `crates/medulla-types/src/ingestion_wire.rs::tests::discover_plan_round_trips` (planned). Model. Keeps every field and rejects hops = 3 and a 2,049-byte query_text. The Python client passes the same Rust vector.
- **MEDULLA-FEDERATED-QUERY-R090.2:** `crates/medulla-h-graph/src/graph_ops/discover_plan.rs::tests::fusion_decay_and_neutral_date` (planned). Behavior: arm fusion and recency. Checks fusion order, decay and the 1.0 cases. covers the served path.
- **MEDULLA-FEDERATED-QUERY-R090.3:** `tests/retrieval/test_discover_planning_served.py::test_deep_mode_calls_one_provider_per_request` (planned). Wiring: planner entry. Counts provider calls through a scripted provider and asserts exactly one.
- **MEDULLA-FEDERATED-QUERY-R090.4:** `tests/retrieval/test_discover_planning_served.py::test_discover_refusals` (planned). Refusal. Asserts each code, an empty result, and the degraded path with a failing provider.
- **MEDULLA-FEDERATED-QUERY-R090.5:** `tests/retrieval/test_discover_planning_served.py::test_discover_evidence_fields` (planned). Evidence. Reads every field and recomputes each digest.
- **MEDULLA-FEDERATED-QUERY-R091.3:** `tests/federation/test_plan_evolution_served.py::test_served_query_records_and_later_query_reads_kept_plan` (planned). Wiring. Runs two served queries and a tick.
- **MEDULLA-FEDERATED-QUERY-R091.4:** `tests/federation/test_plan_evolution_served.py::test_evolution_refusals` (planned). Refusal. Asserts each code and that the active plan stays the baseline.
- **MEDULLA-FEDERATED-QUERY-R091.5:** `tests/federation/test_plan_evolution_served.py::test_explain_names_kept_plan_and_receipt` (planned). Evidence. Reads the receipt through EXPLAIN.
- **MEDULLA-FEDERATED-QUERY-R607:** `tests/reasoning/test_topology_served.py::test_each_analytic_is_served` (planned). Topology analytics find partitions, analogies, synergy and hyperedge scores (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R607.1:** `crates/medulla-types/src/topology.rs::tests::result_round_trips_and_rejects_bad_ids` (planned). Model. Round-trips each type and rejects an empty id.
- **MEDULLA-FEDERATED-QUERY-R607.2:** `crates/medulla-graph-algos/src/graph_algos/spectral.rs::tests::planted_partition_is_recovered_and_ids_are_stable` (planned), `tests/reasoning/test_topology_served.py::test_spectral_and_mincut_served` (planned). Behavior: partitions. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R607.3:** `tests/reasoning/test_topology_served.py::test_method_is_registered_and_reaches_kernels` (planned). Wiring. Asserts the dispatch result equals the kernel output.
- **MEDULLA-FEDERATED-QUERY-R607.4:** `tests/reasoning/test_topology_served.py::test_topology_refusals` (planned). Refusal. Asserts each code and an empty result.
- **MEDULLA-FEDERATED-QUERY-R607.5:** `tests/reasoning/test_topology_served.py::test_evidence_fields_after_success_and_refusal` (planned). Evidence. Reads the field set and the captured spans.
- **MEDULLA-FEDERATED-QUERY-R607.6:** `tests/reasoning/test_topology_served.py::test_each_analytic_is_served` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-graph-algos --bench topology meets the thresholds.
- **MEDULLA-FEDERATED-QUERY-R608:** `tests/reasoning/test_rules_causal_served.py::test_bounded_rules_and_causal_verdict` (planned). Rule fixpoint bound and causal verification on the served reasoning methods (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R608.1:** `crates/medulla-types/src/rdf_report.rs::tests::rule_response_round_trips_with_bound_fields` (planned), `crates/medulla-types/src/causal_wire.rs::tests::verdict_round_trips` (planned). Model. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R608.2:** `crates/medulla-reasoner/src/rules.rs::tests::fixpoint_stops_at_bound_with_partial_facts` (planned), `crates/medulla-epistemic/src/causal.rs::tests::fit_verdict_at_boundary_nrmse` (planned). Behavior: fixpoint bound and causal fit. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R608.3:** `tests/reasoning/test_rules_causal_served.py::test_both_methods_reach_the_kernels` (planned). Wiring. Compares served output with the kernel output.
- **MEDULLA-FEDERATED-QUERY-R608.4:** `tests/reasoning/test_rules_causal_served.py::test_rule_bound_flag_and_causal_refusals` (planned). Refusal. Asserts the flag, both codes and zero writes.
- **MEDULLA-FEDERATED-QUERY-R608.5:** `tests/reasoning/test_rules_causal_served.py::test_evidence_after_success_and_bound` (planned). Evidence. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R608.6:** `tests/reasoning/test_rules_causal_served.py::test_bounded_rules_and_causal_verdict` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-reasoner --bench rule_fixpoint meets the key.
- **MEDULLA-FEDERATED-QUERY-R650:** `tests/retrieval/test_context_compile_served.py::test_bundle_end_to_end` (planned). Context bundle compilation under a certified token budget (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R650.1:** `crates/medulla-plan/src/context_bundle/tests.rs::bundle_types_round_trip_and_reject_unknown_reason` (planned). Model. The Python client passes the same Rust vector.
- **MEDULLA-FEDERATED-QUERY-R650.2:** `crates/medulla-plan/src/context_bundle/tests.rs::cut_select_and_exclusion_reasons` (planned), `crates/medulla-plan/src/context_bundle/tests.rs::result_is_deterministic_across_thread_counts` (planned). Behavior: fuse, cut and select. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R650.3:** `tests/retrieval/test_context_compile_served.py::test_method_is_served_and_no_second_implementation` (planned). Wiring and the new-method recipe. Asserts the served bundle equals compile output, and a source scan finds no other knapsack.
- **MEDULLA-FEDERATED-QUERY-R650.4:** `tests/retrieval/test_context_compile_served.py::test_context_refusals` (planned). Refusal. Asserts each code, an empty result and a cross-tenant negative case.
- **MEDULLA-FEDERATED-QUERY-R650.5:** `tests/retrieval/test_context_compile_served.py::test_evidence_matches_inputs` (planned). Evidence. Recomputes the key and compares every field. A second tenant gets a different key.
- **MEDULLA-FEDERATED-QUERY-R650.6:** `tests/retrieval/test_context_compile_served.py::test_bundle_end_to_end` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench context_bundle meets the key.
- **MEDULLA-FEDERATED-QUERY-R651:** `tests/retrieval/test_query_understanding_served.py::test_filters_intent_and_expansion` (planned). Query understanding and the one expansion port (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R651.1:** `crates/medulla-plan/src/query_understanding.rs::tests::types_round_trip_and_reject_unknown_kind` (planned). Model. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R651.2:** `crates/medulla-plan/src/query_understanding.rs::tests::filters_phrases_and_single_relation_intent` (planned). Behavior: model-free analysis. Covers positive and boundary cases.
- **MEDULLA-FEDERATED-QUERY-R651.3:** `tests/retrieval/test_query_understanding_served.py::test_one_port_serves_discover_and_feedback` (planned). Wiring: the expansion port. Counts provider calls from both callers.
- **MEDULLA-FEDERATED-QUERY-R651.4:** `tests/retrieval/test_query_understanding_served.py::test_expansion_refusals` (planned). Refusal. Asserts each code and that no retrieval ran.
- **MEDULLA-FEDERATED-QUERY-R651.5:** `tests/retrieval/test_query_understanding_served.py::test_analysis_and_expansion_evidence` (planned). Evidence. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R651.6:** `tests/retrieval/test_query_understanding_served.py::test_filters_intent_and_expansion` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench query_understanding meets the key.
- **MEDULLA-FEDERATED-QUERY-R652:** `tests/retrieval/test_rerank_served.py::test_rerank_calibrate_and_fallback` (planned). Reranking and calibrated scores (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R652.1:** `crates/medulla-plan/src/rerank.rs::tests::types_round_trip_and_reject_non_monotone_knots` (planned). Model. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R652.2:** `crates/medulla-plan/src/rerank.rs::tests::bm25_order_and_interpolation` (planned), `crates/medulla-plan/src/rerank.rs::tests::calibration_is_monotone` (planned). Behavior: lexical scoring and calibration. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R652.3:** `tests/retrieval/test_rerank_served.py::test_context_compile_reranks_before_boost` (planned). Wiring. Asserts the receipt order of stages.
- **MEDULLA-FEDERATED-QUERY-R652.4:** `tests/retrieval/test_rerank_served.py::test_rerank_refusals` (planned). Refusal. Asserts both codes and no items.
- **MEDULLA-FEDERATED-QUERY-R652.5:** `tests/retrieval/test_rerank_served.py::test_receipt_fields` (planned). Evidence. Reads and compares every field.
- **MEDULLA-FEDERATED-QUERY-R652.6:** `tests/retrieval/test_rerank_served.py::test_rerank_calibrate_and_fallback` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench rerank meets the key.
- **MEDULLA-FEDERATED-QUERY-R653:** `tests/retrieval/test_corpus_retrieve_served.py::test_each_mode_is_served` (planned). Corpus grep, entity neighbourhood, session history and answer lineage (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R653.1:** `crates/medulla-plan/src/corpus_retrieval.rs::tests::types_round_trip_and_reject_oversize_pattern` (planned). Model. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R653.2:** `crates/medulla-plan/src/corpus_retrieval.rs::tests::grep_order_limit_and_snippet` (planned). Behavior: grep. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R653.3:** `tests/retrieval/test_corpus_retrieve_served.py::test_method_reaches_each_mode` (planned). Wiring. Compares served output with the mode functions.
- **MEDULLA-FEDERATED-QUERY-R653.4:** `tests/retrieval/test_corpus_retrieve_served.py::test_corpus_refusals` (planned). Refusal. Asserts each code, no rows and a cross-tenant negative case.
- **MEDULLA-FEDERATED-QUERY-R653.5:** `tests/retrieval/test_corpus_retrieve_served.py::test_corpus_evidence` (planned). Evidence. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R653.6:** `tests/retrieval/test_corpus_retrieve_served.py::test_each_mode_is_served` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench corpus_retrieval meets the key.
- **MEDULLA-FEDERATED-QUERY-R654:** `tests/retrieval/test_code_context_served.py::test_code_context_and_diagnose` (planned). Code, operations and deployment context (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R654.1:** `crates/medulla-plan/src/code_context.rs::tests::types_round_trip_and_reject_unknown_stage` (planned). Model. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R654.2:** `crates/medulla-plan/src/code_context.rs::tests::depth_cap_and_hub_order` (planned). Behavior: code context. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R654.3:** `tests/retrieval/test_code_context_served.py::test_methods_reach_kernels` (planned). Wiring. Compares served output with the kernel.
- **MEDULLA-FEDERATED-QUERY-R654.4:** `tests/retrieval/test_code_context_served.py::test_ambiguous_degraded_and_refusals` (planned). Refusal and degraded answers. Asserts the status values, both codes and zero writes.
- **MEDULLA-FEDERATED-QUERY-R654.5:** `tests/retrieval/test_code_context_served.py::test_evidence_fields` (planned). Evidence. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R654.6:** `tests/retrieval/test_code_context_served.py::test_code_context_and_diagnose` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench code_context meets the key.
- **MEDULLA-FEDERATED-QUERY-R700:** `tests/reasoning/test_aif_dung_served.py::test_grounded_extension_matches_fixture` (planned). Argument maps with a Dung projection (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R700.1:** `crates/medulla-result-types/src/argument_map.rs::tests::interchange_golden_vector_round_trips_and_projects`. Model. Compile-time test: the types reject the invalid shapes named in .4; golden serialization vector round-trips.
- **MEDULLA-FEDERATED-QUERY-R700.2:** `crates/medulla-result-types/src/argument_map.rs::tests::grounded_and_preferred_extensions`, `crates/medulla-result-types/src/argument_map.rs::tests::grounded_is_linear_on_10_1000_100000_items` (planned). Behavior. Unit tests cover the stated complexity bound on fixtures of 10, 1,000 and 100,000 items.
- **MEDULLA-FEDERATED-QUERY-R700.3:** `tests/reasoning/test_aif_dung_served.py::test_put_get_round_trip_and_evaluate` (planned). Wiring. Stores the golden vector, exports it byte-equal and evaluates it.
- **MEDULLA-FEDERATED-QUERY-R700.4:** `tests/reasoning/test_aif_dung_served.py::test_dangling_edge_refused_and_budget_returns_gap` (planned). Refusal. Negative tests assert each refusal code and that durable state is unchanged.
- **MEDULLA-FEDERATED-QUERY-R700.5:** `tests/reasoning/test_aif_dung_served.py::test_digest_binds_map` (planned). Evidence. Recomputes the digest from ArgumentMapGet and re-derives the extension.
- **MEDULLA-FEDERATED-QUERY-R700.6:** `tests/reasoning/test_aif_dung_served.py::test_grounded_extension_matches_fixture` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-result-types --bench dung_grounded meets the key.
- **MEDULLA-FEDERATED-QUERY-R701:** `tests/query/test_live_artifact_served.py::test_failed_refresh_preserves_prior_data` (planned). Live artifacts re-derive from a query and keep the prior value on failure (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R701.1:** `crates/medulla-source-types/src/live_artifact.rs::tests::live_artifact_golden_vector_round_trips_and_bounds_hold`. Model. Compile-time test: the types reject the invalid shapes named in .4; golden serialization vector round-trips.
- **MEDULLA-FEDERATED-QUERY-R701.2:** `crates/medulla-source-types/src/live_artifact.rs::tests::refresh_is_compare_and_set_and_interpolation_is_one_pass`. Behavior. Unit tests cover the stated complexity bound on fixtures of 10, 1,000 and 100,000 items.
- **MEDULLA-FEDERATED-QUERY-R701.3:** `tests/query/test_live_artifact_served.py::test_put_refresh_get_and_scheduled_refresh` (planned). Wiring. Drives all three methods and one scheduler tick.
- **MEDULLA-FEDERATED-QUERY-R701.4:** `tests/query/test_live_artifact_served.py::test_executable_template_refused_and_failed_refresh_keeps_data` (planned). Refusal. Negative tests assert each refusal code and that durable state is unchanged.
- **MEDULLA-FEDERATED-QUERY-R701.5:** `tests/query/test_live_artifact_served.py::test_refresh_records_success_and_failure` (planned). Evidence. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R701.6:** `tests/query/test_live_artifact_served.py::test_failed_refresh_preserves_prior_data` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench live_artifact_refresh meets the key.
- **MEDULLA-FEDERATED-QUERY-R702:** `tests/query/test_search_task_synthesis_served.py::test_single_clue_task_is_rejected` (planned). Search-task synthesis with shortcut-risk diagnostics (rollup). The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R702.1:** `crates/medulla-work-types/src/search_task.rs::tests::records_round_trip_and_reject_invalid_shapes`. Model. Compile-time test: the types reject the invalid shapes named in .4; golden serialization vector round-trips.
- **MEDULLA-FEDERATED-QUERY-R702.2:** `crates/medulla-plan/src/search_task.rs::tests::detector_complexity_on_10_1000_100000_items` (planned). Behavior: detectors and refinement. Tests::each_detector_threshold_boundary.
- **MEDULLA-FEDERATED-QUERY-R702.3:** `tests/query/test_search_task_synthesis_served.py::test_task_feeds_ranking_evaluation` (planned). Wiring. Synthesizes a task and submits it to DecisionEval.
- **MEDULLA-FEDERATED-QUERY-R702.4:** `tests/query/test_search_task_synthesis_served.py::test_risky_task_refused_with_detector_name` (planned). Refusal. Negative tests assert each refusal code and that durable state is unchanged.
- **MEDULLA-FEDERATED-QUERY-R702.5:** `tests/query/test_search_task_synthesis_served.py::test_risk_report_lists_values` (planned). Evidence. Reads the report for one accepted and one rejected task.
- **MEDULLA-FEDERATED-QUERY-R702.6:** `tests/query/test_search_task_synthesis_served.py::test_single_clue_task_is_rejected` (planned). Hot-path proof. Served test passes. cargo bench -p medulla-plan --bench shortcut_detectors meets the key.
- **MEDULLA-FEDERATED-QUERY-R053.700:** `crates/medulla-federation/src/federation_spark.rs::tests::submission_id_idempotent_and_fenced` (planned), `tests/query/test_spark_trino_sources_served.py::test_resubmission_returns_same_submission` (planned). Trino and Spark sources with idempotent submission: model and behavior. The fixture and expected result are stated in the row's Test cell.
- **MEDULLA-FEDERATED-QUERY-R053.701:** `tests/query/test_spark_trino_sources_served.py::test_resubmission_returns_same_submission` (planned). Trino and Spark sources: refusal, evidence and hot-path proof. It asserts one id for two identical submissions, both refusals, and a cross-tenant negative.
- **MEDULLA-FEDERATED-QUERY-R005.700:** `crates/medulla-exec/src/exec/dispatch/shape_integrity.rs::tests::finding_per_shape_and_bound` (planned), `tests/query/test_graph_integrity_shapes_served.py::test_fatal_finding_blocks` (planned). Graph integrity validation as named shapes: model and behavior. It builds one violation per shape, asserts severity and the blocking flag, and asserts one visit per element.
- **MEDULLA-FEDERATED-QUERY-R005.701:** `tests/query/test_graph_integrity_shapes_served.py::test_fatal_finding_blocks` (planned). Graph integrity validation: refusal, evidence and hot-path proof. It asserts the blocking finding and its shape id. It asserts the gap result over a budget-exceeding graph. A second tenant sees nothing. The benchmark receipt meets the budget.

## Planned bindings, range 4

Each entry names the test the row binds once its code lands; the path is the post-split location.

| Kind | Requirement | Test |
|---|---|---|
| Planned | `MEDULLA-FEDERATED-QUERY-R008.1` | `tests/data_plane/test_catalog_search_served.py::test_nl_catalog_question_returns_search_hits` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703` | `tests/retrieval/test_access_check_served.py::test_denied_hit_never_enters_bundle` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703.3` | `tests/retrieval/test_access_check_served.py::test_denied_hit_never_enters_bundle` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703.4` | `tests/retrieval/test_access_check_served.py::test_missing_port_fails_closed` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703.5` | `tests/retrieval/test_access_check_served.py::test_drop_reasons_read_back` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703.6` | `cargo bench -p medulla-plan --bench access_check` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703.100` | `scripts/tests/test_bench_gate_p99.py::test_p99_over_ceiling_fails_and_missing_sample_fails` |
| Planned | `MEDULLA-FEDERATED-QUERY-R704` | `tests/retrieval/test_knowledge_scope_served.py::test_agent_sees_only_scoped_sources` |
| Planned | `MEDULLA-FEDERATED-QUERY-R704.3` | `tests/retrieval/test_knowledge_scope_served.py::test_agent_sees_only_scoped_sources` |
| Planned | `MEDULLA-FEDERATED-QUERY-R704.4` | `tests/retrieval/test_knowledge_scope_served.py::test_scope_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R704.5` | `tests/retrieval/test_knowledge_scope_served.py::test_scope_digest_recorded` |
| Planned | `MEDULLA-FEDERATED-QUERY-R704.6` | `cargo bench -p medulla-plan --bench knowledge_scope` |
| Planned | `MEDULLA-FEDERATED-QUERY-R705` | `tests/retrieval/test_feedback_boost_served.py::test_upvote_raises_rank` |
| Planned | `MEDULLA-FEDERATED-QUERY-R705.3` | `tests/retrieval/test_feedback_boost_served.py::test_upvote_raises_rank` |
| Planned | `MEDULLA-FEDERATED-QUERY-R705.4` | `tests/retrieval/test_feedback_boost_served.py::test_feedback_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R705.5` | `tests/retrieval/test_feedback_boost_served.py::test_boost_listed_per_hit` |
| Planned | `MEDULLA-FEDERATED-QUERY-R705.6` | `cargo bench -p medulla-plan --bench feedback_boost` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706` | `tests/federated/test_policy_mask_served.py::test_masked_column_over_sql_and_uql` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706.2` | `crates/medulla-plan/tests/policy_mask.rs::masks_and_filters_rewrite_the_plan` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706.3` | `tests/federated/test_policy_mask_served.py::test_masked_column_over_sql_and_uql` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706.4` | `tests/federated/test_policy_mask_served.py::test_mask_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706.5` | `tests/federated/test_policy_mask_served.py::test_explain_lists_policies` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706.6` | `cargo bench -p medulla-plan --bench policy_mask` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707` | `tests/federated/test_sparql_update_served.py::test_insert_then_select` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.1` | `crates/medulla-rdf/tests/sparql_update.rs::update_forms_parse` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.2` | `crates/medulla-rdf/tests/sparql_update.rs::multi_operation_update_is_atomic` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.2` | `tests/federated/test_sparql_update_served.py::test_graph_store_verbs` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.3` | `tests/federated/test_sparql_update_served.py::test_insert_then_select` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.4` | `tests/federated/test_sparql_update_served.py::test_update_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.5` | `tests/federated/test_sparql_update_served.py::test_update_receipt_reads_back` |
| Planned | `MEDULLA-FEDERATED-QUERY-R707.6` | `cargo bench -p medulla-rdf --bench sparql_update` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708` | `tests/federated/test_stored_query_served.py::test_put_then_call` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.2` | `crates/medulla-plan/tests/stored_query.rs::on_change_refreshes` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.3` | `tests/federated/test_stored_query_served.py::test_put_then_call` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.4` | `tests/federated/test_stored_query_served.py::test_stored_query_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.5` | `tests/federated/test_stored_query_served.py::test_staleness_reported` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.6` | `cargo bench -p medulla-plan --bench stored_query` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.7` | `tests/federated/test_stored_query_served.py::test_workspace_share_schedule_and_lint` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709` | `tests/federated/test_geosparql_served.py::test_within_query` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.1` | `crates/medulla-rdf/tests/geosparql.rs::functions_registered` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.2` | `crates/medulla-rdf/tests/geosparql.rs::topology_matches_kernel` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.3` | `tests/federated/test_geosparql_served.py::test_within_query` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.4` | `tests/federated/test_geosparql_served.py::test_geo_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.5` | `tests/federated/test_geosparql_served.py::test_explain_names_spatial_index` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.6` | `cargo bench -p medulla-rdf --bench geosparql` |
| Planned | `MEDULLA-FEDERATED-QUERY-R709.7` | `crates/medulla-rdf/tests/geosparql.rs::indexed_and_scanned_results_are_equal` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720` | `tests/uql_single_plan_all_modalities.rs::five_stage_kinds_one_plan` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.1` | `crates/medulla-exec/src/rowset.rs::tests::epistemic_fields_survive_traversal_and_vector` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.2` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::confidence_asof_decay` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.3` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::cluster_by_embedding_is_deterministic` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.4` | `tests/uql_single_plan_all_modalities.rs::program_runs_under_one_plan_id` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.5` | `tests/uql_single_plan_all_modalities.rs::decay_and_cluster_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.6` | `tests/uql_single_plan_all_modalities.rs::five_stage_kinds_one_plan` |
| Planned | `MEDULLA-FEDERATED-QUERY-R721` | `tests/advanced_crossmodal_roundtrip.rs::fresh_engine_answers_cross_modal_query` |
| Planned | `MEDULLA-FEDERATED-QUERY-R721.1` | `crates/medulla-modality/tests/rowset_views.rs::seven_modalities_implement_contract` |
| Planned | `MEDULLA-FEDERATED-QUERY-R721.2` | `crates/medulla-persistence/src/durable_stores.rs::tests::each_plan_modality_maps_to_one_store` |
| Planned | `MEDULLA-FEDERATED-QUERY-R721.3` | `crates/medulla-uql/src/parser/modality.rs::tests::unknown_modality_refuses_without_plan` |
| Planned | `MEDULLA-FEDERATED-QUERY-R721.4` | `tests/advanced_crossmodal_roundtrip.rs::fresh_engine_answers_cross_modal_query` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722` | `tests/federated/test_pack_source_served.py::test_uql_joins_native_fact_to_live_connector_rows_with_zero_copy` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.2` | `crates/medulla-plan/tests/pack_source.rs::held_serves_none_approved_serves_rows_and_changed_contract_returns_to_held` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.3` | `tests/federated/test_pack_source_served.py::test_import_registers_through_the_one_catalog` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.4` | `tests/federated/test_pack_source_served.py::test_secret_guard_held_and_unavailable_refuse_with_named_codes` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.5` | `tests/federated/test_pack_source_served.py::test_rows_carry_contract_provenance_and_nothing_is_copied` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.6` | `tests/federated/test_pack_source_served.py::test_uql_joins_native_fact_to_live_connector_rows_with_zero_copy` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.1` | `crates/medulla-ann/src/low_storage.rs::tests::manifest_round_trip_and_sum` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.1` | `tests/retrieval/test_low_storage_ann_served.py::test_selectivity_bound_and_readback` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.2` | `crates/medulla-ann/src/low_storage.rs::tests::compact_decode_and_refusals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.3` | `tests/retrieval/test_low_storage_ann_served.py::test_selective_regeneration_and_rescore` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.4` | `crates/medulla-semantic-index/src/semantic_embedding_batch.rs::tests::budgets_keys_and_duplicates` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.5` | `crates/medulla-ann/src/low_storage.rs::tests::pruning_disabled_identical_enabled_connected` |
| Planned | `MEDULLA-FEDERATED-QUERY-R040.5.6` | `tests/retrieval/test_low_storage_ann_served.py::test_selectivity_bound_and_readback` |
| Planned | `MEDULLA-FEDERATED-QUERY-R058.4.7.2.1` | `tests/nl_query_grounding.rs::decision_record_identifiers` |
| Planned | `MEDULLA-FEDERATED-QUERY-R058.4.7.2.2` | `tests/nl_query_grounding.rs::timeseries_identifiers` |
| Planned | `MEDULLA-FEDERATED-QUERY-R058.4.7.2.3` | `tests/nl_query_grounding.rs::cep_identifiers` |
| Planned | `MEDULLA-FEDERATED-QUERY-R058.4.7.2.4` | `tests/nl_query_grounding.rs::udf_identifiers` |
| Planned | `MEDULLA-FEDERATED-QUERY-R058.4.7.2.5` | `tests/nl_query_grounding.rs::rdf_identifiers` |
| Planned | `MEDULLA-FEDERATED-QUERY-R072.2` | `tests/federated/test_unified_query_source_plan_served.py::test_plan_sources_equal_authority_rows` |
| Planned | `MEDULLA-FEDERATED-QUERY-R073.3` | `crates/medulla-federation/src/federation_opt/tests.rs::mcp_remote_delegates_to_connector_row_source` |
| Planned | `MEDULLA-FEDERATED-QUERY-R073.3` | `tests/federation/test_web_metasearch_served.py::test_mcp_source_resolves` |
| Planned | `MEDULLA-FEDERATED-QUERY-R079.2.1` | `tests/lake_manifest_interoperability.rs::append_compaction_cow_delete` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.11` | `tests/retrieval_round_served.rs::round_with_no_parts_returns_rows_only` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.11` | `tests/retrieval/test_round_client.py::test_round_request_round_trips` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.101.1` | `crates/medulla-types/src/retrieval_round.rs::tests::document_state_round_trips_and_checks_coverage` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.101.2` | `crates/medulla-plan/src/doc_state.rs::tests::state_digest_is_stable_for_same_content` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.101.3` | `tests/retrieval_round_served.rs::compressed_state_rerank_is_order_independent` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.103.1` | `crates/medulla-types/src/viewport.rs::tests::request_bounds_and_token_union_round_trip` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.103.2` | `crates/medulla-decision-stats/src/viewport.rs::tests::bins_match_oracle_and_caps_hold` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.103.3` | `tests/viewport_served.rs::selection_tokens_resolve_only_within_pinned_revisions` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.103.4` | `tests/viewport_served.rs::viewport_bins_are_bounded_and_pinned` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.103.4` | `tests/test_viewport_client.py::test_viewport_round_trip` |
| Planned | `MEDULLA-FEDERATED-QUERY-R081.103.5` | `crates/medulla-tsdb/src/store.rs::ordered_chunk_tests::block_aggregates_match_point_scan` |
| Planned | `MEDULLA-FEDERATED-QUERY-R086.100.1` | `crates/medulla-types/src/graphlearn_nary.rs::tests::order_changes_digest_and_arity_is_bounded` |
| Planned | `MEDULLA-FEDERATED-QUERY-R086.100.2` | `crates/medulla-graph-algos/src/graphlearn/nary.rs::tests::unseen_member_scores_inductively` |
| Planned | `MEDULLA-FEDERATED-QUERY-R086.100.3` | `tests/graphlearn_nary_served.rs::refusals_and_unchanged_graph_revision` |
| Planned | `MEDULLA-FEDERATED-QUERY-R086.100.4` | `tests/graphlearn_nary_served.rs::nary_hypotheses_are_read_only_and_ordered` |
| Planned | `MEDULLA-FEDERATED-QUERY-R090.8` | `crates/medulla-graph-core/src/graph/core_expand.rs::tests::hop_cap_and_order` |
| Planned | `MEDULLA-FEDERATED-QUERY-R090.8` | `tests/retrieval/test_discover_planning_served.py::test_two_hop_neighbour_is_returned_with_path` |
| Planned | `MEDULLA-FEDERATED-QUERY-R091.7` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::cache_and_materialize_candidates_respect_capability` |
| Planned | `MEDULLA-FEDERATED-QUERY-R091.7` | `tests/federation/test_plan_evolution_served.py::test_cache_candidate_kept_when_faster` |
| Planned | `MEDULLA-FEDERATED-QUERY-R091.8` | `crates/medulla-federation/src/federation_opt/plan_evolution.rs::tests::index_suggestion_threshold` |
| Planned | `MEDULLA-FEDERATED-QUERY-R091.8` | `tests/federation/test_plan_evolution_served.py::test_index_suggestion_is_a_fact_only` |
| Planned | `MEDULLA-FEDERATED-QUERY-R607.7` | `crates/medulla-graph-algos/src/graph_algos/analogy.rs::tests::isomorphic_subgraphs_match_and_boundary_score` |
| Planned | `MEDULLA-FEDERATED-QUERY-R607.7` | `tests/reasoning/test_topology_served.py::test_analogy_served` |
| Planned | `MEDULLA-FEDERATED-QUERY-R607.8` | `crates/medulla-graph-algos/src/graph_algos/synergy.rs::tests::two_community_bridge_ranks_first` |
| Planned | `MEDULLA-FEDERATED-QUERY-R607.8` | `tests/reasoning/test_topology_served.py::test_synergy_served` |
| Planned | `MEDULLA-FEDERATED-QUERY-R607.9` | `crates/medulla-graph-algos/src/graph_algos/positional.rs::tests::connected_set_outscores_scattered_set` |
| Planned | `MEDULLA-FEDERATED-QUERY-R607.9` | `tests/reasoning/test_topology_served.py::test_hyperedge_score_served` |
| Planned | `MEDULLA-FEDERATED-QUERY-R651.7` | `crates/medulla-h-query/src/query/expansion.rs::tests::parse_drops_duplicates_and_oversize` |
| Planned | `MEDULLA-FEDERATED-QUERY-R651.7` | `tests/retrieval/test_query_understanding_served.py::test_expansion_generation_with_scripted_provider` |
| Planned | `MEDULLA-FEDERATED-QUERY-R652.7` | `crates/medulla-plan/src/rerank.rs::tests::model_scorer_unavailable_falls_back` |
| Planned | `MEDULLA-FEDERATED-QUERY-R652.7` | `tests/retrieval/test_rerank_served.py::test_model_scorer_when_artifact_bound` |
| Planned | `MEDULLA-FEDERATED-QUERY-R653.8` | `crates/medulla-plan/src/corpus_retrieval.rs::tests::entity_context_order` |
| Planned | `MEDULLA-FEDERATED-QUERY-R653.8` | `tests/retrieval/test_corpus_retrieve_served.py::test_entity_mode` |
| Planned | `MEDULLA-FEDERATED-QUERY-R653.9` | `crates/medulla-plan/src/corpus_retrieval.rs::tests::session_hits_order` |
| Planned | `MEDULLA-FEDERATED-QUERY-R653.9` | `tests/retrieval/test_corpus_retrieve_served.py::test_session_mode` |
| Planned | `MEDULLA-FEDERATED-QUERY-R653.10` | `crates/medulla-plan/src/corpus_retrieval.rs::tests::lineage_depth_and_cycles` |
| Planned | `MEDULLA-FEDERATED-QUERY-R653.10` | `tests/retrieval/test_corpus_retrieve_served.py::test_lineage_mode` |
| Planned | `MEDULLA-FEDERATED-QUERY-R654.7` | `crates/medulla-plan/src/code_context.rs::tests::diagnose_orders_chain_by_stage` |
| Planned | `MEDULLA-FEDERATED-QUERY-R654.7` | `tests/retrieval/test_code_context_served.py::test_diagnose_chain` |
| Planned | `MEDULLA-FEDERATED-QUERY-R703.7` | `tests/retrieval/test_access_check_served.py::test_discover_excludes_checked_at_read_sources` |
| Planned | `MEDULLA-FEDERATED-QUERY-R706.7` | `tests/federated/test_policy_mask_served.py::test_cross_tenant_policy_not_applied` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.8` | `tests/federated/test_stored_query_served.py::test_share_scope_limits_calls` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.9` | `tests/federated/test_stored_query_served.py::test_workspace_share_schedule_and_lint` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.10` | `crates/medulla-source-types/src/stored_query.rs::tests::sql_and_sparql_text_binds_typed_literals` |
| Planned | `MEDULLA-FEDERATED-QUERY-R708.10` | `tests/federated/test_stored_query_served.py::test_sql_and_sparql_stored_queries_call` |
| Planned | `MEDULLA-FEDERATED-QUERY-R720.7` | `crates/medulla-plan/tests/uql_epistemic_stages.rs::require_nonempty_refuses_with_code` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.8` | `crates/medulla-plan/tests/pack_source.rs::undeclared_filters_stay_local_and_residual_always_runs` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.9` | `tests/test_uql_foreign_execution.py::test_driver_binding_rejects_payload_authority` |
| Planned | `MEDULLA-FEDERATED-QUERY-R722.10` | `tests/test_uql_foreign_execution.py::test_lifecycle_sequence_replay_expiry_and_terminal` |
| Planned | `MEDULLA-FEDERATED-QUERY-R723` | `tests/semantic_index/test_reembed_methods_served.py::test_admit_then_reembed_one_job` |
| Planned | `MEDULLA-FEDERATED-QUERY-R724` | `tests/served_schema_projections.rs::projections_follow_caller_grants_and_hide_other_boundaries` |
| Planned | `MEDULLA-FEDERATED-QUERY-R725` | `tests/served_viewport_bins.rs::bins_match_independent_reference_and_obey_limits` |
| Planned | `MEDULLA-FEDERATED-QUERY-R726` | `crates/medulla-uql/src/parser/evaluate.rs::tests::parses_every_kernel_and_refuses_unknown` |
| Planned | `MEDULLA-FEDERATED-QUERY-R727` | `tests/query/test_nl_answer_atoms_served.py::test_answer_cites_atom_provenance` |
| Planned | `MEDULLA-FEDERATED-QUERY-R729` | `tests/federated/test_raw_pushdown_served.py::test_raw_source_search_is_budgeted_and_refuses_over_ceiling` |
| Planned | `MEDULLA-FEDERATED-QUERY-R730` | `tests/test_unified_planner_gate.py::test_failed_gate_records_the_freeze_decision` |
| Planned | `MEDULLA-FEDERATED-QUERY-R731` | `tests/query/test_latency_classes_served.py::test_interactive_budget_refuses_and_analytical_runs_as_job` |
| Planned | `MEDULLA-FEDERATED-QUERY-R732` | `tests/query/test_planner_evolution_served.py::test_recorded_plans_train_a_challenger_that_promotes_only_on_improvement` |
| Behavior | `MEDULLA-FEDERATED-QUERY-R722.7` | `crates/medulla-h-admin/src/admin/connector_pack/tests/source_approval.rs::approve_source_requires_current_head_and_contract_digest` |
| Behavior | `MEDULLA-FEDERATED-QUERY-R733` | `crates/medulla-uql/tests/group_stage.rs::group_by_label_counts_rows`, `crates/medulla-uql/tests/group_stage.rs::unknown_field_is_refused` and `tests/query/test_uql_group_served.py::test_node_type_counts_over_the_wire` (L2, L4). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R734` | `crates/medulla-uql/tests/order_skip_stage.rs::order_is_stable_with_row_id_tiebreak` and `crates/medulla-uql/tests/order_skip_stage.rs::skip_past_the_cap_is_refused` (L2). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R735` | `crates/medulla-uql/tests/edges_source.rs::edges_rows_carry_source_target_rel_and_properties` and `tests/query/test_uql_edges_served.py::test_edge_rows_respect_edge_read_scope` (L2, L4). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R736` | Satisfied only when every child is satisfied. Root journey: `tests/query/test_evidence_forest_served.py::test_forest_fits_budget_and_keeps_supported_evidence` (planned) (L4). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R736.1` | `crates/medulla-result-types/tests/evidence_forest.rs::scores_stay_separate_and_bounded` (planned) (L2). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R736.2` | `crates/medulla-plan/tests/evidence_forest.rs::expansion_respects_hops_and_cap` (planned) (L2). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R736.3` | `crates/medulla-plan/tests/evidence_forest.rs::selection_is_a_forest_within_budget_and_drops_redundant_and_conflicting` (planned) (L2). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R736.4` | `tests/query/test_evidence_forest_served.py::test_round_with_budget_returns_forest_rows` (planned) (L4). |
| Behavior | `MEDULLA-FEDERATED-QUERY-R736.5` | `tests/query/test_evidence_forest_served.py::test_forest_fits_budget_and_keeps_supported_evidence` (planned) (L4). |

- **MEDULLA-FEDERATED-QUERY-R737:** `crates/medulla-exec/tests/rowset_columns.rs::group_and_projection_share_one_typed_row_shape` (planned). Proves the row at its entry point; no receipt is asserted.
- **MEDULLA-FEDERATED-QUERY-R720.8:** `crates/medulla-h-query/tests/explain_plan_dag.rs::explain_reports_nodes_and_root_count` (planned). Proves the row at its entry point; no receipt is asserted.
- **MEDULLA-FEDERATED-QUERY-R720.9:** `crates/medulla-uql/tests/union_and_window.rs::union_and_valid_as_of_window_compile_into_one_plan` (planned). Proves the row at its entry point; no receipt is asserted.

## Explicit state machine rows (Article XXXI)

- `MEDULLA-FEDERATED-QUERY-R738`: see each child (rollup).
- `MEDULLA-FEDERATED-QUERY-R738.1`: `crates/medulla-viz-core/src/capability_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-viz-core/src/capability_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-viz-core/src/capability_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::capability_machine_matches_edge_table` (planned, L1): proves: CapabilityEntry/CapabilityStatus state machine: route 2 census findings through CapabilityStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.2`: `crates/medulla-plan/src/ops_causal_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-plan/src/ops_causal_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-plan/src/ops_causal_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::ops_causal_machine_matches_edge_table` (planned, L1): proves: CausalLink/OpsCausalStage state machine: route 1 census findings through OpsCausalStage::apply.
- `MEDULLA-FEDERATED-QUERY-R738.3`: `crates/medulla-types/src/claim_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/claim_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/claim_machine.rs::tests::no_writer_sets_state_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::claim_machine_matches_edge_table` (planned, L1): proves: ClaimRecord/ClaimState state machine: route 1 census findings through ClaimState::apply.
- `MEDULLA-FEDERATED-QUERY-R738.4`: `crates/medulla-plan/src/code_context_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-plan/src/code_context_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-plan/src/code_context_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::code_context_machine_matches_edge_table` (planned, L1): proves: CodeContextAnswer/CodeContextStatus state machine: route 1 census findings through CodeContextStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.5`: `crates/medulla-types/src/contradiction_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/contradiction_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/contradiction_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::contradiction_machine_matches_edge_table` (planned, L1): proves: ContradictionRecord/ContradictionStatus state machine: route 1 census findings through ContradictionStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.6`: `crates/medulla-exec/src/exec/fragment_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-exec/src/exec/fragment_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-exec/src/exec/fragment_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::fragment_machine_matches_edge_table` (planned, L1): proves: ForeignFragmentReport/FragmentStage state machine: route 1 census findings through FragmentStage::apply.
- `MEDULLA-FEDERATED-QUERY-R738.7`: `crates/medulla-types/src/gap_topic_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/gap_topic_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/gap_topic_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::gap_topic_machine_matches_edge_table` (planned, L1): proves: GapTopic/GapStatus state machine: route 1 census findings through GapStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.8`: `crates/medulla-geo/src/task_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-geo/src/task_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-geo/src/task_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::task_machine_matches_edge_table` (planned, L1): proves: GeoTask/TaskStatus state machine: route 1 census findings through TaskStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.9`: `crates/medulla-result-types/src/hierarchy_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-result-types/src/hierarchy_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-result-types/src/hierarchy_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::hierarchy_machine_matches_edge_table` (planned, L1): proves: HierarchyStageReceipt/HierarchyStage state machine: route 2 census findings through HierarchyStage::apply.
- `MEDULLA-FEDERATED-QUERY-R738.10`: `crates/medulla-result-types/src/descent_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-result-types/src/descent_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-result-types/src/descent_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::descent_machine_matches_edge_table` (planned, L1): proves: HierarchyVisit/DescentStage state machine: route 1 census findings through DescentStage::apply.
- `MEDULLA-FEDERATED-QUERY-R738.11`: `crates/medulla-result-types/src/solver_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-result-types/src/solver_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-result-types/src/solver_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::solver_machine_matches_edge_table` (planned, L1): proves: JointSelection/SolverStatus state machine: route 1 census findings through SolverStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.12`: `crates/medulla-catalog-types/src/distribution.rs::tests::rng_word_is_not_a_lifecycle_field` (planned, L1): proves: Non-lifecycle fields named `state`: the generator word of four `SplitMix64` pseudo-random generators.
- `MEDULLA-FEDERATED-QUERY-R738.13`: `crates/medulla-geo/src/gpx.rs::tests::tag_kind_is_one_enum_and_both_flags_are_gone` (planned, L1): proves: Tag kind flags: `is_close` and `is_self_closing` in the GPX tag parser.
- `MEDULLA-FEDERATED-QUERY-R738.14`: `crates/medulla-result-types/src/source_query_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-result-types/src/source_query_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-result-types/src/source_query_machine.rs::tests::no_writer_sets_phase_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::source_query_machine_matches_edge_table` (planned, L1): proves: SourceQueryApprovalCheck/SourceQueryPhase state machine: route 1 census findings through SourceQueryPhase::apply.
- `MEDULLA-FEDERATED-QUERY-R738.15`: `crates/medulla-types/src/promotion_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/promotion_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/promotion_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::promotion_machine_matches_edge_table` (planned, L1): proves: StageReceipt/PromotionStage state machine: route 1 census findings through PromotionStage::apply.
- `MEDULLA-FEDERATED-QUERY-R738.16`: `crates/medulla-result-types/src/epistemic_operations/trace_outcome_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-result-types/src/epistemic_operations/trace_outcome_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-result-types/src/epistemic_operations/trace_outcome_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::trace_outcome_machine_matches_edge_table` (planned, L1): proves: TraceOutcome/TraceOutcomeStatus state machine: route 1 census findings through TraceOutcomeStatus::apply.
- `MEDULLA-FEDERATED-QUERY-R738.17`: `crates/medulla-types/src/uql_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/uql_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/uql_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::uql_machine_matches_edge_table` (planned, L1): proves: UqlStageReport/UqlStage state machine: route 1 census findings through UqlStage::apply.
- `MEDULLA-FEDERATED-QUERY-R738.18`: `crates/medulla-types/src/verdict_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/verdict_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/verdict_machine.rs::tests::no_writer_sets_state_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::verdict_machine_matches_edge_table` (planned, L1): proves: VerificationVerdict/VerdictState state machine: route 1 census findings through VerdictState::apply.
