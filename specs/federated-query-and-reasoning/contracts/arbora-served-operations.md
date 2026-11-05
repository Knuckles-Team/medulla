# Seam contract: served operations of the query family

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-FEDERATED-QUERY-R650.3
Consumer: arbora ARBORA-OPS-R006

## Provider

Repository: medulla. Rows: `MEDULLA-FEDERATED-QUERY-R650.3` (the seven-step recipe for a new served method), `MEDULLA-FEDERATED-QUERY-R081.11`, `MEDULLA-FEDERATED-QUERY-R700.3`, `MEDULLA-FEDERATED-QUERY-R701.3`, `MEDULLA-FEDERATED-QUERY-R702.3` and `MEDULLA-FEDERATED-QUERY-R707`.

## Consumer

Repository: arbora. Row `ARBORA-OPS-R006` serves each engine method as an operation. Row `ARBORA-OPS-R883` binds a new method. Row `ARBORA-OPS-R857` serves the SPARQL endpoint. Arbora adds no handler of its own for this seam.

## Wire shape

| Method | Request | Answer |
|---|---|---|
| `RetrievalRound` | `RoundRequest {plan, corpus_revision, model_revision, parts}` | `RoundResult {rows, verdicts, inference, premises}` |
| `ArgumentMapPut`, `ArgumentMapGet`, `ArgumentMapEvaluate` | map id, map, semantics, node budget | stored map or `ExtensionResult` |
| `LiveArtifactPut`, `LiveArtifactGet`, `LiveArtifactRefresh` | artifact, id, expected version | artifact with refresh records |
| `SynthesizeSearchTask` | evidence subgraph | task and `RiskReport` |
| `ApplyMutation` | event type `SPARQL_HTTP_UPDATE_EVENT` and the update text | `SparqlUpdateReport` |

There is no `Method::SparqlUpdate`. Updates always arrive as `ApplyMutation`. The Rust serde types are the only contract. The Python client is hand-written and tested against a Rust round-trip vector.

## Refusal codes

- `ACCESS_DENIED`: the caller lacks the scope of the method.
- `SPARQL_UPDATE_FORBIDDEN`: an update without the write scope.
- `RETRIEVAL_ROUND_LIMIT_REQUIRED`: a verdict call without a round limit.
- `AIF_DANGLING_EDGE`, `LIVE_ARTIFACT_TEMPLATE_UNSAFE`, `SEARCH_TASK_SHORTCUT`: typed refusals of the new methods.

## Authoritative types

- `Method` variants: `crates/medulla-protocol-types/src/protocol/method/method_05.rs` and `method_07.rs` in medulla.
- Result contract: `crates/medulla-protocol-types/src/result_contract/query.rs` in medulla.

## Test vectors

- `tests/retrieval/test_context_compile_served.py::test_method_is_served_and_no_second_implementation` in medulla.
- `tests/federated/test_sparql_update_served.py::test_insert_then_select` in medulla.
