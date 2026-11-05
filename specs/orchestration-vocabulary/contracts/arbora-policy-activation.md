# Seam: policy activation behind review

Provider: medulla MEDULLA-ORCH-VOCAB-R806
Consumer: arbora ARBORA-OPS-R050.2

## Wire shape

Arbora sends `Method::PolicyActivation` with the change kind, the source reference and the digest of the review receipt it holds. Medulla validates the receipt (R012.8), compares every rule against the invariant generation (R012.7.3) and then activates, replaces or retires the rule nodes in one batch. It returns one entity reference per clause.

## Refusal codes

`RECEIPT_MISSING`, `RECEIPT_STALE` and `INVARIANT_CONTRADICTION` with the invariant IRI; the promotion codes `POLICY_REVIEW_STALE` and `PROMOTION_CAS_CONFLICT`. Each refusal keeps the prior rule set.

## Authoritative types

`PolicyBinding` and `PolicyCandidate` in `crates/medulla-types/src/policy_document.rs`; the variant in `crates/medulla-protocol-types/src/protocol/method/method_11.rs`.

## Test vectors

The served tests of R806 and R012.7.3, including the cross-tenant negative case.
