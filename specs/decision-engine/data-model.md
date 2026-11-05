# MEDULLA-DECISION-ENGINE — Data model

This file describes the entities of the refusal taxonomy that `MEDULLA-DECISION-ENGINE-R010.100` adds, and the record that every decision seals.

## Entities

### RefusalCode

- Fields: one stable upper-snake string per variant.
- Owner: `crates/medulla-decision-types/src/decision/record/outcome.rs`.
- Rule: a code is unique across all families, and the unit test `codes_are_unique_and_stable` proves it.

### AbstainReason

- Fields: one variant per decision outcome that withholds an answer, each with its `RefusalCode`.
- Relation: a sealed `DecisionRecord` stores at most one `AbstainReason`.
- Family refusals convert into it through `From`; no family defines a parallel wire enum.

### DecisionError

- Fields: one variant per request refusal, each with its `RefusalCode`.
- Relation: it leaves through `DecisionErrorCode` and never enters a sealed record.

### Shared codes

- New file: `crates/medulla-decision-types/src/decision/codes.rs`.
- It holds the request codes that more than one family uses, declared once.

### DecisionRecord

- Fields: request digest, visible candidate facts with provenance, ontology, policy, catalog and head digests, quantized feature matrix when used, solver identity, eliminated constraints, derivations, `ResolutionKind`, `EvidenceClass`, result and record digest.
- Owner: `crates/medulla-decision-types/src/decision/record.rs`.
- Relation: a committed record is stored as the component `decision:<digest>`.
