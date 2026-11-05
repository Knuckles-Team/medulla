# Seam: connector control and run feedback

Provider: medulla MEDULLA-DECISION-ENGINE-R159.3
Consumer: scion-sdk SCION-CONNECTOR-CONTROL-R904

## Wire shape

Medulla ranks connector paths from a window of independently evaluated outcomes in the decision log and returns the ranked order as an advisory proposal. The connector transport applies the order as its fallback order. Outcome reporting stays a harness row.

## Refusal codes

Medulla refuses a malformed path request with a stable code. A refusal changes no stored outcome.

## Authoritative types

`DecisionOutcomeEvaluation` in `crates/medulla-decision-types/src/decision/statistical/log.rs`.

## Test vectors

The served tests named by the provider row are the vectors; the consumer proves its side against the same request and response types.
