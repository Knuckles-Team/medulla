"""The feature schema of the ``process.*`` control-step decision points.

Row MEDULLA-DECISION-ENGINE-R180.8. Mirrors ``process_control_feature_schema`` in
``crates/medulla-h-decide-learning/src/decide/family/process_decision.rs`` (the source of
truth); keep the two in step by hand.

A host publishes each entry once per tenant as a ``feature_schema`` component under
its ``component_id``, with ``idempotency_key`` as the publish key, so a repeat publish
replays the same component revision. It then pins the ``definition_digest`` that
publish returns. Both control steps share one body: a process decision scores nothing,
so the schema carries the one inert ``declared_cost_micros`` feature (a schema declares
at least one) and no process decision reads it.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any, Final

_BODY: Final[dict[str, Any]] = {
    "schema_version": 1,
    "features": [
        {
            "name": "declared_cost_micros",
            "kind": {"feature": "declared_cost_micros"},
            "missing": {
                "missing": "impute",
                "value": {"scale": "q32", "value": 0},
            },
        }
    ],
}

# The compact canonical bytes the engine hashes: JSON in field order, no whitespace.
_DIGEST_HEX: Final = hashlib.sha256(
    json.dumps(_BODY, separators=(",", ":")).encode("utf-8")
).hexdigest()

PROCESS_CONTROL_QUESTION_IDS: Final = (
    "process.branch_select",
    "process.decision_table_eval",
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


PROCESS_CONTROL_FEATURE_SCHEMAS: Final[dict[str, dict[str, Any]]] = {
    question_id: _entry(question_id) for question_id in PROCESS_CONTROL_QUESTION_IDS
}
