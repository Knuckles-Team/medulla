# MEDULLA-IDENTITY — Quickstart

The shortest served proofs of the identity, scope and metadata rows run against a locally served engine with locally signed credentials. No live identity provider or network is needed.

## Scenario

1. Start a served engine in `local` mode with one admin and write a node.
2. Transition the mode to `external` with no provider and check `IDENTITY_MODE_NO_IDP`; configure a provider, link the admin and transition; check that the older session refuses.
3. Write a `:RunScopePolicy` node and assemble with a candidate beyond it; check `SCOPE_EXCEEDS_POLICY`.
4. Share a node to `org`, revoke it, restart the engine and read as a second principal.

## Commands

Run the identity-mode journey (planned):

```text
uv run pytest tests/identity/test_identity_mode_served.py
```

Run the run-scope journey (planned):

```text
uv run pytest tests/identity/test_scope_bound_served.py
```

Run the share-and-restart journey (planned):

```text
uv run pytest tests/identity/test_share_restart_served.py
```

Run the existing scope-bound unit tests:

```text
cargo test -p medulla-isolation scope_bound
```

Run the existing tenancy and segment unit tests:

```text
cargo test -p medulla-identity-types tenancy segment
```
