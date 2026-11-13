# orchestration-vocabulary — Research

## Decision: Per-turn usage is an append-only record

**Decision.** Per-turn usage is an append-only `SessionTimingRecord` written through the `DecisionLog` writer at each committed turn (`MEDULLA-ORCH-VOCAB-R814`); `turn_index` comes from the committed-turn receipt. `SessionUsage` is computed from the records and never stored, so it cannot drift from them.

## Decision: Usage totals read the pinned price table as a graph node and page by a client-composed cursor

**Decision.** `run_trace_usage` (`MEDULLA-ORCH-VOCAB-R010.101`) takes the `PriceTable` revision as a request parameter and reads the `PriceTable` node with that `revision` from the same snapshot as the traces; it does not follow a champion pointer, so a replayed query prices identically. The counters are their own `UsageCategory` set on the trace rather than `SessionUsage`, because a trace is immutable and the session record is per turn. Totals are channels of `UqlResult::Rows`; values are exact up to 2^53 minor units, far above any real total. `UqlResult::Rows` carries no next-cursor field and adding one would change every construction site and the wire, so the cursor is composed by the client from `snapshot_revision` and the last row id, and a cursor from another snapshot refuses `USAGE_CURSOR_STALE`. The page ceiling is the registry knob `orchestration_limits.page_size_max`, shared with R004.6.3 and R009.101.2.

**Rationale.** Pinning by parameter keeps the query a pure function of its inputs; unknown stays null with a reason so a missing rate is never read as free.
