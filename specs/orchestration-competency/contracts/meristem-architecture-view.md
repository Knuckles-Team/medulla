# Seam: Architecture view consumed by planning admission

Provider: medulla MEDULLA-CQ-ORCH-R010.3
Consumer: meristem MERISTEM-HARNESS-CYCLE-R026.1

## Wire shape

Meristem reads `ArchitectureView` through the served method and compares the digest of the view in its planning admission request; it never derives a view itself.

## Refusal codes

`VIEW_NO_SUPPORTING_SOURCE`, `VIEW_UNKNOWN_REPOSITORY`, `VIEW_STALE_REVISION`, `VIEW_TOO_LARGE`.

## Authoritative types

`ArchitectureView`, `ViewScope` and `ViewAbstained` in `crates/medulla-types/src/architecture_view.rs`.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
