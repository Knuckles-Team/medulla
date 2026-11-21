# Seam: git export of wants and haves

Provider: medulla MEDULLA-SCM-R014
Consumer: arbora ARBORA-OPS-R851

## Wire shape

Arbora sends `GitExportPackRequest { repo, branch, wants, haves }`. Medulla returns `GitExportPackReceipt { object_format, pack_ref, pack_bytes }`: `pack_ref` is the content-addressed blob ref `cas:sha256:<digest>` of the version-2 pack, read through the existing blob read; the bytes are never in the reply. Commits are recreated with the same tree hash as the import.

## Refusal codes

`PATH_READ_DENIED` (MEDULLA-SCM-R007) when one path of the request is outside the caller's read grant; the whole request is refused. `REVISION_NOT_ON_CHAIN` for a want that is not a changeset of the branch (an unknown have is ignored).

## Authoritative types

`Changeset` and `GitOrigin` in `crates/medulla-source-types/src/source_changeset.rs`.

## Test vectors

The served export test of `MEDULLA-SCM-R014` clones the chain with a git client and compares the tree hash; one denied path returns `PATH_READ_DENIED`.
