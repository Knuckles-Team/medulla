"""The canonical feature schema of the ``catalog.*_rank`` decision points.

Rows MEDULLA-DECISION-ENGINE-R829.4. Mirrors ``catalog_rank_feature_schema`` in
``crates/medulla-h-decide/src/decide/features/capability_text.rs`` (the source
of truth); keep the two in step by hand.

A host publishes each entry once per tenant as a ``feature_schema`` component
under its ``component_id``, with ``idempotency_key`` as the publish key, so a
repeat publish replays the same component revision. It then pins the
``definition_digest`` that publish returns. All four points share one body, so
one tenant's pins are reproducible; each point pins its own component id, the
id ``connector_points.rs`` publishes.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any, Final

_FEATURES: Final = (
    "name_overlap",
    "description_overlap",
    "term_rarity_weighted_overlap",
    "name_coverage",
    "semantic_similarity",
)

_BODY: Final[dict[str, Any]] = {
    "schema_version": 1,
    "features": [
        {
            "name": name,
            "kind": {"feature": "number", "key": f"rank.{name}"},
            "missing": {
                "missing": "impute",
                "value": {"scale": "q32", "value": 0},
            },
        }
        for name in _FEATURES
    ],
}

# The compact canonical bytes the engine hashes: JSON in field order, no whitespace.
_DIGEST_HEX: Final = hashlib.sha256(
    json.dumps(_BODY, separators=(",", ":")).encode("utf-8")
).hexdigest()

CATALOG_RANK_QUESTION_IDS: Final = (
    "catalog.skill_rank",
    "catalog.operation_rank",
    "catalog.catalog_item_rank",
    "catalog.intent_action_rank",
)


def _entry(question_id: str) -> dict[str, Any]:
    component_id = f"decide.schema.{question_id}"
    return {
        "component_id": component_id,
        "kind": "feature_schema",
        "schema_version": 1,
        "content_digest": "sha256:" + _DIGEST_HEX,
        "idempotency_key": f"eg:{component_id}:sha256:{_DIGEST_HEX}",
        "body": json.loads(json.dumps(_BODY)),
    }


CATALOG_RANK_FEATURE_SCHEMAS: Final[dict[str, dict[str, Any]]] = {
    question_id: _entry(question_id) for question_id in CATALOG_RANK_QUESTION_IDS
}
