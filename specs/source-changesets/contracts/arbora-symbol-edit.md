# Seam: symbol edit serving

Provider: medulla MEDULLA-SCM-R013.4
Consumer: arbora ARBORA-OPS-R848.4

## Wire shape

Arbora sends `Method::SubmitSymbolEdit { request }` with `SymbolEdit { repo, branch, base_rev, ops, message, run_trace_id }`. Each op is `{kind, symbol_id, anchor_span_digest, new_text}` with `kind` one of `replace_symbol`, `replace_body`, `insert_before`, `insert_after`, `delete`. Medulla projects the edit to file bytes, re-parses, and lands one changeset or refuses. The harness row `MERISTEM-SCM-R002` applies the answer and holds no projection code.

## Refusal codes

`EDIT_BASE_STALE`, `EDIT_OPS_OVERLAP`, `EDIT_UNPARSEABLE`, `EDIT_ROUNDTRIP_MISMATCH`, `EDIT_SYMBOL_UNKNOWN`, `EDIT_NO_OPS` and `EDIT_BLOB_UNREADABLE`. A refusal lands nothing.

## Authoritative types

`SymbolEdit`, `SymbolEditOp` and `SymbolEditKind` in `crates/medulla-types/src/source_symbol_edit.rs`.

## Test vectors

`tests/test_scm_symbol_edit_served.py::test_symbol_edit_lands_and_reindexes` and `tests/test_scm_symbol_edit_served.py::test_symbol_edit_refusals` are the vectors; Arbora proves its call against the same request and refusal codes.
