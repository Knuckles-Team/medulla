# Seam: repository-source effects for prompt blueprints

Provider: scion-sdk SCION-GOVERNED-WRITEBACK-R004.5
Consumer: medulla MEDULLA-TYPED-PACKS-R094.7

## Wire shape

Medulla never writes a file. The export of `MEDULLA-TYPED-PACKS-R094.2` (`BlueprintExport`) is the payload. Arbora `fleet.write_back.preview` and `fleet.write_back.commit` (`ARBORA-FLEET-R003.4` to `ARBORA-FLEET-R003.10`) carry it to the repository connector, which applies it as a typed, approved, conditional effect (`SCION-GOVERNED-WRITEBACK-R004.5` to `SCION-GOVERNED-WRITEBACK-R004.12`). Arbora then calls `MEDULLA-TYPED-PACKS-R094.8` with the receipt.

## Refusal codes

Owned by the provider rows: missing approval, stale base, path escape and unbound receipt. Medulla adds none.

## Authoritative types

`BlueprintExport` in `crates/medulla-agent-types/src/agent_component.rs`. The effect and receipt types belong to the provider rows.

## Test vectors

`tests/test_prompt_blueprints_served.py::test_approved_source_effect_dependency` drives export, arbora commit through a loopback connector and the receipt record.
