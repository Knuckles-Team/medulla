# Seam: run-trace metric reads

Provider: medulla MEDULLA-ORCH-VOCAB-R010.1
Consumer: arbora ARBORA-OPS-R021.4.4

## Wire shape

Arbora sends one UQL statement of the `HARNESS_QUERIES` table through the engine client `uql`. No other method serves these reads. Medulla returns rows per group, or the state `abstained` with a reason. An unknown value is never a zero row.

## Refusal codes

`NO_SOLVED_TRACE` for a group without a solved trace. A statement outside the table is not a harness statement and runs as ordinary UQL. Arbora shows an abstention as no data.

## Authoritative types

The statement table in `crates/medulla-mutation-types/src/harness_metrics.rs`.

## Test vectors

The served tests named by R010.1 to R010.5 are the vectors; the consumer proves its side against the same fixture `crates/medulla-core/tests/fixtures/run_traces.json`.
