# Seam: the served supply-chain impact method

Provider: medulla MEDULLA-CQ-XSRC-R011
Consumer: arbora ARBORA-FLEET-R052.1

## Wire shape

Arbora serves one intent tool that takes the question's ontology class roots and calls `Method::SupplyChainImpact` (new variant in `crates/medulla-protocol-types/src/protocol/method/method_10.rs`). Medulla returns `SupplyChainImpactReport` of `crates/medulla-source-types/src/supply_chain_report.rs`: a list of `ServiceImpact`, `sources_used` and `abstentions`. The method reads only; it writes no record.

## Refusal codes

`CQ_CLASS_UNKNOWN` for an unknown class root; `PARAMETER_INVALID` and `TENANT_DENIED` for parameter and tenant faults; the `SOURCE_COVERAGE_*` codes for coverage faults. An abstention is part of a valid report, never a refusal.

## Authoritative types

`SupplyChainImpactReport`, `ServiceImpact`, `ServiceImpactItem`, `SourceUsed` and `ReportAbstention` in `crates/medulla-source-types/src/supply_chain_report.rs`.

## Test vectors

`tests/federated/test_supply_chain_impact_served.py` (planned) holds the served vectors that both sides test against; the Rust round-trip vector is `crates/medulla-source-types/src/supply_chain_report.rs::tests::a_valid_report_builds_and_round_trips_the_wire_encoding`.
