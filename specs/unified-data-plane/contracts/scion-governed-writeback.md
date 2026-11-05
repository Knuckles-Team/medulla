# Seam: governed write-back to an attached source

Provider: medulla MEDULLA-UNIFIED-DATA-PLANE-R012.1
Consumer: scion-sdk SCION-GOVERNED-WRITEBACK-R001

## Wire shape

A write carries `{idempotency_key, approval_receipt, audit_reservation}` beside the shared envelope `{tenant_id, actor_id, source_id, trace_id, policy_context}`. The connector prefers the application API when the application owns validation, triggers or other business rules, and carries the approval receipt to Medulla. A replayed key returns the prior outcome and applies nothing again.

## Refusal codes

`DENIED` for a missing approval or a policy denial, `OUTCOME_INDETERMINATE` when the effect is unknown, and `CAPABILITY_UNSUPPORTED` for a source that declares no write path. A missing audit reservation produces no effect.

## Authoritative types

The write-back governance model in `crates/medulla-types` named by `MEDULLA-UNIFIED-DATA-PLANE-R012.1` and the audit reservation of `MEDULLA-DURABLE-KERNEL-R031`.

## Test vectors

The refusal tests of the provider row are the vectors: a missing approval, a missing reservation, a replayed key and a policy denial each leave the source unchanged. The consumer proves that its client sends the same fields.
