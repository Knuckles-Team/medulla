"""MEDULLA-ORCH-VOCAB-R009.701 served receipt: durable units are listed by
status after a resume, and a unit without a run is refused."""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError


def _unit(unit_id: str, status: str, artifacts: list[str]) -> dict:
    return {
        "node_type": "DurableUnit",
        "unit_id": unit_id,
        "run_id": "trace-d",
        "status": status,
        "artifact_refs": artifacts,
    }


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.701")
def test_resume_finds_produced_artifacts(clean_graph):
    clean_graph.nodes.add("trace-d", {"node_type": "RunTrace"})
    clean_graph.nodes.add("unit-1", _unit("unit-1", "completed", ["cas:a"]))
    clean_graph.nodes.add("unit-2", _unit("unit-2", "completed", ["cas:b"]))
    clean_graph.nodes.add("unit-3", _unit("unit-3", "in_flight", []))
    units = sorted(
        (
            (node_id, clean_graph.nodes.properties(node_id))
            for node_id in ("unit-1", "unit-2", "unit-3")
        ),
        key=lambda item: item[0],
    )
    completed = [props for _, props in units if props["status"] == "completed"]
    assert [props["artifact_refs"] for props in completed] == [["cas:a"], ["cas:b"]]
    with pytest.raises(EngineResponseError, match="DURABLE_UNIT_RUN_CONFLICT"):
        clean_graph.nodes.add(
            "unit-1", {**_unit("unit-1", "completed", ["cas:a"]), "run_id": "other"}
        )


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.701")
def test_unit_without_run_id_is_refused(clean_graph):
    unit = _unit("unit-x", "in_flight", [])
    del unit["run_id"]
    with pytest.raises(EngineResponseError, match="DURABLE_UNIT_RUN_MISSING"):
        clean_graph.nodes.add("unit-x", unit)
    assert clean_graph.nodes.has("unit-x") is False
