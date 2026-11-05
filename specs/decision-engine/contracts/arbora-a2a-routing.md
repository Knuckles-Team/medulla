# Seam: A2A routing through the decision ladder

Provider: medulla MEDULLA-DECISION-ENGINE-R044.2
Consumer: arbora ARBORA-A2A-R023

## Wire shape

Arbora sends a typed routing question through `Method::Decide`. Medulla returns a ranked proposal with a resolution kind and an evidence class. Arbora applies the proposal after its own authorization.

## Refusal codes

An abstention carries a stable `AbstainReason` code. Arbora treats an abstention as no route.

## Authoritative types

`QuestionKind` in `crates/medulla-decision-types/src/decision/statistical.rs` and `AbstainReason` in `crates/medulla-decision-types/src/decision/record/outcome.rs`.

## Test vectors

The served tests named by the provider row are the vectors; the consumer proves its side against the same request and response types.
