# Seam: served branch operations

Provider: medulla MEDULLA-BRANCH-MERGE-R001
Consumer: arbora ARBORA-OPS-R006

## Wire shape

Arbora forwards `CreateBranch`, `BranchMerge`, `BranchDiff` and `BranchReadAt` to the engine with the verified principal and tenant. Each method carries `tenant` and `graph`. Medulla checks `tenant` against the verified tenant and never reads a principal from the request body. A mutation names its branch in `MutationBatch.branch`; the default is `main`.

## Refusal codes

`BRANCH_NAME_INVALID`, `BRANCH_EXISTS`, `BRANCH_NAME_RESERVED`, `BRANCH_TENANT_DENIED`, `BRANCH_BASE_UNKNOWN`, `BRANCH_UNKNOWN`, `BRANCH_DELETED`, `BRANCH_NO_COMMON_ANCESTOR`, `BRANCH_VERSION_NOT_ON_CHAIN`, `MERGE_ADOPTION_INVALID`, `MERGE_TARGET_STALE`, `MERGE_ACTOR_UNKNOWN`, `LANDING_GATE_BLOCKED`, `SOURCE_CONFLICT`, `MERGE_BLOB_UNREADABLE`. The conflict codes are in `MEDULLA-DECISION-ENGINE-R828`.

## Authoritative types

The request and result types in `crates/medulla-protocol-types/src/protocol/method/method_11.rs` and `crates/medulla-types/src/branch.rs`.

## Test vectors

`tests/test_branch_create.py` and `tests/test_branch_merge_publish.py` hold the served vectors that both sides test against.
