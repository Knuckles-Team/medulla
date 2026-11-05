# Seam: governed write-back proposals

Provider: medulla MEDULLA-DECISION-ENGINE-R615
Consumer: scion-sdk SCION-GOVERNED-WRITEBACK-R006

## Wire shape

Medulla queues a write-back proposal for approval and releases it as one change set after approval. The connector executes only the released change set.

## Refusal codes

Arbora issues the approval (`ARBORA-OPS-R865`). Medulla refuses a release without a recorded approval. Exploration is refused for write-back questions.

## Authoritative types

`DecisionLog` write-back operations in `crates/medulla-h-decide/src/decide/stat_log.rs`.

## Test vectors

The served tests named by the provider row are the vectors; the consumer proves its side against the same request and response types.
