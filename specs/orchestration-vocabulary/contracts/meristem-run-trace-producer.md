# Seam: run-trace and observation-window producers

Provider: medulla MEDULLA-ORCH-VOCAB-R013.2, MEDULLA-ORCH-VOCAB-R009.18.2
Consumer: meristem MERISTEM-HARNESS-CYCLE-R072, MERISTEM-HARNESS-CYCLE-R071

## Wire shape

Meristem writes a `:RunTrace` node, its ordered step records and its observation windows through the engine client `add_node`. Each step carries three digests computed by the one published `step_digest` recipe: SHA-256 over canonical JSON. A replay compares stored digests step by step through `Method::Uql`.

## Refusal codes

`RUN_TRACE_SHAPE_INVALID`, `STEP_DIGEST_INVALID`, `STEP_ORDER_INVALID`, `WINDOW_RUN_REVISION_MISMATCH` and `WINDOW_CONTENT_CONFLICT`. Each refusal stores nothing, and Meristem treats it as a failed write.

## Authoritative types

`StepRecord` and `step_digest` in `crates/medulla-result-types/src/outcome_bundle.rs`; `RunObservationWindow` in `crates/medulla-types/src/run_observation.rs`.

## Test vectors

The byte vectors of R013.2 are committed beside the function; Meristem recomputes them in its own tests.
