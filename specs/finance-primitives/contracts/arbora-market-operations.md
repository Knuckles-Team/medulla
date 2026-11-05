# Seam: market operations over finance records

Provider: medulla MEDULLA-FINANCE-PRIMITIVES-R004.3.2
Consumer: arbora ARBORA-DATA-MARKET-R003

## Wire shape

Arbora sends `Method::FinanceRecordPut { kind, schema_version, record, expected_head }` and `Method::FinanceRecordGet { kind, record_id }` through signed dispatch. Position and projection reads and `Method::FinanceMarket` operations travel the same door. Medulla verifies tenant and principal from the signed context and returns typed results with source, as-of and currency.

## Refusal codes

`FINANCE_RECORD_VERSION_UNKNOWN`, `RECORD_DERIVED_READONLY`, `ACCESS_DENIED`, `RECORD_HEAD_CONFLICT` and `RECORD_SHAPE_INVALID`. Arbora passes each code to its caller unchanged.

## Authoritative types

`FinanceRecordKind` and `FINANCE_RECORD_POLICY` in `crates/medulla-types/src/wire_finance.rs`, and the `Method` variants in `crates/medulla-protocol-types/src/protocol/method/method_04.rs`.

## Test vectors

`tests/fixtures/finance_record_vectors.json` holds one round-trip vector per record kind. The provider proves them in `tests/finance_records_served.rs`; the consumer proves its side against the same bytes.
