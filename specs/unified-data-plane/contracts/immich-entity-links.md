# Seam: cross-application entity links

Provider: medulla MEDULLA-UNIFIED-DATA-PLANE-R030.3
Consumer: immich-api CONN-IMMICH-API-R001

## Wire shape

The connector ingests per-user people and assets incrementally and emits cross-application Person links as proposals. Medulla scores candidate pairs by blocking, field similarity and Fellegi–Sunter weights, stores each proposal with its evidence, and writes an `owl:sameAs` edge only after an operator approves it. Per-user media, location and face data never cross user scope.

## Refusal codes

`DENIED` for an unapproved or non-match assertion and for a read outside the caller's user scope. An ambiguous pair stays a proposal.

## Authoritative types

`EntityLinkProposal` in `crates/medulla-types/src/entity_link_proposal.rs`.

## Test vectors

The served test `tests/data_plane/test_entity_link_served.py::test_no_sameas_before_approval` and the four-user synthetic fixture are the vectors: a second ingest is a no-op, one new upload appears, unnamed people stay unlinked and a cross-user read is denied.
