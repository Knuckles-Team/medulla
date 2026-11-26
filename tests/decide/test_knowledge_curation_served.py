"""MEDULLA-DECISION-ENGINE-R709.6 served knowledge-curation journey.

L4: every call goes through the running engine over the wire client. Two open flags and
one run trace are written through ``KnowledgeFlag`` and ``BatchUpdate``. The curator
claims the curation loop, reads ``Inputs`` and proposes an edit that renames a column in
a data-access entry and in a skill. The first proposal is admitted as an evaluated
revision and a second, conflicting proposal is refused with
``CONTRADICTS_ACTIVE_DEFINITION``. Promotion of the admitted revision through the
champion pointer is proved by the L3 journey (``adoption_closes_the_addressed_flags``),
because it needs a held-out receipt this test does not fabricate.
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

GRAPH = "__commons__"


def _digest(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _proposal(
    prefix: str, name: str, flags: list[str], after: str, body: str, trace: str
) -> dict[str, Any]:
    return {
        "proposal_id": f"{prefix}-{name}",
        "flag_ids": flags,
        "target_ref": f"source:{prefix}-orders",
        "knowledge_kind": "data_access",
        "edit": {
            "op": "update",
            "after_digest": after,
            "body_digest": _digest(body),
            "references": [f"{prefix}.amount"],
        },
        "rationale_trace_ids": [trace],
        "evidence_digest": _digest("evidence"),
    }


async def _journey(prefix: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )
    try:
        run_id, trace = f"{prefix}-run", f"{prefix}-trace"
        await client.lifecycle.batch_update(
            [
                {
                    "op": "upsert_node",
                    "id": trace,
                    "properties": {"node_type": "RunTrace", "run_id": run_id},
                },
                {
                    "op": "upsert_node",
                    "id": f"source:{prefix}-orders",
                    "properties": {"type": "DataSource"},
                },
                {
                    "op": "upsert_node",
                    "id": f"{prefix}.amount",
                    "properties": {"type": "Column"},
                },
            ]
        )
        decisions = dc.DecisionClient(client=client, graph=GRAPH)
        flags = []
        for index, topic in enumerate(("refunds", "exchanges")):
            raised = await decisions.knowledge_flag(
                dc.knowledge_flag_raise_op(
                    GRAPH,
                    "data_access",
                    "missing",
                    f"topic:{prefix}-{topic}",
                    f"{topic} is not documented",
                    run_id,
                    1_700_000_000_000 + index,
                    trace_node_id=trace,
                )
            )
            flags.append(raised["flag"]["flag_id"])
        claim = await decisions.loops(
            {
                "op": "claim",
                "loop_id": f"{prefix}-curation",
                "objective_digest": "sha256:curate",
                "priority": 3,
            }
        )
        epoch = claim["instance"]["lease"]["epoch"]
        loop_id = f"{prefix}-curation"
        inputs = await decisions.knowledge_curation(
            dc.knowledge_curation_inputs_op(GRAPH, loop_id, epoch, force=True)
        )
        body = "the amount column is renamed to total"
        first = await decisions.knowledge_curation(
            dc.knowledge_curation_propose_op(
                GRAPH,
                loop_id,
                epoch,
                _proposal(prefix, "first", flags, _digest("v2"), body, trace),
                body,
            )
        )
        refused = None
        other = "the amount column is renamed to gross"
        try:
            await decisions.knowledge_curation(
                dc.knowledge_curation_propose_op(
                    GRAPH,
                    loop_id,
                    epoch,
                    _proposal(prefix, "second", flags, _digest("v3"), other, trace),
                    other,
                )
            )
        except Exception as error:  # the wire client raises the engine refusal
            refused = str(error)
        status = await decisions.knowledge_curation(
            dc.knowledge_curation_status_op(GRAPH, f"{prefix}-first")
        )
        listed = await decisions.knowledge_flag(
            dc.knowledge_flag_list_op(GRAPH, state="open")
        )
        return {
            "flags": flags,
            "inputs": inputs,
            "first": first,
            "refused": refused,
            "status": status,
            "listed": listed,
        }
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R709.6")
def test_flag_trace_to_fixed_skill_and_entry(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R709.6
    prefix = "kc" + uuid.uuid4().hex[:8]
    out = asyncio.run(_journey(prefix))

    served = {flag["flag_id"] for flag in out["inputs"]["flags"]}
    assert set(out["flags"]) <= served
    assert any(t["run_id"] == f"{prefix}-run" for t in out["inputs"]["traces"])

    # The first proposal is admitted as an evaluated revision.
    assert out["first"]["stage"] == "evaluated"
    assert out["status"]["stage"] == "evaluated"

    # The second, conflicting proposal is refused and changes nothing.
    assert out["refused"] is not None
    assert "CONTRADICTS_ACTIVE_DEFINITION" in out["refused"]
    open_ids = {flag["flag_id"] for flag in out["listed"]["flags"]}
    assert set(out["flags"]) <= open_ids
