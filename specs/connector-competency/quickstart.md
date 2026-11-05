# MEDULLA-CQ-CONN — Quickstart

The shortest served proof of a connector competency question runs one case through the production query path.

## Scenario

1. Start a served engine with the fixture sources of the case.
2. Import the connector pack, approve its mapping, and replay the recording of the connector through `FixtureSource`.
3. Run the `uql` of the case through `Method::Uql` and check the columns, `min_rows` and the `source`, `record_id` and `as_of` fields.
4. Remove the pack or one other source and check the refusal or the `Unconnected` abstention.

## Commands

Run the table-driven served test of `MEDULLA-CQ-CONN-R067.9` for one connector (planned):

```text
uv run pytest tests/connector_competency/test_competency_served.py -k "ansible-tower"
```

Run the vocabulary audit of `MEDULLA-CQ-CONN-R068` (planned):

```text
cargo test -p medulla-core --test connector_competency_r068_infrastructure
```

Run the onboarding proof of `MEDULLA-CQ-CONN-R085.6` (planned):

```text
uv run pytest tests/typed_packs/test_connector_auto_onboard_served.py
```
