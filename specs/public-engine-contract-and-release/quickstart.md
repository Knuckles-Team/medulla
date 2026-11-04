# MEDULLA-CONTRACT — Quickstart

Run the proofs of the public engine contract from a fresh checkout. The model and catalog checks run
without a server; the served proofs start a local engine from the checked-in test fixtures.

## Strict request and result models

The hand-written Python models replay the Rust round-trip vectors and refuse unknown fields:

```
python3 -m pytest -q tests/test_generated_models.py
```

## Run-time method catalog

The installed package's `medulla.engine.method_catalog()` parses and matches the descriptor rows:

```
python3 -m pytest -q tests/test_method_catalog.py
```

## Served proofs

The installed wire client calls `Decide` and the Agent Library pack journey against a served engine
and its authorization boundary:

```
python3 -m pytest -q tests/test_decide_auth_boundary.py tests/test_installed_wheel_decision_client_boundary.py
```

A source test alone is not served acceptance: the answer must come back through the served method.
