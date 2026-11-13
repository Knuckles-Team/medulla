"""MEDULLA-DECISION-ENGINE-R135.15 hot-path proof: after a run, insights are
written through the served `DecisionLog` (`run_record` / `insight`) and a later
`Decide` of kind `insight_selection` selects them with their source runs.

The served engine admits only its deployment tenant, so the other-tenant read is
proven by the Rust L3 test
`stat_tests/insight_tests.rs::served_selection_returns_insights_with_source_runs`."""

from __future__ import annotations

from typing import Any

import pytest
from _decision_resident_fixture import publish
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError

SIGNATURE = "export weekly report"


def _insight(
    insight_id: str, text: str, runs: list[str], signature: str, successes: int
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "insight_id": insight_id,
        "text": text,
        "source_run_ids": runs,
        "context_signature": signature,
        "success_count": successes,
        "selection_revision": 4,
    }


def _submit(client: Any, insight: dict[str, Any]) -> dict[str, Any]:
    return client._send(
        "DecisionLog",
        {
            "op": {
                "op": "run_record",
                "tenant_id": TEST_TENANT,
                "record": {"kind": "insight", "insight": insight},
            }
        },
    )


def _select(client: Any, schema: dict[str, Any], signature: str) -> dict[str, Any]:
    request = dc.decide_request(
        TEST_TENANT,
        ("insight.select", "insight_selection", "ordinary"),
        dc.declared([dc.DeclaredOption("insight", numbers={"signal": 0})]),
        schema,
        params=[dc.param("context_signature", "text", signature)],
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


def _selected(record: dict[str, Any]) -> list[str]:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return [score["option_id"].removeprefix("insight:") for score in outcome["scores"]]


def _premise(record: dict[str, Any], insight_id: str) -> str:
    for premise in record["premises"]:
        if premise["fact"].startswith(f"insight:id={insight_id};"):
            return premise["fact"]
    raise AssertionError(f"premise of {insight_id} is missing")


# spec: MEDULLA-DECISION-ENGINE-R135.15
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R135.15")
def test_post_run_write_then_context_injection(clean_graph):
    body = ds.feature_schema_body([ds.feature("signal", ds.number("signal"))])
    schema = publish(clean_graph, "insight-schema", "feature_schema", body)

    # Nothing is stored yet: the selection reads the (empty) stored rows and
    # abstains rather than inventing an insight.
    before = _select(clean_graph, schema, SIGNATURE)
    assert before["outcome"]["outcome"] == "abstained", before["outcome"]

    # Two insights with source runs are stored after a run.
    first = _insight(
        "ins-1",
        "retry the export once before escalating",
        ["run-1", "run-2"],
        SIGNATURE,
        3,
    )
    second = _insight(
        "ins-2",
        "attach the checksum to the upload",
        ["run-3"],
        "export weekly archive",
        1,
    )
    for item in (first, second):
        stored = _submit(clean_graph, item)
        assert stored["replayed"] is False
        assert stored["stored"]["insight"]["insight_id"] == item["insight_id"]

    # The selection reads the stored rows: both are selected, strongest context
    # match first, and each carries its source runs.
    record = _select(clean_graph, schema, SIGNATURE)
    assert _selected(record) == ["ins-1", "ins-2"]
    assert ";sources=run-1,run-2;" in _premise(record, "ins-1")
    assert ";sources=run-3;" in _premise(record, "ins-2")

    # An insight with no source run refuses with the typed code and stores
    # nothing: the selection is unchanged.
    orphan = _insight("ins-3", "an unsourced hint", [], SIGNATURE, 5)
    with pytest.raises(EngineResponseError, match="MISSING_SOURCE_RUN"):
        _submit(clean_graph, orphan)
    assert _selected(_select(clean_graph, schema, SIGNATURE)) == ["ins-1", "ins-2"]
