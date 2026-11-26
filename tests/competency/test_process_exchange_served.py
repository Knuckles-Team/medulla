"""MEDULLA-CQ-ORCH-R702.14 and R702.15 served process document round trips.

L4: every call goes through the running engine over the wire client. A BPMN
document with two lanes, an exclusive gateway with a default flow, a parallel
split and join, a timer boundary event and one extension element, and a DMN
document with UNIQUE, FIRST and COLLECT SUM tables, are each imported, exported
and imported again into a second model. The canonical model digests are equal.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import MedullaClient

GRAPH = "__commons__"

BPMN = b"""<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
    xmlns:ext="urn:example:extension" id="defs1">
  <bpmn:collaboration id="collab1">
    <bpmn:participant id="p1" processRef="proc1"/>
  </bpmn:collaboration>
  <bpmn:process id="proc1" name="Invoice">
    <bpmn:laneSet id="ls1">
      <bpmn:lane id="lane1" name="Finance">
        <bpmn:flowNodeRef>user1</bpmn:flowNodeRef>
        </bpmn:lane>
      <bpmn:lane id="lane2" name="Systems">
        <bpmn:flowNodeRef>service1</bpmn:flowNodeRef>
        </bpmn:lane>
    </bpmn:laneSet>
    <bpmn:startEvent id="start1" name="Start"/>
    <bpmn:userTask id="user1" name="Review">
      <bpmn:extensionElements>
      <ext:form key="f1"/>
      </bpmn:extensionElements>
      </bpmn:userTask>
    <bpmn:serviceTask id="service1" name="Post"/>
    <bpmn:exclusiveGateway id="xor1" name="Amount?" default="flow4"/>
    <bpmn:parallelGateway id="split1"/>
    <bpmn:parallelGateway id="join1"/>
    <bpmn:boundaryEvent id="timer1" attachedToRef="user1">
      <bpmn:timerEventDefinition id="td1">
      <bpmn:timeDuration>PT1H</bpmn:timeDuration>
      </bpmn:timerEventDefinition>
      </bpmn:boundaryEvent>
    <bpmn:endEvent id="end1" name="End"/>
    <bpmn:sequenceFlow id="flow1" sourceRef="start1" targetRef="xor1"/>
    <bpmn:sequenceFlow id="flow2" name="high" sourceRef="xor1" targetRef="user1">
      <bpmn:conditionExpression>amount &gt; 1000</bpmn:conditionExpression>
      </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="flow3" name="mid" sourceRef="xor1" targetRef="split1">
      <bpmn:conditionExpression>amount &gt; 100</bpmn:conditionExpression>
      </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="flow4" sourceRef="xor1" targetRef="service1"/>
    <bpmn:sequenceFlow id="flow5" sourceRef="split1" targetRef="user1"/>
    <bpmn:sequenceFlow id="flow6" sourceRef="split1" targetRef="service1"/>
    <bpmn:sequenceFlow id="flow7" sourceRef="user1" targetRef="join1"/>
    <bpmn:sequenceFlow id="flow8" sourceRef="service1" targetRef="join1"/>
    <bpmn:sequenceFlow id="flow9" sourceRef="join1" targetRef="end1"/>
  </bpmn:process>
</bpmn:definitions>
"""

DMN = b"""<?xml version="1.0"?>
<definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/"
    id="defs" name="Limits">
  <inputData id="amount" name="Amount"/>
  <decision id="tier" name="Tier">
    <informationRequirement><requiredInput href="#amount"/></informationRequirement>
    <informationRequirement><requiredDecision href="#base"/></informationRequirement>
    <decisionTable id="t1" hitPolicy="COLLECT" aggregation="SUM">
      <input id="i1" label="Amount">
        <inputExpression typeRef="number">
        <text>amount</text>
        </inputExpression>
        </input>
      <output id="o1" name="fee" typeRef="number"/>
      <rule id="r1">
        <inputEntry>
        <text>&lt; 100</text>
        </inputEntry>
        <outputEntry>
        <text>1.5</text>
        </outputEntry>
        </rule>
      <rule id="r2">
        <inputEntry>
        <text>&gt;= 100</text>
        </inputEntry>
        <outputEntry>
        <text>2</text>
        </outputEntry>
        </rule>
    </decisionTable>
  </decision>
  <decision id="base" name="Base">
    <decisionTable id="t2" hitPolicy="FIRST">
      <input id="i2"><inputExpression><text>x</text></inputExpression></input>
      <output id="o2" name="y"/>
      <rule id="r3">
        <inputEntry>
        <text>1</text>
        </inputEntry>
        <outputEntry>
        <text>"a"</text>
        </outputEntry>
        </rule>
      <rule id="r4">
        <inputEntry>
        <text>2</text>
        </inputEntry>
        <outputEntry>
        <text>"b"</text>
        </outputEntry>
        </rule>
    </decisionTable>
  </decision>
  <decision id="unique" name="Unique">
    <decisionTable id="t3" hitPolicy="UNIQUE">
      <input id="i3"><inputExpression><text>z</text></inputExpression></input>
      <output id="o3" name="w"/>
      <rule id="r5">
        <inputEntry>
        <text>1</text>
        </inputEntry>
        <outputEntry>
        <text>"p"</text>
        </outputEntry>
        </rule>
      <rule id="r6">
        <inputEntry>
        <text>2</text>
        </inputEntry>
        <outputEntry>
        <text>"q"</text>
        </outputEntry>
        </rule>
    </decisionTable>
  </decision>
</definitions>
"""


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )


async def _round_trip(
    document: bytes, format: str, prefix: str
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Import, export, import the export into a second model, export again."""
    client = await _connect()
    try:
        first_iri, second_iri = f"urn:{prefix}:first", f"urn:{prefix}:second"
        await client.graph.process_document_import(
            {"format": format, "document": document}, first_iri, 0
        )
        first = await client.graph.process_document_export(
            {"kind": "model", "model_iri": first_iri}, format
        )
        await client.graph.process_document_import(first["document"], second_iri, 0)
        second = await client.graph.process_document_export(
            {"kind": "model", "model_iri": second_iri}, format
        )
        return first, second
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R702.14")
def test_bpmn_round_trip_keeps_digest(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R702.14
    prefix = f"pe{uuid.uuid4().hex[:8]}"
    first, second = asyncio.run(_round_trip(BPMN, "bpmn20", prefix))
    assert len(first["canonical_model_digest"]) == 64
    assert second["canonical_model_digest"] == first["canonical_model_digest"]
    assert second["document_digest"] == first["document_digest"]


# proves R702.15: DMN round-trip hot-path proof
@pytest.mark.spec("MEDULLA-CQ-ORCH-R702.15")
def test_dmn_round_trip_keeps_digest(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R702.15
    prefix = f"pe{uuid.uuid4().hex[:8]}"
    first, second = asyncio.run(_round_trip(DMN, "dmn13", prefix))
    assert len(first["canonical_model_digest"]) == 64
    assert second["canonical_model_digest"] == first["canonical_model_digest"]
    assert second["document"]["document"] == first["document"]["document"], (
        "rule order is kept"
    )
