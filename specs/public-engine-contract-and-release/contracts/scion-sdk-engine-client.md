# Seam: scion-sdk's engine client adapter imports the published `medulla` package

Provider: medulla MEDULLA-CONTRACT-R001
Consumer: scion-sdk SCION-CENTRALIZED-CAPABILITIES-R018

Connectors reach the engine only through the SDK's engine adapter (`scion_sdk/sinks/medulla.py`,
`scion_sdk/compute/engine.py`, `scion_sdk/decide/`). The adapter depends on the published `medulla`
package from the package index; it never vendors or regenerates engine types.

## Wire shape

The adapter imports the engine client and the strict request and result models of the installed
package: `medulla.client` and the modules under `medulla.generated` (for example
`medulla.generated.source_ingestion` for `SourceRecord`, `SourceCheckpoint` and
`SourceIngestionReceipt`). The models are hand-written, set `extra=forbid`, and are pinned to the
Rust serde types by round-trip vectors. The client encodes each request body with the shared method
codec and signs the bytes it sends.

## Refusal codes

A model refuses an unknown or missing field with a validation error before any request is sent, and
refuses a result whose shape does not match. A served refusal arrives as the method's stable error
code and is raised as the typed error class generated for that code; a code newer than the installed
package is raised as the unknown-code variant. The adapter surfaces these to the connector unchanged.

## Owner rows

- medulla: MEDULLA-CONTRACT-R001 (strict Python models for every wire method, pinned to Rust vectors).
- scion-sdk: SCION-CENTRALIZED-CAPABILITIES-R018 (the engine client adapter and compute).
