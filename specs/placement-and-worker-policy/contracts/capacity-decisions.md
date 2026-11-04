# Seam: capacity decisions

Provider: medulla MEDULLA-DECISION-ENGINE-R112
Consumer: medulla MEDULLA-PLACEMENT-R001

## Wire shape

The placement premise is `capacity_premise::headroom(state, graph, scope)`: read-only headroom observations named `lease:<cell>:<class>` at the request's priority, recorded as `Observation` premises; the ledger is unchanged. The capacity adjustment question (`QuestionKind::CapacityProposal`, MEDULLA-DECISION-ENGINE-R824) answers `CapacityAdjustmentVerdict` over `CapacityAdjustmentProposal`; the model capacity question (`QuestionKind::ModelCapacity`, MEDULLA-DECISION-ENGINE-R825) answers a proposed concurrency limit, breaker state and group slot count.

## Refusal codes

`CAPACITY_UNAVAILABLE` (missing ledger facts); the nine `CapacityRefusalCode` values from `CAPACITY_PROPOSAL_DIGEST_INVALID` through `CAPACITY_HEADROOM_EXCEEDED`; `POLICY_LOOSENING` above a declared ceiling.

## Authoritative types

`CapacityAdjustmentProposal` and `CapacityAdjustmentVerdict` in `crates/medulla-types/src/decision/statistical/capacity.rs`; `ModelCapacityController`, `CircuitBreaker`, `GpuGroupBudget` and `CognitiveScheduler` in `crates/medulla-resource/src/admission.rs`.
