# Knob table (MEDULLA-CONTRACT-R058)

Provider: medulla MEDULLA-CONTRACT-R058
Consumer: medulla MEDULLA-DURABLE-KERNEL-R083.5
Consumer: medulla MEDULLA-DURABLE-KERNEL-R800.3

The initial `KNOB_TABLE` of `crates/medulla-server-api/src/knob_config.rs`: one entry per knob named by a spec row (`knob <name>`). Every knob is read from `--knob <name>=<value>`, then env `MEDULLA_<NAME>` (upper-case, dots to underscores), then the default. Ranges are inclusive. A `Fixed` knob has a one-value range; the evolution loop never changes it (MEDULLA-DECISION-ENGINE-R179).

| Knob | Type | Default | Range | Unit | Class | Env |
|---|---|---|---|---|---|---|
| `kvcache.hot_bytes` | u64 | 1073741824 | [67108864, 1099511627776] | bytes | Tunable | `MEDULLA_KVCACHE_HOT_BYTES` |
| `kvcache.warm_bytes` | u64 | 4294967296 | [67108864, 1099511627776] | bytes | Tunable | `MEDULLA_KVCACHE_WARM_BYTES` |
| `kvcache.cold_bytes` | u64 | 68719476736 | [67108864, 1099511627776] | bytes | Tunable | `MEDULLA_KVCACHE_COLD_BYTES` |
| `kvcache.min_worth` | f64 | 0.001 | [0, 1000] | ms per byte per hit | Tunable | `MEDULLA_KVCACHE_MIN_WORTH` |
| `resp.incr_retry_limit` | u64 | 8 | [1, 64] | attempts | Tunable | `MEDULLA_RESP_INCR_RETRY_LIMIT` |
| `kvcache.answer_max_bytes` | u64 | 268435456 | [1048576, 68719476736] | bytes | Tunable | `MEDULLA_KVCACHE_ANSWER_MAX_BYTES` |
| `council.quorum_fraction` | f64 | 0.5 | [0.5, 1] | fraction | Tunable | `MEDULLA_COUNCIL_QUORUM_FRACTION` |
| `designation.reward_decay` | f64 | 0.99 | [0.5, 1] | factor | Tunable | `MEDULLA_DESIGNATION_REWARD_DECAY` |
| `designation.max_required_capabilities` | u64 | 32 | [32, 32] | count | Fixed | `MEDULLA_DESIGNATION_MAX_REQUIRED_CAPABILITIES` |
| `xshard.decision_deadline_ms` | u64 | 30000 | [1000, 600000] | ms | Tunable | `MEDULLA_XSHARD_DECISION_DEADLINE_MS` |
| `topology.max_width` | u64 | 16 | [1, 16] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_WIDTH` |
| `topology.max_depth` | u64 | 8 | [1, 8] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_DEPTH` |
| `topology.max_rounds` | u64 | 8 | [1, 8] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_ROUNDS` |
| `topology.analogy_min` | f64 | 0.6 | [0.1, 1.0] | ratio | Tunable | `MEDULLA_TOPOLOGY_ANALOGY_MIN` |
| `topology.lease_ttl_ms` | u64 | 900000 | [1000, 86400000] | ms | Tunable | `MEDULLA_TOPOLOGY_LEASE_TTL_MS` |
| `topology.max_candidates` | u64 | 1000 | [1, 100000] | sets | Tunable | `MEDULLA_TOPOLOGY_MAX_CANDIDATES` |
| `schema_repair.shadow_sample_nodes` | u64 | 10000 | [100, 1000000] | nodes | Tunable | `MEDULLA_SCHEMA_REPAIR_SHADOW_SAMPLE_NODES` |
| `ontology_diff.max_listed` | u64 | 100 | [10, 1000] | ids | Tunable | `MEDULLA_ONTOLOGY_DIFF_MAX_LISTED` |
| `reputation.min_support` | u64 | 10 | [1, 1000] | observations | Tunable | `MEDULLA_REPUTATION_MIN_SUPPORT` |
| `transfer.min_backbones` | u64 | 2 | [2, 16] | count | Tunable | `MEDULLA_TRANSFER_MIN_BACKBONES` |
| `transfer.min_samples` | u64 | 30 | [10, 10000] | labels | Tunable | `MEDULLA_TRANSFER_MIN_SAMPLES` |
| `transfer.skill_after_successes` | u64 | 3 | [1, 100] | labels | Tunable | `MEDULLA_TRANSFER_SKILL_AFTER_SUCCESSES` |
| `transfer.min_rollouts` | u64 | 3 | [1, 100] | records | Tunable | `MEDULLA_TRANSFER_MIN_ROLLOUTS` |
| `memory_sync.cursor_ttl_s` | u64 | 900 | [60, 86400] | seconds | Tunable | `MEDULLA_MEMORY_SYNC_CURSOR_TTL_S` |
| `memory.search_state_max_bytes` | u64 | 1048576 | [65536, 1048576] | bytes | Tunable | `MEDULLA_MEMORY_SEARCH_STATE_MAX_BYTES` |
| `toc.line_index_stride` | u64 | 64 | [16, 1024] | lines | Tunable | `MEDULLA_TOC_LINE_INDEX_STRIDE` |
| `team.signal_max_age` | u64 | 30 | [1, 3600] | seconds | Tunable | `MEDULLA_TEAM_SIGNAL_MAX_AGE` |
| `held_item.min_passes` | u64 | 2 | [1, 20] | passes | Tunable | `MEDULLA_HELD_ITEM_MIN_PASSES` |
| `measured_fact.min_cases` | u64 | 30 | [1, 100000] | cases | Tunable | `MEDULLA_MEASURED_FACT_MIN_CASES` |
| `red_team.max_probes` | u64 | 50 | [1, 500] | probes | Tunable | `MEDULLA_RED_TEAM_MAX_PROBES` |
| `identity.ownership_writer_share` | f64 | 0.10 | [0.01, 0.5] | fraction | Tunable | `MEDULLA_IDENTITY_OWNERSHIP_WRITER_SHARE` |
| `retrieval.lineage_depth` | u64 | 8 | [1, 16] | hops | Tunable | `MEDULLA_RETRIEVAL_LINEAGE_DEPTH` |
| `drift.sample_max` | u64 | 1000 | [10, 10000] | records | Tunable | `MEDULLA_DRIFT_SAMPLE_MAX` |
| `conformance.evidence_ttl_ms` | u64 | 2592000000 | [60000, 31536000000] | ms | Tunable | `MEDULLA_CONFORMANCE_EVIDENCE_TTL_MS` |
| `finance.regime_forecast.max_horizon` | u64 | 10000 | [1, 1000000] | steps | Tunable | `MEDULLA_FINANCE_REGIME_FORECAST_MAX_HORIZON` |
| `finance.regime_forecast.sum_tolerance` | f64 | 0.000000001 | [0.000000000001, 0.000001] | probability | Tunable | `MEDULLA_FINANCE_REGIME_FORECAST_SUM_TOLERANCE` |
| `finance.regime_forecast.max_brier` | f64 | 2.0 | [0.0, 2.0] | squared probability | Tunable | `MEDULLA_FINANCE_REGIME_FORECAST_MAX_BRIER` |
| `feedback.events_per_doc` | u64 | 1000 | [10, 100000] | events | Tunable | `MEDULLA_FEEDBACK_EVENTS_PER_DOC` |
| `feedback.rate_limit_per_hour` | u64 | 100 | [100, 100] | events per hour | Fixed | `MEDULLA_FEEDBACK_RATE_LIMIT_PER_HOUR` |
| `federated.raw_scan_cost_per_kib` | f64 | 1 | [1, 100] | cost units per KiB | Tunable | `MEDULLA_FEDERATED_RAW_SCAN_COST_PER_KIB` |
| `query.interactive_node_budget` | u64 | 100000 | [1000, 10000000] | nodes | Tunable | `MEDULLA_QUERY_INTERACTIVE_NODE_BUDGET` |
| `ingest.link_min_shared_mentions` | u64 | 2 | [1, 20] | count | Tunable | `MEDULLA_INGEST_LINK_MIN_SHARED_MENTIONS` |
| `media.max_blob_bytes` | u64 | 33554432 | [1048576, 268435456] | bytes | Tunable | `MEDULLA_MEDIA_MAX_BLOB_BYTES` |
| `synthesis.context_budget_tokens` | u64 | 8192 | [512, 1048576] | tokens | Tunable | `MEDULLA_SYNTHESIS_CONTEXT_BUDGET_TOKENS` |
| `sdlc.mode.evolving_requires_approval` | bool | true | [true, true] | flag | Fixed | `MEDULLA_SDLC_MODE_EVOLVING_REQUIRES_APPROVAL` |
| `sdlc.delivery.shadow_unprompted_in_development` | bool | true | [true, true] | flag | Fixed | `MEDULLA_SDLC_DELIVERY_SHADOW_UNPROMPTED_IN_DEVELOPMENT` |
| `sdlc.delivery.max_automated_review_rounds` | u64 | 3 | [1, 20] | rounds | Tunable | `MEDULLA_SDLC_DELIVERY_MAX_AUTOMATED_REVIEW_ROUNDS` |
| `sdlc.delivery.owner_approval_required` | bool | true | [true, true] | flag | Fixed | `MEDULLA_SDLC_DELIVERY_OWNER_APPROVAL_REQUIRED` |
| `pattern.doji_body_max` | f64 | 0.1 | [0.01, 0.3] | ratio | Tunable | `MEDULLA_PATTERN_DOJI_BODY_MAX` |
| `pattern.shadow_ratio_min` | f64 | 2.0 | [1.5, 4.0] | ratio | Tunable | `MEDULLA_PATTERN_SHADOW_RATIO_MIN` |
| `pattern.wick_max` | f64 | 0.1 | [0.0, 0.3] | ratio | Tunable | `MEDULLA_PATTERN_WICK_MAX` |
| `process.document_max_bytes` | u64 | 16777216 | [1024, 268435456] | bytes | Tunable | `MEDULLA_PROCESS_DOCUMENT_MAX_BYTES` |
| `process.document_max_depth` | u64 | 64 | [4, 256] | levels | Tunable | `MEDULLA_PROCESS_DOCUMENT_MAX_DEPTH` |
| `sync.max_records` | u64 | 50000 | [1, 1000000] | records | Tunable | `MEDULLA_SYNC_MAX_RECORDS` |
| `toc.max_span_lines` | u64 | 2000 | [10, 20000] | lines | Tunable | `MEDULLA_TOC_MAX_SPAN_LINES` |
| `context_budget.relevance_min` | f64 | 0.5 | [0.0, 1.0] | ratio | Tunable | - |
| `generation.batch_nodes` | u64 | 256 | [16, 4096] | nodes | Tunable | `MEDULLA_GENERATION_BATCH_NODES` |
| `adapter.rank` | u64 | 8 | [1, 8] | directions | Tunable | `MEDULLA_ADAPTER_RANK` |
| `adapter.holdout_pct` | u64 | 20 | [5, 50] | percent | Tunable | `MEDULLA_ADAPTER_HOLDOUT_PCT` |
| `adapter.min_eval_items` | u64 | 30 | [10, 10000] | items | Tunable | `MEDULLA_ADAPTER_MIN_EVAL_ITEMS` |
| `adapter.max_resident_bytes` | u64 | 40960 | [4096, 1048576] | bytes | Tunable | `MEDULLA_ADAPTER_MAX_RESIDENT_BYTES` |
| `adapter.monitor_min_outcomes` | u64 | 50 | [10, 10000] | outcomes | Tunable | `MEDULLA_ADAPTER_MONITOR_MIN_OUTCOMES` |
| `adapter.max_per_tenant` | u64 | 64 | [1, 1024] | adapters | Tunable | `MEDULLA_ADAPTER_MAX_PER_TENANT` |
| `failure.context_steps` | u64 | 2 | [0, 8] | steps | Tunable | `MEDULLA_FAILURE_CONTEXT_STEPS` |
| `failure.min_count` | u64 | 3 | [2, 1000] | records | Tunable | `MEDULLA_FAILURE_MIN_COUNT` |
| `usage.min_trials` | u64 | 5 | [1, 1000] | outcomes | Tunable | `MEDULLA_USAGE_MIN_TRIALS` |
| `research.node_budget` | u64 | 1000 | [10, 100000] | nodes | Tunable | `MEDULLA_RESEARCH_NODE_BUDGET` |
| `research.depth_limit` | u64 | 3 | [1, 10] | links | Tunable | `MEDULLA_RESEARCH_DEPTH_LIMIT` |
| `research.value_floor_micro` | u64 | 0 | [0, 1000000] | micro-units | Tunable | `MEDULLA_RESEARCH_VALUE_FLOOR_MICRO` |
| `match.w_token` | u64 | 400000 | [0, 1000000] | ppm | Tunable | `MEDULLA_MATCH_W_TOKEN` |
| `match.w_label` | u64 | 300000 | [0, 1000000] | ppm | Tunable | `MEDULLA_MATCH_W_LABEL` |
| `match.w_embedding` | u64 | 300000 | [0, 1000000] | ppm | Tunable | `MEDULLA_MATCH_W_EMBEDDING` |
| `match.max_block` | u64 | 256 | [8, 4096] | features | Tunable | `MEDULLA_MATCH_MAX_BLOCK` |
| `match.score_min` | u64 | 600000 | [0, 1000000] | ppm | Tunable | `MEDULLA_MATCH_SCORE_MIN` |
| `match.entropy_mbit` | u64 | 3500 | [1000, 6000] | millibits | Tunable | `MEDULLA_MATCH_ENTROPY_MBIT` |

## Wire shape

Input: repeatable `--knob <name>=<value>` command-line option and env `MEDULLA_<NAME>`; no wire message. Output: `KnobConfig`, one typed field per table row, handed to the listeners at startup.

## Refusal codes

`KNOB_INVALID` (value does not parse as the type), `KNOB_OUT_OF_RANGE` (outside the inclusive range), `KNOB_UNKNOWN` (a `--knob` name in no table). Each names the knob and the server exits non-zero before it binds.

## Owner rows

`MEDULLA-CONTRACT-R058` owns the loader and this table.
