# Seam: Summary invocation receipt

Provider: meristem MERISTEM-HARNESS-CYCLE-R068
Consumer: medulla MEDULLA-MEDIA-VOCAB-R001.101.5

## Wire shape

Meristem calls the model and mints the invocation receipt `{owner_requirement_id, receipt_id, receipt_digest, provider, model, policy_revision, input_context_digest, output_digest}`. Medulla makes no model call and never mints a receipt; it validates the receipt against the stored summary context and stores the summary.

## Refusal codes

`RECEIPT_UNKNOWN`, `RECEIPT_MISMATCH`, `CITATION_OUTSIDE_TEXT`, `TRANSCRIPT_STALE`.

## Authoritative types

`SummaryReceipt` in `crates/medulla-types/src/meeting_transcript.rs`; the receipt digest is SHA-256 over the other fields.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
