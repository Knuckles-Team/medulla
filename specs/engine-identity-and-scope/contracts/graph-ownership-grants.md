# Seam: graph ownership grants and rollback

Provider: medulla MEDULLA-IDENTITY-R700
Consumer: arbora ARBORA-IDENTITY-R652

## Wire shape

Arbora calls the read-only `Method::GraphOwnershipPlan { graph }`, which answers `GraphOwnershipPlanReply { schema_version, graph, signals, signals_digest, plan, plan_digest, applied }`, then applies the returned plan through `Method::RbacAdmin` with `{"ApplyOwnership":{"graph","plan","plan_digest","signals_digest"}}` (reply `OwnershipApplied { graph, plan_digest, principals }`) and undoes it with `{"RollbackOwnership":{"graph","plan_digest"}}` (reply `ownership_rolled_back`). Each plan entry carries the full intended role set; apply stores a rollback record keyed by graph and plan digest in the identity store in the same image write.

## Refusal codes

`OWNERSHIP_LAST_ADMIN`, `OWNERSHIP_WIDENS_TENANT` and `OWNERSHIP_NO_ROLLBACK`; durable state is unchanged on refusal.

## Authoritative types

`GrantPlanEntry`, `RollbackEntry`, `GraphOwnershipSignals`, `GraphOwnershipPlanReply` and `OwnershipRecord` in `crates/medulla-identity-core-types/src/identity/ownership.rs`; `ApplyOwnership` and `RollbackOwnership` in `crates/medulla-identity-core-types/src/acl.rs`.

## Test vectors

`tests/identity/test_graph_ownership_served.py` (planned) holds the apply-then-rollback vector.
