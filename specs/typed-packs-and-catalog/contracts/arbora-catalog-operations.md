# Seam: glossary, stewardship and dashboard operations

Provider: medulla MEDULLA-TYPED-PACKS-R801
Consumer: arbora ARBORA-OPS-R855

## Wire shape

Arbora serves the methods as `medulla.glossary.*`, `medulla.stewardship.*` and `dashboard.*` operations with no bespoke operation of its own. Each call reaches the served methods `Method::Glossary{op}` and `Method::DashboardAsset{op}` through `dispatch_catalog_admin_methods`. Reads are UQL, SPARQL and `search` under the caller grants. Arbora supplies the verified principal and tenant. Approval admission lives in arbora (`ARBORA-OPS-R888`); the record and the reviewer choice stay in Medulla.

## Refusal codes

The stable codes registered by the child rows of `MEDULLA-TYPED-PACKS-R801` to `MEDULLA-TYPED-PACKS-R805` and `MEDULLA-TYPED-PACKS-R109`, in `ServerErrorCode`. A refused call writes nothing.

## Authoritative types

`Method::Glossary`, `GlossaryOp`, `Method::DashboardAsset` and `DashboardAssetOp` in `crates/medulla-protocol-types/src/protocol/method/method_finish.rs`. The records are `crates/medulla-types/src/glossary.rs` and `crates/medulla-types/src/dashboard.rs` (both new).

## Test vectors

`tests/typed_packs/test_glossary_served.py` and `tests/typed_packs/test_dashboard_asset_served.py` hold the served vectors that both sides test against.
