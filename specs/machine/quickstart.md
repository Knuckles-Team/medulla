# MEDULLA-MACHINE — Quickstart

The shortest path that shows the feature working is the served proof.

1. Build the wheel: `maturin develop -m crates/medulla-pyengine/Cargo.toml --features python`.
2. Run the function proof: `pytest crates/medulla-pyengine/tests/test_machine_functions.py`. It checks the seven identity vectors, the lifecycle table and the hosts-file refusals through `medulla.engine`.
3. Run the served proof: `pytest tests/test_machine_inventory_ingest_served.py::test_three_connector_records_resolve_to_one_machine_with_its_state`. It ingests the fixture record set through `SourceIngest` and asks one `UnifiedQuery`.
4. Run the ontology proof: `cargo nextest run -p medulla-rdf state_machine_vocabulary`. It compares the lifecycle Turtle with the Rust edge table.

A minimal hosts file:

```yaml
schema: hosts/v1
hosts:
  - name: alpha
    addresses: [192.0.2.10]
    management_address: 192.0.2.110
    roles: [compute]
    groups: [rack-a]
    credentials:
      ssh: vault://hosts/alpha/ssh
      management: vault://hosts/alpha/management
```
