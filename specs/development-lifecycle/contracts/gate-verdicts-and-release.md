# Seam contract: gate verdicts, derived state and release plans

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-SDLC-R005
Consumer: pipelines PIPE-CONNSPEC-R009
Consumer: pipelines PIPE-GATES-R004
Provider: pipelines PIPE-RELEASE-R025
Consumer: medulla MEDULLA-SDLC-R018.1

## Wire shape

| Direction | Record | Fields |
|---|---|---|
| pipelines to medulla | `RecordGateVerdict` | `{changeset, gate_id, input_digest, verdict, evidence_digest, duration_ms}` |
| medulla to pipelines | `SpecStatus` answer | `[{row_id, state, landing_changesets[], test_ids[]}]` |
| pipelines to medulla | bundle plan file under `releases/` | `{bundle, rows_by_repo, closure, train_order, scenario, open_tasks}` |

`state` is one of `SPECIFIED`, `LANDED`, `VERIFIED` and `RETIRED`.

## Refusal codes

- `GATE_INPUT_DIGEST_MISMATCH`: the verdict names a tree that is not the changeset tree.
- `GATE_VERDICT_SEALED`: a verdict for the same gate and changeset exists.
- `SPEC_REVISION_UNKNOWN`: the status read names an unknown revision.
- `BUNDLE_ROW_UNRESOLVED`: a bundle plan names an unknown row.

## Authoritative types

- `crates/medulla-source-types/src/lifecycle.rs`: `SpecState`, `SpecRowState` and `SpecStatusAnswer`.
- `crates/medulla-types/src/source_changeset.rs`: the changeset that a verdict names.

## Test vectors

- `tests/test_sdlc_spec_status_served.py::test_served_status_reads_graph` holds the status vector.
- `tests/sdlc/test_release_graph_served.py::test_bundle_plan_becomes_records_with_in_bundle_edges` holds the bundle vector.
