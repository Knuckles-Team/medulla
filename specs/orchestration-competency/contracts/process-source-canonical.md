# Seam: Process sources and the canonical workflow

Provider: medulla MEDULLA-CQ-ORCH-R701, MEDULLA-CQ-ORCH-R704.9 and MEDULLA-ORCH-VOCAB-R014.9
Consumer: connectors `camunda-mcp` (BPMN 2.0, CONN-CAMUNDA-MCP-CONNECTOR-CONTENT-R003) and `aris-mcp` (EPC, the `CONN-ARIS-MCP` process source spec)

## Wire shape

A process source hands the engine either a document it reads natively (BPMN 2.0, DMN) or the canonical wire form `medulla.process.canonical/v1`: `schema`, `name`, `steps` (`id`, `kind`, `gate`, `label`, `role`, `ref`), `flows` (`source`, `target`, `condition`, `default`), `digest`, `source` (system, model id, step id to element id), `annex` (`after`, `before`, `construct`, `element_id`, `label`, `type_num`) and `losses`. Labels, roles, references and conditions are NFC with whitespace collapsed and trimmed. Step kinds are the closed `:dagNodeKind` values `start`, `end`, `human`, `agent`, `decision`, `branch`, `fork`, `join`, `wait`, `call`, `boundary`; gates are `and`, `or`, `xor`. Canonical ids `s1` to `sn` follow the Kahn order of MEDULLA-CQ-ORCH-R701.2. The digest is the SHA-256 of the sorted-key canonical JSON of `schema`, `steps` and `flows` only. A connector adapter computes ids and digest by the same rules; the engine recomputes both and refuses a difference. Exports carry `lossless` and `losses`; strict exports refuse instead of writing.

## Refusal codes

`PROCESS_IMPORT_UNMAPPED_ELEMENT`, `PROCESS_IMPORT_NOT_CANONICAL`, `PROCESS_IMPORT_DIGEST_MISMATCH`, `PROCESS_IMPORT_UNKNOWN_SOURCE`, `PROCESS_IMPORT_TOO_LARGE`, `PROCESS_EXPORT_LOSSY`; from an unconfigured ARIS connector, `ARIS_NOT_CONFIGURED`, which the sync passes through as `SOURCE_UNAVAILABLE`.

## Authoritative types

`CanonicalWorkflow`, `CanonicalStep`, `StepKind`, `GateKind`, `CanonicalFlow`, `SourceTrace`, `AnnexEntry`, `ProcessLoss` and `ProcessSourceSystem` in `crates/medulla-types/src/process_import.rs`.

## Test vectors

The invoice approval fixture in `crates/medulla-core/tests/fixtures/process_equivalence/` (`invoice-approval.bpmn`, `invoice-approval.aml`, golden `invoice-approval.canonical.json`) is shared with the `aris-mcp` test fixtures; both sides test against the same golden digest.
