# MEDULLA-OBJECT-STORAGE-BACKEND — Data model

This file describes the entities of the backend spec and of the object-root layout.

## Entities

### StorageBackendSpec

- Variants: `local` (default) and `object_store { endpoint, bucket, root, durability, layout_digest }`.
- Owner: `crates/medulla-storage/src/backend_spec.rs`. Endpoint, bucket and root are `SecretRef` values, never values.
- Relation: `layout_digest` names an entry of the owner-store lineage registry.

### VersionManifest

- Fields: `schema_version`, `tenant`, `graph`, `version`, `parent_version`, `snapshot`, `batches`, `digest`.
- Owner: new `crates/medulla-storage/src/object/manifest.rs`. Stored at `<root>/<tenant>/<graph>/manifests/<version:020>.json`; immutable.

### SegmentRef and BatchSegment

- `SegmentRef`: `{ key, bytes, sha256 }` with 64 lowercase hex characters. Segments live at `segments/<sha256>.seg` and are written once.
- `BatchSegment`: a MessagePack list of `MutationBatchRecord`.

### HEAD

- Fields: `{version, manifest}`. The one mutable object; updated only by a conditional put on its current version tag.

### ArchiveSegment

- Key: `<root>/archive/<tenant>/<graph>/<first:020>-<last:020>.seg`; immutable; digest in its manifest entry.

### CostRecord

- Fields: `schema_version`, `host`, `revision`, `lifecycle_digest`, `dimension`, `value`. One JSON line per dimension: time, object actions, bytes, peak memory.
