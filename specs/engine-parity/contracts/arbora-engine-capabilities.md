# Seam: engine capability report

Provider: medulla MEDULLA-ENGINE-PARITY-R020.2
Consumer: arbora ARBORA-OPS-R006

## Wire shape

The served method `EngineCapabilities` takes no parameters and returns the `EngineCapabilities` report: wire features, index kinds, constraint kinds, the compatibility-matrix digest, and the replication and recovery flags. It needs scope `service:control`, is read-only and is not audited. Arbora reads the engine's packaged wire-callable contract and exposes the method as a generated operation.

## Refusal codes

None. The method never refuses a caller that holds `service:control`; a caller without the scope receives the standard access refusal.

## Authoritative types

`EngineCapabilities`, `WireFeature`, `IndexKind` and `ConstraintKind` in `crates/medulla-types/src/engine_capabilities.rs`.

## Test vectors

`tests/test_engine_capabilities_served.py` and `crates/medulla-types/tests/engine_capabilities.rs` hold the vectors that both sides test against.
