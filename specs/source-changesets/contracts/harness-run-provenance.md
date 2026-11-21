# Seam: agent run provenance on a changeset

Provider: medulla MEDULLA-SCM-R012
Consumer: meristem MERISTEM-SCM-R001

## Wire shape

A delegated agent request passes `run_trace_id` in `SubmitChangeset`. The harness must first write that run node, with `kind` `run_trace`, into the graph named by the request `repo`. Medulla checks that the caller can read the node and stores `agent_identity` and `run_trace_id` on the sealed changeset. The derived lifecycle edge `producedBy` (MEDULLA-SDLC-R001.2) joins `commit:<changeset id>` to the run node in that graph.

## Refusal codes

`AGENT_RUN_TRACE_MISSING` and `RUN_TRACE_UNKNOWN`.

## Authoritative types

`Changeset` in `crates/medulla-source-types/src/source_changeset.rs`.

## Test vectors

`tests/test_scm_provenance.py::test_changeset_links_to_run_trace` is the vector; the harness proves its side against the same request.
