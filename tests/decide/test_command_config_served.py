"""MEDULLA-DECISION-ENGINE-R148.6 served proof of mode decision and replay.

The entry point is the served ``Decide`` method with the ``command_config``
question kind: the caller names a graph that holds a ``CommandConfigSchema``
node and ``CommandMode`` nodes, and the engine chooses the mode, builds its
config against the schema and seals the answer. The same input decided twice
gives the same config digest and the same ``inputs_digest``; a revision of the
mode gives another config digest. ``Decide`` is evaluate-only: nothing is
written.
"""

from __future__ import annotations

from typing import Any

import msgpack
import pytest
from _decision_resident_fixture import publish, schema_body
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op
from conftest import TEST_TENANT

from medulla import decision_client as dc

SCHEMA_ID = "schema:cmd-r148"
MODE_ID = "fast-r148"
SAFE_ID = "safe-r148"


def _scalar(value: int | str) -> dict[str, Any]:
    kind = "int" if isinstance(value, int) else "text"
    return {"type": kind, "value": value}


def _mode(mode_id: str, ordinal: int, revision: int, retries: int) -> dict[str, Any]:
    return {
        "type": "CommandMode",
        "schema_version": 1,
        "mode_id": mode_id,
        "mode_ordinal": ordinal,
        "mode_revision": revision,
        "schema_id": SCHEMA_ID,
        "settings": {"retries": _scalar(retries), "label": _scalar(mode_id)},
    }


SCHEMA_NODE = {
    "type": "CommandConfigSchema",
    "schema_version": 1,
    "schema_id": SCHEMA_ID,
    "revision": 1,
    "fields": {"retries": "int", "label": "text"},
}


def _write(client: Any, key: str, sequence: int, nodes: list[tuple[str, dict]]) -> None:
    """Write ``nodes`` through the generated governed write, the same entry
    point as the harness-config served tests."""
    envelope = _envelope(
        _graph_name(client),
        key,
        key,
        sequence,
        [
            op(
                ordinal,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
            for ordinal, (node_id, props) in enumerate(nodes)
        ],
    )
    results = client.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results


def _request(schema: dict[str, Any], graph: str) -> dict[str, Any]:
    return dc.decide_request(
        TEST_TENANT,
        ("q:r148:mode", "command_config", "ordinary"),
        dc.declared([dc.DeclaredOption("mode")]),
        schema,
        params=[
            dc.param("graph", "text", graph),
            dc.param("mode_id", "text", MODE_ID),
        ],
    )


def _decide(client: Any, request: dict[str, Any]) -> dict[str, Any]:
    return client._send("Decide", {"request": request})["records"][0]


def _config_option(record: dict[str, Any]) -> str:
    options = [score["option_id"] for score in record["outcome"]["scores"]]
    assert options[0] == f"mode:{MODE_ID}", options
    (config,) = [option for option in options if option.startswith("config:")]
    return config


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R148.6")
def test_mode_decision_and_replay(clean_graph):
    client = clean_graph
    graph = _graph_name(client)
    _write(
        client,
        "r148-6:modes",
        1,
        [
            ("schema:cmd-r148", SCHEMA_NODE),
            ("mode:safe-r148", _mode(SAFE_ID, 0, 1, 1)),
            (f"mode:{MODE_ID}", _mode(MODE_ID, 1, 7, 5)),
        ],
    )
    schema = publish(client, "r148-6-schema", "feature_schema", schema_body())
    request = _request(schema, graph)

    # The same input decided twice: equal config digests and inputs digests.
    first = _decide(client, request)
    second = _decide(client, request)
    assert first["question"]["kind"] == "command_config"
    assert _config_option(first) == _config_option(second)
    assert first["inputs_digest"] == second["inputs_digest"]
    assert any(
        premise["fact"] == f"command_config:mode={MODE_ID}@7"
        for premise in first["premises"]
    )

    # A revision of the mode changes the config digest.
    _write(
        client,
        "r148-6:revision",
        2,
        [(f"mode:{MODE_ID}", _mode(MODE_ID, 1, 8, 6))],
    )
    revised = _decide(client, request)
    assert _config_option(revised) != _config_option(first)
    assert any(
        premise["fact"] == f"command_config:mode={MODE_ID}@8"
        for premise in revised["premises"]
    )
