# Seam contract: retrieval verdict consumer

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-FEDERATED-QUERY-R081
Consumer: meristem MERISTEM-BOUNDARY-R034.1

## Provider

Repository: medulla. Rows: `MEDULLA-FEDERATED-QUERY-R081`, `MEDULLA-FEDERATED-QUERY-R081.11` and `MEDULLA-FEDERATED-QUERY-R082`. Medulla judges sufficiency, selects chunks, applies feedback and decides the stop. It runs no model. A terminal verdict returns no continuation.

## Consumer

Repository: meristem. Row `MERISTEM-BOUNDARY-R034.1`. The harness applies the verdict and runs the next round. It cannot bypass a terminal verdict.

## Wire shape

| Item | Shape |
|---|---|
| Call | `RoundRequest` with optional parts `hierarchy`, `sufficiency`, `selection`, `feedback`, `stop`, `arbitration` |
| Verdict | `SufficiencyVerdict` with `verdict`, `evidence_count`, `reason`, `repair`, `fallback` and `round` |
| Evidence | `RoundResult.premises` lists one `RetrievalPremise` per step |

## Refusal codes

- `RETRIEVAL_ROUND_LIMIT_REQUIRED`: a verdict call without `round_limit`.
- `RETRIEVAL_SCORE_PAIR_REQUIRED`: a selection without both scores.
- `RETRIEVAL_FEEDBACK_INVALID`: forged document ids or incompatible revisions.
- `RETRIEVAL_BUDGET_EXCEEDED`: an exhausted feedback or round budget.

## Authoritative types

- `SufficiencyVerdict`, `ChunkScorePair`, `RetrievalFeedback`: `crates/medulla-result-types/src/retrieval_verdict.rs` in medulla.
- `RoundRequest` and `RoundResult`: `crates/medulla-types/src/retrieval_round.rs` in medulla.

## Test vectors

- `crates/medulla-retrieval/src/retrieval_verdict_tests.rs::strict_insufficient_abstains` in medulla.
- `tests/retrieval_round_served.rs::adaptive_selection_rescues_low_embedding_chunk` in medulla.
