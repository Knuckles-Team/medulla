# MEDULLA-CQ-XSRC — Quickstart

Run the proofs of the cross-source competency. The recorded acceptance runs without network; the served proofs run against a locally served engine.

## Recorded acceptance (exit gate)

The supply-chain impact question over the recorded connector responses (`MEDULLA-CQ-XSRC-R007.1`):

```
cargo test -p medulla-rdf --test xsrc_supply_chain_acceptance
```

Class-only query planning and the authority-derived source sets (`MEDULLA-CQ-XSRC-R005`, `R008`):

```
cargo test -p medulla-rdf --test xsrc_supply_chain_query
```

The ontology declarations (supplier, component, vulnerability, host limits, contract; `MEDULLA-CQ-XSRC-R002`, `R008.2.1`, `R008.3.1`):

```
cargo test -p medulla-rdf --test supply_chain_declarations --test supply_chain_reasoning --test infrastructure_host_limits --test commercial_declarations
```

The authority seed and mapping fixtures (`MEDULLA-CQ-XSRC-R002.5`, `R002.7`, `R003.1` to `R003.5`):

```
cargo test -p medulla-rdf --test source_authority_fixture && cargo test -p medulla-core --test xsrc_mappings
```

## Served proofs

The served tests named in `test-spec.md` (the `tests/federated/` rows, `MEDULLA-CQ-XSRC-R011`) run against a served engine, for example the exit-gate case:

```
pytest tests/federated/test_supply_chain_impact_served.py
```

A source test alone is not served acceptance: the answer must come back through the served `SupplyChainImpact` method of `MEDULLA-CQ-XSRC-R011`.
