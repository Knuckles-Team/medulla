# MEDULLA-CONTRACT — Data model

This spec adds the typed server-config knob types to the engine. Types marked new do not exist yet; the owning row names their exact file.

## Entities

### Knob configuration

- Fields: `KnobConfig` (one typed field per engine knob), `KNOB_TABLE` (dotted name, type, default, inclusive range, unit per knob) and `KnobRefusal` with the codes `KNOB_INVALID`, `KNOB_OUT_OF_RANGE` and `KNOB_UNKNOWN` (`MEDULLA-CONTRACT-R058`, `crates/medulla-server-api/src/knob_config.rs`, new).
- Rule: a value is read from the command-line flag, then from `MEDULLA_<NAME>`, then the default; a bad value refuses startup before any listener binds.
