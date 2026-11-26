"""MEDULLA-CQ-ORCH-R701.9 served cross-source equivalence.

L4: one invoice approval process authored as BPMN 2.0 and as an EPC canonical form
enters through the wire client (``process_document_import`` then ``process_compile``,
and ``process_model_import``), and ``Decide`` with the ``process_plan`` question gives
both stored workflows one plan digest.
"""

from __future__ import annotations

import asyncio
import json
import os
import uuid
from pathlib import Path
from typing import Any
from xml.etree import ElementTree

import pytest
from _decision_resident_fixture import publish, schema_body
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

FIXTURES = (
    Path(__file__).resolve().parents[2]
    / "crates/medulla-core/tests/fixtures/process_equivalence"
)


_KINDS = ("call", "branch", "join", "decision", "approval")


def _bpmn_process_element_id(document: bytes) -> str:
    """The id of the BPMN process element, read from the imported document itself."""
    for element in ElementTree.fromstring(document).iter():
        if element.tag.rsplit("}", 1)[-1] == "process":
            return element.attrib["id"]
    raise AssertionError("the BPMN fixture has no process element")


def _shape(plan: dict[str, Any]) -> list[tuple[str, str, list[str]]]:
    """Per task: its id, its kind and its sorted dependencies."""
    return [
        (
            task["task_id"],
            next((kind for kind in _KINDS if task.get(kind) is not None), "prompt"),
            sorted(task.get("depends_on", [])),
        )
        for task in plan["tasks"]
    ]


async def _connect(graph: str) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )


async def _import_both(
    graph: str, suffix: str
) -> tuple[dict[str, Any], dict[str, Any], str]:
    client = await _connect(graph)
    try:
        model_iri = f"urn:model:invoice-{suffix}"
        document = (FIXTURES / "invoice-approval.bpmn").read_bytes()
        await client.graph.process_document_import(
            {"format": "bpmn20", "document": document}, model_iri, 0
        )
        process_iri = f"process:{model_iri}:{_bpmn_process_element_id(document)}"
        bpmn = await client.graph.process_compile(process_iri, 0)
        epc_workflow = json.loads(
            (FIXTURES / "invoice-approval.epc.canonical.json").read_text()
        )
        epc = await client.graph.process_model_import(
            epc_workflow, f"invoice-epc-{suffix}", 0
        )
        return bpmn, epc, process_iri
    finally:
        await client.close()


async def _plan(
    schema: dict[str, Any], graph: str, workflow_id: str, version: int
) -> Any:
    client = await _connect(graph)
    try:
        request = dc.process_plan_request(
            TEST_TENANT,
            f"q:r701-9:{workflow_id}:{version}",
            schema,
            graph=graph,
            workflow_id=workflow_id,
            version=version,
        )
        payload = await dc.DecisionClient(client=client).decide(request)
        return payload["records"][0]["answer"]["value"]
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R701.9")
def test_bpmn_and_epc_plan_digests_match(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R701.9
    suffix = uuid.uuid4().hex[:8]
    graph = _graph_name(clean_graph)
    schema = publish(
        clean_graph, f"r701-9-schema-{suffix}", "feature_schema", schema_body()
    )
    golden = json.loads((FIXTURES / "invoice-approval.canonical.json").read_text())

    bpmn, epc, process_iri = asyncio.run(_import_both(graph, suffix))
    assert bpmn["workflow_digest"] == "sha256:" + golden["digest"]
    assert epc["workflow_digest"] == "sha256:" + golden["digest"]

    bpmn_plan = asyncio.run(_plan(schema, graph, process_iri, bpmn["version"]))
    epc_plan = asyncio.run(
        _plan(schema, graph, f"invoice-epc-{suffix}", epc["version"])
    )
    assert bpmn_plan["plan_digest"] == epc_plan["plan_digest"]
    assert len(bpmn_plan["tasks"]) == len(epc_plan["tasks"])
    assert _shape(bpmn_plan) == _shape(epc_plan)
