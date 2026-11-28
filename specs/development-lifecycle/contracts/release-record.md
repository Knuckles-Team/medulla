# Release record seam with the arbora connector release recording

Provider: medulla MEDULLA-SDLC-R002
Provider: medulla MEDULLA-SDLC-R002.3
Consumer: arbora ARBORA-HOST-R037
Consumer: arbora ARBORA-HOST-R037.1

## Wire shape

`RecordRelease` takes `release` with `repo`, `version`, `git_ref`, `changeset_head`, `artifact_digests` and `published_at`. Arbora records a connector release from the package index, which names no source commit, so it sends empty `git_ref` and `changeset_head`. The engine seals the record as the node `release:<repo>:<version>`, stores the whole record as the JSON text property `release_record`, links no changeset and answers a node read as MessagePack bytes. A named `changeset_head` must be on the repository chain.

## Refusal codes

- `RELEASE_HEAD_UNKNOWN` for a named head that is not on the chain, never for an empty head.
- `RELEASE_VERSION_TAKEN` for a different record of the same repository and version. An identical replay succeeds.

## Authoritative types

- `crates/medulla-source-types/src/lifecycle.rs` `Release` and `handle_record_release`.

## Test vectors

- `crates/medulla-types/tests/lifecycle_records.rs::unknown_head_and_taken_version_refuse` asserts both refusals and that a release with an empty head contains no changeset.

## Owner rows

- Provider: medulla MEDULLA-SDLC-R002 and MEDULLA-SDLC-R002.3.
- Consumer: arbora ARBORA-HOST-R037 and ARBORA-HOST-R037.1.
