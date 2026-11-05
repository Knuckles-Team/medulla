# Seam: agent run provenance on a changeset

Provider: medulla MEDULLA-SCM-R012
Consumer: meristem MERISTEM-SCM-R001

## Wire shape

An agent principal passes `run_trace_id` in `SubmitChangeset`. It must resolve to a `:RunTrace` node of the tenant. Medulla stores `agent_identity` and `run_trace_id` on the changeset and writes a `producedByRun` edge to the run.

## Refusal codes

`AGENT_RUN_TRACE_MISSING` and `RUN_TRACE_UNKNOWN`.

## Authoritative types

`Changeset` in `crates/medulla-types/src/source_changeset.rs`.

## Test vectors

`tests/test_scm_provenance.py::test_changeset_links_to_run_trace` is the vector; the harness proves its side against the same request.
