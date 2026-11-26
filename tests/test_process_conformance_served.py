"""Served process conformance: MEDULLA-REPO-INGEST-R002.651.4.

`ProcessConformance` replays traces on the footprint of a stored `:ProcessModel` (the
node `MineProcess` writes back) and scores the fit in thousandths. A conforming trace
scores 1000, a trace with a reversed step scores below, and an absent model refuses
`PROCESS_MODEL_UNKNOWN`.
"""

from __future__ import annotations

import pytest

_LOG = [
    ["register", "check", "decide", "accept"],
    ["register", "check", "decide", "reject"],
]


def _stored_model(client) -> str:
    client._send(
        "MineProcess",
        {"traces": _LOG, "process_id": "loan", "writeback": True},
    )
    models = [node_id for node_id, _ in client.nodes.list_by_label("ProcessModel", 0)]
    assert len(models) == 1, models
    return models[0]


# proves R002.651.4: Process conformance: footprint replay against a stored process ...
@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.651.4")
def test_conforming_trace_scores_one_deviating_below(clean_graph) -> None:
    model = _stored_model(clean_graph)

    conforming = clean_graph._send(
        "ProcessConformance",
        {
            "process_model_id": model,
            "traces": [["register", "check", "decide", "accept"]],
        },
    )
    assert conforming["fitness_milli"] == 1000
    assert conforming["traces"] == 1
    assert conforming["process_id"] == model

    deviating = clean_graph._send(
        "ProcessConformance",
        {
            "process_model_id": model,
            "traces": [["register", "decide", "check", "accept"]],
        },
    )
    assert deviating["fitness_milli"] < 1000
    assert deviating["missing_tokens"] >= 1

    with pytest.raises(Exception, match="PROCESS_MODEL_UNKNOWN"):
        clean_graph._send(
            "ProcessConformance",
            {"process_model_id": "process_model:absent", "traces": [["register"]]},
        )
