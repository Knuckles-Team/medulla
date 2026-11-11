# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""The wire-vector manifest the Meristem lifecycle consumes.

``MANIFEST`` holds ``contract_revision``, the exported type ids and
``schema_digest``: a SHA-256, in 64 lowercase hex characters, over the sorted
wire-vector files under ``tests/fixtures/wire_vectors/`` -- each file name
(NUL-terminated) followed by its bytes. ``tests/test_wire_vectors.py``
(R001.4) recomputes the digest from the vector files, so a stale manifest
fails the gate. The exported type ids are the request tags of every
``models.Method`` variant, the types the contract publishes to the python
profile.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

VECTOR_DIR = Path(__file__).resolve().parents[2] / "tests" / "fixtures" / "wire_vectors"


def vector_schema_digest(vector_files: list[Path]) -> str:
    """The SHA-256 over the sorted vector files: name (NUL) + bytes, per file."""
    digest = hashlib.sha256()
    for path in sorted(vector_files, key=lambda item: item.name):
        digest.update(path.name.encode("utf-8"))
        digest.update(b"\x00")
        digest.update(path.read_bytes())
    return digest.hexdigest()


def exported_type_ids() -> tuple[str, ...]:
    """The request tags of every ``models.Method`` variant, sorted.

    The tag is read from the variant's raw class annotation, not its model
    fields: the variants defer their schema build, and touching ``model_fields``
    mid-import would build them before the module finishes loading.
    """
    from typing import get_args

    from . import models

    union = get_args(models.Method)[0]
    tags = []
    for cls in get_args(union):
        args = get_args(cls.__annotations__.get("method"))
        tags.append(args[0] if args else str(cls.__annotations__.get("method")))
    return tuple(sorted(tags))


MANIFEST: dict[str, Any] = {
    "contract_revision": "1",
    "exported_types": exported_type_ids(),
    # SHA-256 over the sorted wire-vector files; regenerated with the vectors.
    "schema_digest": "8a978e0ac30ff766b36547cf6bd68c0f7782bf2eb65591252cc8cec230166cdb",
}
