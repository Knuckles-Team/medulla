# Seam: Execution-plan acceptance scenario

Provider: medulla MEDULLA-CQ-ORCH-R012
Consumer: arbora ARBORA-ACCEPTANCE-R006.7

## Wire shape

Arbora asks the plain-language question and the engine answers through `AgentExecutionAnswer` with the exact connector operations, their order and the skill that guides each step, each part with provenance.

## Refusal codes

`ORCH_PART_NO_SOURCE`, `ORCH_PART_EMPTY` and `ORCH_PART_ASSEMBLY_ABSTAINED` per part; `ACCESS_DENIED` for a tenant mismatch.

## Authoritative types

`ExecutionAnswer` in `crates/medulla-decision-outcome-types/src/decision/execution_answer.rs`.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
