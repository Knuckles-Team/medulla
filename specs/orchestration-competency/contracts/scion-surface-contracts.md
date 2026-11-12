# Contract — scion-surface-contracts

Provider: scion-sdk SCION-CONNECTOR-CONTROL-R916.3
Consumer: medulla MEDULLA-CQ-ORCH-R012.1

The same resource feeds `MEDULLA-CQ-ORCH-R012.2`; its prompt fields are `SCION-CONNECTOR-CONTROL-R916.8`.

## Wire shape

Each connector serves `ontology://<connector>/contracts.ttl` (text/turtle), captured into its pack. Namespace `sc:` = `https://knuckles-team.github.io/ontology/surface-contract#`. Individuals use `urn:scion:<connector>:<kind>:<name>` and core types `:Task`, `:Tool`, `:Prompt`, `:Skill`. A `sc:ToolUseContract` has `sc:forTask` and core `:hasStep` to `sc:ToolStep` nodes. A step has `sc:stepOrder`, `sc:callsTool`, `sc:param`, `sc:resultField` and `sc:refusal` to a `sc:Refusal` with `sc:refusalCode` and `sc:refusalWhen`; steps chain with core `:precedes`. Prompts, skills and tools carry `sc:servesTask`; prompts and skills carry `sc:usesTool`. Every `sc:` class is a subclass of a core class.

## Refusal codes

A task with no contract abstains its part with `ORCH_PART_EMPTY` (`MEDULLA-CQ-ORCH-R008.3`). A malformed pack is refused by the existing `ConnectorPack.Import` checks.

## Authoritative types

The vocabulary and renderer are `scion_sdk/ontology/surface_contract.ttl` and `scion_sdk/surface_contracts.py` in scion-sdk.

## Test vectors

The SDK hot-path test of `SCION-CONNECTOR-CONTROL-R916.6` renders the vectors; the fixture pack under `tests/orchestration/fixtures/q11/packs/` reuses one.
