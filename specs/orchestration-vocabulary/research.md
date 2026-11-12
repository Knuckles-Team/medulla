# orchestration-vocabulary — Research

## Decision: Per-turn usage is an append-only record

**Decision.** Per-turn usage is an append-only `SessionTimingRecord` written through the `DecisionLog` writer at each committed turn (`MEDULLA-ORCH-VOCAB-R814`); `turn_index` comes from the committed-turn receipt. `SessionUsage` is computed from the records and never stored, so it cannot drift from them.
