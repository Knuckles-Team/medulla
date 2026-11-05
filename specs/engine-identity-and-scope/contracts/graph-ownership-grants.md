# Seam: graph ownership grants and rollback

Provider: medulla MEDULLA-IDENTITY-R700
Consumer: arbora ARBORA-IDENTITY-R652

## Wire shape

Arbora calls the read-only `Method::GraphOwnershipPlan`, then applies the returned plan through `Method::RbacAdmin`. Each plan entry carries the full intended role set; apply stores a rollback record with the plan digest in the same transaction.

## Refusal codes

`OWNERSHIP_LAST_ADMIN`, `OWNERSHIP_WIDENS_TENANT` and `OWNERSHIP_NO_ROLLBACK`; durable state is unchanged on refusal.

## Authoritative types

`GrantPlanEntry`, `RollbackEntry` and `GraphOwnershipSignals` in `crates/medulla-identity-types/src/identity/ownership.rs` (new).

## Test vectors

`tests/identity/test_graph_ownership_served.py` (planned) holds the apply-then-rollback vector.
