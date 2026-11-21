"""One federated read over two virtual graphs and engine facts
(MEDULLA-CQ-CONN-R090).

A caller reads a source only through a completed host-driven read of it, under a
mapping admitted by ``ConnectorPackOp::ApproveSource``. The statement names two
virtual graphs; only the admitted and completed one is read, the other is replaced
by an empty table and listed as unconnected. Each foreign row cites its source;
the engine row carries no provenance.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "connector_competency"))

from conftest import request_context  # noqa: E402
from fixture_source import FixtureSource  # noqa: E402
from host_read import completed_read  # noqa: E402

from medulla.client import MedullaClient  # noqa: E402

OPERATION = "list_items"
STATEMENT = (
    "MATCH (:Service) |> UNION [MATCH (:Service)] "
    "[FOREIGN SCAN 'vg-alpha'] [FOREIGN SCAN 'vg-beta']"
)


def _fixture(tmp_path: Path, name: str, items: list[dict[str, str]]) -> FixtureSource:
    recordings = tmp_path / name
    recordings.mkdir()
    (recordings / f"{OPERATION}.json").write_text(
        json.dumps({"items": items}), encoding="utf-8"
    )
    return FixtureSource("mcp_tool", recordings, operations=[OPERATION]).start()


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


@pytest.mark.spec("MEDULLA-CQ-CONN-R090")
def test_two_virtual_graphs_and_engine_facts_cite_each_source_under_per_source_access(
    clean_graph, tmp_path
) -> None:
    clean_graph.nodes.add("svc:checkout", {"type": "Service", "name": "checkout"})
    alpha = _fixture(tmp_path, "alpha", [{"id": "alpha-1", "label": "Alpha"}])
    beta = _fixture(tmp_path, "beta", [{"id": "beta-1", "label": "Beta"}])
    answers: list[dict[str, Any]] = []

    async def exercise() -> None:
        client = await _connect()
        try:
            prepared = await client.query.uql(STATEMENT, prepare=True)
            assert prepared["kind"] == "explain", prepared
            digests = {
                "plan_digest": prepared["plan_digest"],
                "query_digest": prepared["query_digest"],
            }
            execution = await completed_read(
                client,
                kind="mcp_tool",
                connector="vg-alpha",
                source_id="vg-alpha",
                fixture=alpha,
                **digests,
            )
            # The unadmitted mapping refuses at the first lifecycle step that binds it.
            with pytest.raises(Exception, match="SOURCE_APPROVAL_NOT_ADMITTED"):
                await completed_read(
                    client,
                    kind="mcp_tool",
                    connector="vg-beta",
                    source_id="vg-beta",
                    fixture=beta,
                    approve=False,
                    **digests,
                )
            answers.append(await client.query.uql(STATEMENT, foreign_executions=[execution]))
        finally:
            await client.close()

    try:
        asyncio.run(exercise())
    finally:
        alpha.close()
        beta.close()

    answer = answers[0]
    assert answer["kind"] == "rows", answer
    rows = {row["id"]: row for row in answer["rows"]}
    assert set(rows) == {"svc:checkout", "alpha-1"}, rows
    assert rows["alpha-1"]["provenance"]["connector"] == "vg-alpha"
    assert not rows["svc:checkout"].get("provenance")
    assert "beta-1" not in rows
    assert answer["source_coverage"] == [
        {"source": "vg-alpha", "used": True},
        {"source": "vg-beta", "used": False, "reason": "unconnected"},
    ]


@pytest.mark.spec("MEDULLA-CQ-CONN-R090")
def test_write_stage_refuses_before_any_source_is_read(clean_graph, tmp_path) -> None:
    alpha = _fixture(tmp_path, "alpha", [{"id": "alpha-1", "label": "Alpha"}])

    async def exercise() -> None:
        client = await _connect()
        try:
            with pytest.raises(Exception, match="UQL_UNKNOWN_STAGE"):
                await client.query.uql("MATCH (:Service) |> DELETE", prepare=True)
        finally:
            await client.close()

    try:
        asyncio.run(exercise())
        # No plan digest was issued, so no read could bind and no fetch happened.
        assert alpha.request_count == 0
    finally:
        alpha.close()
