# Seam: run-trace metric reads

Provider: medulla MEDULLA-ORCH-VOCAB-R010.1
Consumer: arbora ARBORA-OPS-R021.4.4

## Wire shape

Arbora sends one UQL statement of the `HARNESS_QUERIES` table through the engine client `uql`, or `Method::RunMetrics` for the five-query view (R803). Medulla returns rows per group, or the state `abstained` with a reason. An unknown value is never a zero row.

## Refusal codes

`NO_SOLVED_TRACE` for a group without a solved trace, `RUN_METRIC_QUERY_UNKNOWN` and `RUN_VIEW_WINDOW_INVALID` for a bad request. Arbora shows an abstention as no data.

## Authoritative types

The statement table in `crates/medulla-types/src/harness_metrics.rs` and the method variant in `crates/medulla-protocol-types/src/protocol/method/method_10.rs`.

## Test vectors

The served tests named by R010.1 to R010.5 and R803 are the vectors; the consumer proves its side against the same fixture `crates/medulla-core/tests/fixtures/run_traces.json`.
