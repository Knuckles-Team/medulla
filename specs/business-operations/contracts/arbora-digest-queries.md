# Seam: stored digest queries and rule results

Provider: medulla MEDULLA-BUSINESS-OPS-R003
Consumer: arbora ARBORA-OPS-R855

## Wire shape

At tenant onboarding arbora puts the three definitions of `business_digest::digest_queries()` (`digest.pending_approvals`, `digest.overdue`, `digest.changed_since`) with `Method::StoredQueryPut`, taking the text from the served vectors, and calls each by name with `Method::StoredQueryCall { name, version: None, args }` under the verified principal and tenant. The as-of is a parameter (`as_of_date`, or `from` and `to`). Medulla adds no method. Rule evaluation is the UQL `EVALUATE RULE` stage (`MEDULLA-BUSINESS-OPS-R002.7`).

## Refusal codes

`STORED_QUERY_PARAM_MISSING`, `ASOF_REQUIRED`, `RULE_NOT_FOUND`, `RULE_VERSION_UNKNOWN`

## Authoritative types

The stored-query type in `crates/medulla-source-types/src/stored_query.rs` the definitions in `crates/medulla-source-types/src/business_digest.rs`, and the rule result shape `RuleResult` in `crates/medulla-numeric/src/business/registry.rs`.

## Test vectors

`crates/medulla-server-served-tests/src/split_tests/business_ops_served_tests.rs` (planned) holds the served vectors that both sides test against.
