# Seam: stored digest queries and rule results

Provider: medulla MEDULLA-BUSINESS-OPS-R003
Consumer: arbora (row to file in the arbora spec)

## Wire shape

Arbora reads a stored-query node by name and runs it through `Method::Uql` with the verified principal and tenant. Every query takes an explicit as-of time. Medulla adds no method. Rule evaluation uses the same path through the `EVALUATE` stage (`MEDULLA-BUSINESS-OPS-R002`).

## Refusal codes

`ASOF_REQUIRED`, `RULE_VERSION_UNKNOWN`

## Authoritative types

The stored-query type in `crates/medulla-source-types/src/stored_query.rs` and the rule result shape in `crates/medulla-numeric/src/business/rule_spec.rs` (planned).

## Test vectors

`crates/medulla-server-served-tests/src/split_tests/business_ops_served_tests.rs` (planned) holds the served vectors that both sides test against.
