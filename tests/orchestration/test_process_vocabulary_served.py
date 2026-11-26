"""MEDULLA-ORCH-VOCAB-R014.8 served process vocabulary.

A BPMN user task, an exclusive gateway and two sequence flows are written through
the served ``AddNode`` and ``AddEdge``. A UQL query for the core classes
``:ProcessStep`` and ``:ProcessGate`` returns them through subclass closure. A
sequence flow without a target is refused by the BPMN shapes.
"""

from __future__ import annotations

import uuid

import pytest

CORE = "https://knuckles-team.github.io/ontology/core#"


def _ids(result: dict) -> set[str]:
    assert result["kind"] == "rows", result
    return {row["id"] for row in result["rows"]}


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R014.8")
def test_bpmn_nodes_answer_core_class_queries(clean_graph) -> None:
    tag = uuid.uuid4().hex[:8]
    task, gateway = f"bpmn-task-{tag}", f"bpmn-gateway-{tag}"
    to_gateway, to_task = f"bpmn-flow-in-{tag}", f"bpmn-flow-out-{tag}"

    clean_graph.nodes.add(
        task, {"type": "BpmnUserTask", "bpmnElementId": f"approve-{tag}"}
    )
    clean_graph.nodes.add(
        gateway, {"type": "BpmnExclusiveGateway", "bpmnElementId": f"decide-{tag}"}
    )
    clean_graph.nodes.add(
        to_gateway, {"type": "BpmnSequenceFlow", "bpmnElementId": f"f1-{tag}"}
    )
    clean_graph.nodes.add(
        to_task, {"type": "BpmnSequenceFlow", "bpmnElementId": f"f2-{tag}"}
    )
    clean_graph.edges.add(to_gateway, task, {"relationship": "flowSource"})
    clean_graph.edges.add(to_gateway, gateway, {"relationship": "flowTarget"})
    clean_graph.edges.add(to_task, gateway, {"relationship": "flowSource"})
    clean_graph.edges.add(to_task, task, {"relationship": "flowTarget"})

    # The task is a process step and the gateway a process gate, by subclass closure.
    assert task in _ids(clean_graph.query.uql("MATCH (:ProcessStep)"))
    assert gateway in _ids(clean_graph.query.uql("MATCH (:ProcessGate)"))
    # Neither answers for the other core class.
    assert gateway not in _ids(clean_graph.query.uql("MATCH (:ProcessStep)"))
    assert task not in _ids(clean_graph.query.uql("MATCH (:ProcessGate)"))

    # A sequence flow without a target is refused by the BPMN shapes, and is not stored.
    orphan = f"urn:medulla:probe:flow-{tag}"
    with pytest.raises(Exception, match="flowTarget"):
        clean_graph.rdf.add_triples(
            turtle=(
                f"@prefix : <{CORE}> .\n"
                f'<{orphan}> a :BpmnSequenceFlow ; :bpmnElementId "orphan-{tag}" ; '
                f"  :flowSource <{task}> .\n"
            )
        )
    assert not clean_graph.rdf.sparql(f"SELECT ?p WHERE {{ <{orphan}> ?p ?o }}")
