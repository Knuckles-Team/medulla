# Seam: issuer-bound host-start grant

Provider: medulla MEDULLA-DURABLE-KERNEL-R030.1
Consumer: arbora ARBORA-HOST-R010.7.1

## Wire shape

A service caller with scope `host:issue-start` sends `IssueHostStartGrant` with the beneficiary, session, workspace, configuration, interface and policy assertions. The grant is at most 64 KiB, each reference at most 512 bytes, and the engine inserts exactly one row. Engine time bounds the expiry to the asserted session expiry and to 24 hours. The issuer comes from the authenticated batch actor and sits outside the grant JSON. Arbora owns session policy and startup and consumes the grant through the engine.

## Refusal codes

`ACCESS_DENIED` for a missing scope and `CONTROL_LEASE_KIND_RESERVED` when the generic `IssueControlLease` names the kind `host.start`. Generic consumption of the grant refuses.

## Authoritative types

The control-lease row in `crates/medulla-work-store/src/redb_store/work_item/control_lease/host_start.rs`. The served handler is under `crates/medulla-h-jobs/src/work_item/`.

## Test vectors

`crates/medulla-h-jobs/src/work_item/host_start_tests.rs` holds the served vectors. The bound tests of the row hold the bounds, stamping and consumption refusals.
