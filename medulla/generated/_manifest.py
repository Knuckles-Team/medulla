# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""The wire-vector manifest the Meristem lifecycle consumes.

``MANIFEST`` holds ``contract_revision`` and the exported type ids: the request
tags of every ``models.Method`` variant, the types the contract publishes to the
python profile. No vector file is committed, so no digest over vector files is
pinned here. The run-time contract identity a consumer binds is
``catalog_digest`` from ``medulla.engine.method_catalog()`` (MEDULLA-CONTRACT-R057).
``tests/test_wire_vectors.py`` (R001.4, R001.4.1) reads the vectors from the one
Rust emitter at test time.
"""

from __future__ import annotations

from typing import Any


def exported_type_ids() -> tuple[str, ...]:
    """The request tags of every ``models.Method`` variant, sorted.

    The tag is read from the variant's raw class annotation, not its model
    fields: the variants defer their schema build, and touching ``model_fields``
    mid-import would build them before the module finishes loading. Under
    ``from __future__ import annotations`` the annotation is the source text
    ``Literal['Tag']``, so the tag is read from that text.
    """
    import re
    from typing import get_args

    from . import models

    union = get_args(models.Method)[0]
    tags = []
    for cls in get_args(union):
        annotation = cls.__annotations__.get("method")
        if isinstance(annotation, str):
            found = re.fullmatch(
                r"Literal\[\s*['\"]([^'\"]+)['\"]\s*\]", annotation.strip()
            )
            if found is None:
                raise TypeError(f"{cls.__name__}.method is not a single literal tag")
            tags.append(found.group(1))
        else:
            (tag,) = get_args(annotation)
            tags.append(tag)
    return tuple(sorted(tags))


MANIFEST: dict[str, Any] = {
    "contract_revision": "1",
    "exported_types": exported_type_ids(),
}
