# Knob table (MEDULLA-CONTRACT-R058)

Provider: medulla MEDULLA-CONTRACT-R058
Consumer: medulla MEDULLA-DURABLE-KERNEL-R083.5

The initial `KNOB_TABLE` of `crates/medulla-server-api/src/knob_config.rs`: one entry per knob named by a spec row (`knob <name>`). Every knob is read from `--knob <name>=<value>`, then env `MEDULLA_<NAME>` (upper-case, dots to underscores), then the default. Ranges are inclusive. A `Fixed` knob has a one-value range; the evolution loop never changes it (MEDULLA-DECISION-ENGINE-R179).

| Knob | Type | Default | Range | Unit | Class | Env |
|---|---|---|---|---|---|---|
| `kvcache.hot_bytes` | u64 | 1073741824 | [67108864, 1099511627776] | bytes | Tunable | `MEDULLA_KVCACHE_HOT_BYTES` |
| `kvcache.warm_bytes` | u64 | 4294967296 | [67108864, 1099511627776] | bytes | Tunable | `MEDULLA_KVCACHE_WARM_BYTES` |
| `kvcache.cold_bytes` | u64 | 68719476736 | [67108864, 1099511627776] | bytes | Tunable | `MEDULLA_KVCACHE_COLD_BYTES` |
| `council.quorum_fraction` | f64 | 0.5 | [0.5, 1] | fraction | Tunable | `MEDULLA_COUNCIL_QUORUM_FRACTION` |
| `designation.reward_decay` | f64 | 0.99 | [0.5, 1] | factor | Tunable | `MEDULLA_DESIGNATION_REWARD_DECAY` |
| `designation.max_required_capabilities` | u64 | 32 | [32, 32] | count | Fixed | `MEDULLA_DESIGNATION_MAX_REQUIRED_CAPABILITIES` |
| `topology.max_width` | u64 | 16 | [1, 16] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_WIDTH` |
| `topology.max_depth` | u64 | 8 | [1, 8] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_DEPTH` |
| `topology.max_rounds` | u64 | 8 | [1, 8] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_ROUNDS` |
| `topology.analogy_min` | f64 | 0.8 | [0, 1] | score | Tunable | `MEDULLA_TOPOLOGY_ANALOGY_MIN` |
| `topology.max_candidates` | u64 | 64 | [1, 1024] | count | Tunable | `MEDULLA_TOPOLOGY_MAX_CANDIDATES` |
| `schema_repair.shadow_sample_nodes` | u64 | 1000 | [1, 100000] | nodes | Tunable | `MEDULLA_SCHEMA_REPAIR_SHADOW_SAMPLE_NODES` |
| `reputation.min_support` | u64 | 10 | [1, 1000] | observations | Tunable | `MEDULLA_REPUTATION_MIN_SUPPORT` |
| `memory.search_state_max_bytes` | u64 | 1048576 | [4096, 67108864] | bytes | Tunable | `MEDULLA_MEMORY_SEARCH_STATE_MAX_BYTES` |
| `toc.line_index_stride` | u64 | 64 | [1, 4096] | lines | Tunable | `MEDULLA_TOC_LINE_INDEX_STRIDE` |
| `team.signal_max_age` | u64 | 300 | [1, 86400] | seconds | Tunable | `MEDULLA_TEAM_SIGNAL_MAX_AGE` |
| `held_item.min_passes` | u64 | 2 | [1, 10] | passes | Tunable | `MEDULLA_HELD_ITEM_MIN_PASSES` |
| `measured_fact.min_cases` | u64 | 30 | [1, 100000] | cases | Tunable | `MEDULLA_MEASURED_FACT_MIN_CASES` |
| `red_team.max_probes` | u64 | 16 | [1, 256] | probes | Tunable | `MEDULLA_RED_TEAM_MAX_PROBES` |
| `identity.ownership_writer_share` | f64 | 0.5 | [0, 1] | fraction | Tunable | `MEDULLA_IDENTITY_OWNERSHIP_WRITER_SHARE` |
| `retrieval.lineage_depth` | u64 | 3 | [1, 16] | hops | Tunable | `MEDULLA_RETRIEVAL_LINEAGE_DEPTH` |
| `feedback.events_per_doc` | u64 | 20 | [1, 1000] | events | Tunable | `MEDULLA_FEEDBACK_EVENTS_PER_DOC` |
| `feedback.rate_limit_per_hour` | u64 | 60 | [1, 10000] | events per hour | Fixed | `MEDULLA_FEEDBACK_RATE_LIMIT_PER_HOUR` |
| `federated.raw_scan_cost_per_kib` | f64 | 1.0 | [0.001, 1000] | cost units per KiB | Tunable | `MEDULLA_FEDERATED_RAW_SCAN_COST_PER_KIB` |
| `query.interactive_node_budget` | u64 | 100000 | [1000, 10000000] | nodes | Tunable | `MEDULLA_QUERY_INTERACTIVE_NODE_BUDGET` |
| `ingest.link_min_shared_mentions` | u64 | 2 | [1, 100] | mentions | Tunable | `MEDULLA_INGEST_LINK_MIN_SHARED_MENTIONS` |
| `media.max_blob_bytes` | u64 | 268435456 | [1048576, 4294967296] | bytes | Tunable | `MEDULLA_MEDIA_MAX_BLOB_BYTES` |
| `synthesis.context_budget_tokens` | u64 | 8192 | [512, 1048576] | tokens | Tunable | `MEDULLA_SYNTHESIS_CONTEXT_BUDGET_TOKENS` |
| `sdlc.mode.evolving_requires_approval` | bool | true | [true, true] | flag | Fixed | `MEDULLA_SDLC_MODE_EVOLVING_REQUIRES_APPROVAL` |
| `sdlc.delivery.shadow_unprompted_in_development` | bool | true | [true, true] | flag | Fixed | `MEDULLA_SDLC_DELIVERY_SHADOW_UNPROMPTED_IN_DEVELOPMENT` |
| `sdlc.delivery.max_automated_review_rounds` | u64 | 3 | [1, 20] | rounds | Tunable | `MEDULLA_SDLC_DELIVERY_MAX_AUTOMATED_REVIEW_ROUNDS` |
| `sdlc.delivery.owner_approval_required` | bool | true | [true, true] | flag | Fixed | `MEDULLA_SDLC_DELIVERY_OWNER_APPROVAL_REQUIRED` |
