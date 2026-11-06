# Seam: Process engine and architecture tool sync

Provider: medulla MEDULLA-CQ-ORCH-R703 and MEDULLA-CQ-ORCH-R705
Consumer: connectors CONN-CAMUNDA-MCP-CONNECTOR-CONTENT-R003, CONN-CAMUNDA-MCP-R008, the `aris-mcp` process source spec (`CONN-ARIS-MCP`), CONN-ARCHIMATE-MCP-CONNECTOR-CONTENT-R003 and CONN-ARCHIMATE-MCP-CONNECTOR-CONTENT-R004

## Wire shape

Each connector declares pack sources for its definition or model documents. Each record carries a key, a revision and the document text; the ARIS connector's record carries the canonical wire form of `contracts/process-source-canonical.md` instead, live or from AML export files. The engine reads them through the foreign read path. Outbound, the engine queues a governed write-back proposal. Its payload is the document and its SHA-256 digest. The connector sink executes only a released change set.

## Refusal codes

`SOURCE_HELD`, `SOURCE_UNAVAILABLE` (an unconfigured ARIS connector answers `ARIS_NOT_CONFIGURED`), `PROCESS_SYNC_SOURCE_NOT_SECONDARY`, `SYNC_SOURCE_NOT_SECONDARY`, `PROPOSAL_UNAPPROVED`.

## Authoritative types

`ProcessDocument` in `crates/medulla-types/src/process_exchange.rs`; `ArchimateExchange` in `crates/medulla-types/src/archimate_exchange.rs`; `GovernedChangeSet` of MEDULLA-DECISION-ENGINE-R615.3.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
