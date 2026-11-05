# Seam: git export of wants and haves

Provider: medulla MEDULLA-SCM-R014
Consumer: arbora ARBORA-OPS-R851

## Wire shape

Arbora sends `GitExportPack` with a repository, wants and haves. Medulla returns pack bytes for the changeset chain, recreating commits with the same tree hash as the import.

## Refusal codes

`SCM_PATH_DENIED` when one path of the request is outside the caller's read grant; the whole request is refused.

## Authoritative types

`Changeset` and `GitOrigin` in `crates/medulla-types/src/source_changeset.rs`.

## Test vectors

The served export test of `MEDULLA-SCM-R014` clones the chain with a git client and compares the tree hash; one denied path returns `SCM_PATH_DENIED`.
