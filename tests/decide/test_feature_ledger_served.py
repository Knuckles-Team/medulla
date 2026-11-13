"""MEDULLA-DECISION-ENGINE-R707.3 served feature ledger: a gap is opened with the
matches it came from, planned, refused when the machine or the revision says so,
and closed by a known change, all through the ``feature_ledger`` client method.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient
from medulla.generated.index_repository import IndexRepositoryScope


def _index_commit(clean_graph: Any, tag: str) -> str:
    """Index one repository whose history holds a commit; return its digest."""
    digest = hashlib.sha1(tag.encode()).hexdigest()
    body = b"def ping():\n    return 1\n"
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": f"repo-{tag}",
            "commits": [
                {
                    "digest": digest,
                    "author": "writer",
                    "timestamp_ms": 1,
                    "parents": [],
                    "changes": [],
                }
            ],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": "c" * 40,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": "pkg/a.py",
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
            ],
        }
    )
    try:
        clean_graph.graph.index_repository([("pkg/a.py", body)], scope=scope)
    except Exception as exc:
        text = str(exc)
        if "AST feature not enabled" in text or "requires durable persistence" in text:
            pytest.skip("engine built without the ast feature or durable persistence")
        raise
    return digest


async def _journey(tag: str, change: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        feature = f"feature-{tag}"
        opened = await decision.feature_ledger(
            dc.feature_ledger_open_op(feature, [f"match:{tag}:a:b"])
        )
        try:
            await decision.feature_ledger(
                dc.feature_ledger_close_op(feature, change, 0)
            )
            early = None
        except Exception as error:  # the typed refusal carries the code
            early = str(error)
        try:
            await decision.feature_ledger(
                dc.feature_ledger_plan_op(f"plan-{tag}", [feature], "sha256:r", 9)
            )
            stale = None
        except Exception as error:
            stale = str(error)
        planned = await decision.feature_ledger(
            dc.feature_ledger_plan_op(f"plan-{tag}", [feature], "sha256:r", 0)
        )
        closed = await decision.feature_ledger(
            dc.feature_ledger_close_op(feature, change, 1)
        )
        read = await decision.feature_ledger(dc.feature_ledger_get_op([feature]))
        return {
            "feature": feature,
            "opened": opened,
            "early": early,
            "stale": stale,
            "planned": planned,
            "closed": closed,
            "read": read,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R707.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R707.3")
def test_ledger_served(clean_graph):
    tag = uuid.uuid4().hex[:12]
    change = _index_commit(clean_graph, tag)
    outcome = asyncio.run(_journey(tag, change))

    (opened,) = outcome["opened"]["entries"]
    assert opened["feature_id"] == outcome["feature"]
    assert (opened["status"], opened["revision"]) == ("open", 0)
    assert opened["match_ids"] == [f"match:{tag}:a:b"]

    assert outcome["early"] is not None
    assert "TRANSITION_REFUSED" in outcome["early"]
    assert outcome["stale"] is not None
    assert "REVISION_CONFLICT" in outcome["stale"]

    (planned,) = outcome["planned"]["entries"]
    assert (planned["status"], planned["revision"]) == ("planned", 1)
    assert planned["plan_id"] == f"plan-{tag}"

    (closed,) = outcome["closed"]["entries"]
    assert (closed["status"], closed["revision"]) == ("closed", 2)
    assert closed["closed_by"] == change
    assert outcome["read"]["entries"] == [closed]
