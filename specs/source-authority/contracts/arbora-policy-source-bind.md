# Seam: policy source binding

Provider: medulla MEDULLA-SRC-AUTH-R801
Consumer: arbora ARBORA-OPS-R050.2.1

## Wire shape

After a human review, Arbora sends `PolicySourceBind` with `source_ref`, `source_revision` (40 hex), `source_digest` (64 lowercase hex), `policy_iri` and `approver`. Medulla stores the binding on the component revision and activates it only when the stored revision and digest equal the request.

## Refusal codes

`POLICY_SOURCE_DIGEST_MISMATCH`, `POLICY_SOURCE_STALE` and `POLICY_SOURCE_UNREVIEWED`. Each leaves the active policy unchanged.

## Authoritative types

`PolicySourceBinding` in `crates/medulla-types/src/policy_source.rs`.

## Test vectors

`tests/policy_source_served.rs::stale_review_leaves_policy_unchanged` is the vector; Arbora proves its call against the same request and refusal codes.
