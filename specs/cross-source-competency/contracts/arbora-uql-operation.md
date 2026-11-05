# Seam: the UQL operation behind the agent-claims acceptance

Provider: arbora ARBORA-OPS-R018
Consumer: medulla MEDULLA-CQ-XSRC-R009

## Wire shape

Arbora `ask` op `uql` forwards the program `tests/fixtures/cq/xsrc_r009.uql` (planned) with the typed parameters `$agent_id` and `$report_id` to `Method::Uql`. Medulla runs one plan: claims of the last 30 days that touch the report, their evidence clustered by embedding, and the contradictions decayed by age. The request carries the caller's tenant and principal.

## Refusal codes

`CQ_REPORT_NOT_FOUND` for an unknown report id, `UQL_E_CLUSTER_NO_VECTOR` when evidence rows hold no vectors and `UQL_E_DECAY_NO_TIME` for rows without valid time. A known report with an empty claim window returns an empty typed result, not an error.

## Authoritative types

`Method::Uql` and its parameter types in `crates/medulla-protocol-types/src/protocol/`; the handler is `crates/medulla-h-query/src/query/uql_statement.rs::handle_uql`.

## Test vectors

`tests/test_cq_xsrc_r009.py` (planned) holds the served vectors.
