"""Argument maps with a Dung projection over the served wire (R700).

MEDULLA-FEDERATED-QUERY-R700.

The session fixture in conftest starts and authenticates the native server. The test
stores the golden vector, exports it byte-equal and evaluates it.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

GOLDEN = {
    "nodes": [
        {"nodeID": "a", "text": "text a", "type": "I"},
        {"nodeID": "b", "text": "text b", "type": "I"},
        {"nodeID": "ca", "text": "text ca", "type": "CA"},
    ],
    "edges": [
        {"edgeID": "e1", "fromID": "b", "toID": "ca"},
        {"edgeID": "e2", "fromID": "ca", "toID": "a"},
    ],
}


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _put(client: MedullaClient, map_id: str, map_doc: dict) -> dict:
    result = await query.send_argument_map_put(
        client, {"map_id": map_id, "map": map_doc}
    )
    return result.payload


async def _get(client: MedullaClient, map_id: str) -> dict:
    return (await query.send_argument_map_get(client, {"map_id": map_id})).payload


async def _evaluate(client: MedullaClient, map_id: str, semantics: str) -> dict:
    return (
        await query.send_argument_map_evaluate(
            client, {"map_id": map_id, "semantics": semantics}
        )
    ).payload


# spec: MEDULLA-FEDERATED-QUERY-R700.3
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R700.3")
@pytest.mark.asyncio
async def test_put_get_round_trip_and_evaluate() -> None:
    client = await _connect()
    map_id = f"aif-{uuid.uuid4().hex}"

    stored = await _put(client, map_id, GOLDEN)
    assert stored == GOLDEN

    # The interchange document exports byte-equal: the stored canonical JSON
    # round-trips to the very document that was stored, and re-storing the
    # export replaces the map with itself.
    exported = await _get(client, map_id)
    assert exported == GOLDEN
    assert await _put(client, map_id, exported) == GOLDEN

    # `b` attacks `a` through `ca`: the grounded extension is `[b]`, and it is
    # the only preferred extension.
    grounded = await _evaluate(client, map_id, "grounded")
    assert grounded["extensions"] == [["b"]]
    assert grounded["complete"] is True
    preferred = await _evaluate(client, map_id, "preferred")
    assert preferred["complete"] is True
    assert preferred["extensions"] == [["b"]]


def _digest(doc: dict) -> str:
    # The canonical interchange JSON is the AIF document in its declared order:
    # `nodes` then `edges`, each record's keys as declared. The JSON result wire
    # carries objects with sorted keys, so the order is rebuilt here, not assumed.
    canonical = {
        "nodes": [
            {"nodeID": n["nodeID"], "text": n["text"], "type": n["type"]}
            for n in doc["nodes"]
        ],
        "edges": [
            {"edgeID": e["edgeID"], "fromID": e["fromID"], "toID": e["toID"]}
            for e in doc["edges"]
        ],
    }
    return hashlib.sha256(
        json.dumps(canonical, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()


# spec: MEDULLA-FEDERATED-QUERY-R700.5
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R700.5")
@pytest.mark.asyncio
async def test_digest_binds_map() -> None:
    client = await _connect()
    map_id = f"aif-{uuid.uuid4().hex}"
    await _put(client, map_id, GOLDEN)

    exported = await _get(client, map_id)
    grounded = await _evaluate(client, map_id, "grounded")
    assert grounded["semantics"] == "grounded"
    assert grounded["map_digest"] == _digest(exported)
    assert grounded["extensions"] == [["b"]]

    # Re-derive the extension from the exported map: `b` attacks `a` through `ca`.
    attacked = {edge["toID"] for edge in exported["edges"] if edge["fromID"] == "ca"}
    arguments = {n["nodeID"] for n in exported["nodes"] if n["type"] == "I"}
    assert [sorted(arguments - attacked)] == grounded["extensions"]

    # Evaluation stores nothing: the digest is unchanged by evaluating again.
    preferred = await _evaluate(client, map_id, "preferred")
    assert preferred["map_digest"] == grounded["map_digest"]
    assert preferred["complete"] is True

    # Replacing the map changes the digest the next evaluation reports.
    changed = {
        **GOLDEN,
        "nodes": [{**GOLDEN["nodes"][0], "text": "other"}, *GOLDEN["nodes"][1:]],
    }
    await _put(client, map_id, changed)
    again = await _evaluate(client, map_id, "grounded")
    assert again["map_digest"] == _digest(changed)
    assert again["map_digest"] != grounded["map_digest"]


def _grounded_oracle(arguments: set[str], attacks: set[tuple[str, str]]) -> list[str]:
    """The least fixed point of the characteristic function, by plain iteration."""
    accepted: set[str] = set()
    rejected: set[str] = set()
    changed = True
    while changed:
        changed = False
        for argument in sorted(arguments - accepted - rejected):
            attackers = {a for a, b in attacks if b == argument}
            if attackers <= rejected:
                accepted.add(argument)
                changed = True
        for argument in sorted(arguments - accepted - rejected):
            if any(a in accepted for a, b in attacks if b == argument):
                rejected.add(argument)
                changed = True
    return sorted(accepted)


def _fixture() -> dict:
    # i1 -> i2 -> i3 -> i4 is a chain; i5 and i6 attack each other (undecided); i7 is
    # attacked by i1 and defended by nobody; i8 is attacked only by the undecided i5.
    arguments = [f"i{n}" for n in range(1, 9)]
    attacks = [
        ("i1", "i2"),
        ("i2", "i3"),
        ("i3", "i4"),
        ("i5", "i6"),
        ("i6", "i5"),
        ("i1", "i7"),
        ("i5", "i8"),
    ]
    nodes = [{"nodeID": a, "text": f"text {a}", "type": "I"} for a in arguments]
    edges = []
    for index, (src, dst) in enumerate(attacks, start=1):
        nodes.append({"nodeID": f"ca{index}", "text": f"attack {index}", "type": "CA"})
        edges.append({"edgeID": f"e{index}a", "fromID": src, "toID": f"ca{index}"})
        edges.append({"edgeID": f"e{index}b", "fromID": f"ca{index}", "toID": dst})
    return {"nodes": nodes, "edges": edges, "arguments": arguments, "attacks": attacks}


# spec: MEDULLA-FEDERATED-QUERY-R700.6
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R700.6")
@pytest.mark.asyncio
async def test_grounded_extension_matches_fixture() -> None:
    fixture = _fixture()
    document = {"nodes": fixture["nodes"], "edges": fixture["edges"]}
    client = await _connect()
    map_id = f"aif-{uuid.uuid4().hex}"
    await _put(client, map_id, document)

    result = await _evaluate(client, map_id, "grounded")
    expected = _grounded_oracle(set(fixture["arguments"]), set(fixture["attacks"]))
    assert expected == ["i1", "i3"]
    assert result["complete"] is True
    assert [sorted(extension) for extension in result["extensions"]] == [expected]

    # Two maps on different graphs evaluate independently and concurrently.
    other_id = f"aif-{uuid.uuid4().hex}"
    await _put(client, other_id, GOLDEN)
    first, second = await asyncio.gather(
        _evaluate(client, map_id, "grounded"), _evaluate(client, other_id, "grounded")
    )
    assert first["extensions"] == result["extensions"]
    assert second["extensions"] == [["b"]]
