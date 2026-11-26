"""The canonical task-plan feature schema (MEDULLA-DECISION-ENGINE-R134.111.7).

Mirrors ``medulla_types::decision::task_plan::task_plan_feature_schema`` (the source of
truth); keep the two in step by hand.

A host publishes ``body`` once per tenant as a ``feature_schema`` component
under ``component_id``, with ``idempotency_key`` as the publish key, so a
repeat publish replays the same component revision. It then pins the
``definition_digest`` that publish returns.
"""

from __future__ import annotations

import json
from typing import Any, Final

_BODY = """
{
  "schema_version": 1,
  "features": [
    {
      "name": "task_position",
      "kind": {
        "feature": "number",
        "key": "task_position"
      },
      "missing": {
        "missing": "abstain"
      }
    },
    {
      "name": "task_call",
      "kind": {
        "feature": "number",
        "key": "task_call"
      },
      "missing": {
        "missing": "abstain"
      }
    },
    {
      "name": "task_call_bindings",
      "kind": {
        "feature": "number",
        "key": "task_call_bindings"
      },
      "missing": {
        "missing": "abstain"
      }
    },
    {
      "name": "task_step_kind",
      "kind": {
        "feature": "number",
        "key": "task_step_kind"
      },
      "missing": {
        "missing": "abstain"
      }
    },
    {
      "name": "task_foreach",
      "kind": {
        "feature": "number",
        "key": "task_foreach"
      },
      "missing": {
        "missing": "abstain"
      }
    }
  ]
}
"""

_DIGEST_HEX = "a7e48379bf6f5e14677013cceff43be895050f21ee0cbf84ded95f75f7709017"

TASK_PLAN_FEATURE_SCHEMA: Final[dict[str, Any]] = {
    "component_id": "task-plan-features",
    "kind": "feature_schema",
    "schema_version": 1,
    "content_digest": "sha256:" + _DIGEST_HEX,
    "idempotency_key": "eg:task-plan-feature-schema:sha256:" + _DIGEST_HEX,
    "body": json.loads(_BODY),
}
