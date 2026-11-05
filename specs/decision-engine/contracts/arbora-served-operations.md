# Seam: served web page selection

Provider: medulla MEDULLA-DECISION-ENGINE-R802.3
Consumer: arbora ARBORA-OPS-R067

## Wire shape

Arbora calls the served method `SelectWebPages` with the request graph and the candidate pages. Medulla reads the graph rows that the caller may see after the graph ACL and returns the selected pages with their evidence. Arbora supplies the verified principal and tenant. Medulla never reads a principal from the request body.

## Refusal codes

Medulla returns `ACCESS_DENIED` when the request tenant differs from the verified tenant. The other stable decision codes are in `MEDULLA-DECISION-ENGINE-R010.100`.

## Authoritative types

The request and result types of `SelectWebPages` in `crates/medulla-protocol-types/src/protocol/`. The selection logic is `select_web_pages` behind `crates/medulla-server-graph/src/dispatch/graph_pipeline/web_ingest.rs`.

## Test vectors

`tests/web_document_enrich_served.rs` holds the served vectors that both sides test against.
