"""Revisioned city-state facts (MEDULLA-WORLD-VIEW-R004)."""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from typing import Any

import msgpack
import pytest
from _client_fixtures import RecordingTransport, SentCall

from medulla.world_view import (
    ACTIVITY_STATES,
    HEARTBEAT_TTL,
    CityStateReader,
    CityStateRefused,
    CityStateWriter,
    Override,
    layout_revision_of,
)

pytestmark = pytest.mark.no_engine

SUBJECT = "principal:ada"
NOW = datetime(2026, 11, 28, 12, 0, tzinfo=UTC)


class _Graph(RecordingTransport):
    """A fake graph with the engine's node semantics: `CreateNodeIfAbsent` stores a new
    node, `CompareAndSetNodeFields` merges its updates only when every condition holds,
    and `GetNodesByLabel` serves the stored nodes."""

    def __init__(self) -> None:
        super().__init__()
        self.nodes: dict[str, dict[str, Any]] = {}

    def reply(self, call: SentCall) -> Any:
        assert call.params is not None
        if call.method == "CreateNodeIfAbsent":
            node_id = call.params["node_id"]
            if node_id in self.nodes:
                return False
            self.nodes[node_id] = msgpack.unpackb(
                call.params["properties_msgpack"], raw=False
            )
            return True
        if call.method == "CompareAndSetNodeFields":
            node = self.nodes.get(call.params["node_id"])
            conditions = msgpack.unpackb(call.params["conditions_msgpack"], raw=False)
            if node is None or any(node.get(k) != v for k, v in conditions.items()):
                return False
            node.update(msgpack.unpackb(call.params["updates_msgpack"], raw=False))
            return True
        if call.method == "GetNodesByLabel":
            return [
                [node_id, msgpack.packb(properties, use_bin_type=True)]
                for node_id, properties in self.nodes.items()
                if properties.get("type") == call.params["label"]
            ]
        raise AssertionError(f"unexpected {call.method}")


def _clock(at: datetime):
    return lambda: at


def _heartbeat(writer: CityStateWriter, expected: int, **overrides: Any) -> Any:
    arguments: dict[str, Any] = {
        "subject": SUBJECT,
        "expected_revision": expected,
        "x": 3.5,
        "y": -2.0,
        "heading": 90.0,
        "activity_state": "walking",
        "target_ref": "building:alpha",
    }
    arguments.update(overrides)
    return asyncio.run(writer.heartbeat(**arguments))


def _snapshot(transport: _Graph, at: datetime = NOW, overrides: Any = None):
    reader = CityStateReader(transport, overrides, _clock(at))
    return asyncio.run(reader.snapshot("tenant:a"))


def _npc(transport: _Graph, name: str, revision: int) -> None:
    transport.nodes[f"world:presence:npc:{name}"] = {
        "type": "CityPresence",
        "presence_kind": "npc",
        "world_x": 1,
        "world_y": 2,
        "heading": 0,
        "activity_state": "walking",
        "target_ref": None,
        "revision": revision,
    }


# spec: MEDULLA-WORLD-VIEW-R004
@pytest.mark.spec("MEDULLA-WORLD-VIEW-R004")
def test_snapshot_is_revisioned_and_heartbeat_is_subject_bound_and_fenced() -> None:
    transport = _Graph()
    writer = CityStateWriter(transport, SUBJECT, _clock(NOW))
    assert _snapshot(transport).revision == 0
    assert _snapshot(transport).entities == ()

    assert _heartbeat(writer, 0) == 1
    assert _heartbeat(writer, 1, x=4.0) == 2
    _npc(transport, "baker", revision=7)
    snapshot = _snapshot(transport)
    assert snapshot.revision == 7, "the highest entity revision"
    assert snapshot.simulated_at == NOW.isoformat()
    human = next(e for e in snapshot.entities if e.presence_kind == "human")
    assert (human.subject, human.x, human.revision) == (SUBJECT, 4.0, 2)
    assert human.expires_at == (NOW + HEARTBEAT_TTL).isoformat()

    # A stale expected revision is fenced and writes nothing; so is a second create.
    before = {key: dict(value) for key, value in transport.nodes.items()}
    with pytest.raises(CityStateRefused, match="REVISION_CONFLICT") as stale:
        _heartbeat(writer, 1)
    assert stale.value.code == "REVISION_CONFLICT"
    with pytest.raises(CityStateRefused, match="REVISION_CONFLICT"):
        _heartbeat(writer, 0)
    assert transport.nodes == before

    # Another subject's node is not this connection's to write.
    sent = len(transport.sent)
    with pytest.raises(CityStateRefused, match="SUBJECT_ACCESS_DENIED"):
        _heartbeat(writer, 0, subject="principal:grace")
    assert len(transport.sent) == sent

    # An expired human node is absent from the snapshot; an npc never expires.
    later = _snapshot(transport, NOW + HEARTBEAT_TTL + timedelta(seconds=1))
    assert [e.presence_kind for e in later.entities] == ["npc"]
    assert later.revision == 7

    # The layout revision follows the stored overrides.
    assert snapshot.layout_revision == 0
    override = Override("alpha", 1, 2, "moved")
    assert layout_revision_of([override]) != 0
    assert layout_revision_of([override]) != layout_revision_of(
        [Override("alpha", 1, 3, "moved")]
    )


# spec: MEDULLA-WORLD-VIEW-R004
@pytest.mark.spec("MEDULLA-WORLD-VIEW-R004")
def test_heartbeat_stores_activity_and_target_and_refuses_unknown_values() -> None:
    transport = _Graph()
    writer = CityStateWriter(transport, SUBJECT, _clock(NOW))

    revision = 0
    for state in ACTIVITY_STATES:
        revision = _heartbeat(writer, revision, activity_state=state)
        entity = _snapshot(transport).entities[0]
        assert entity.activity_state == state
        assert entity.target_ref == "building:alpha"
    assert ACTIVITY_STATES == ("walking", "driving", "flying", "sailing", "inside")

    # `None` clears the target; the snapshot reads it as absent.
    revision = _heartbeat(writer, revision, target_ref=None)
    assert _snapshot(transport).entities[0].target_ref is None

    # Any other value refuses INVALID_ARGUMENT before the write.
    written = len(transport.sent)
    for bad in (
        {"activity_state": "swimming"},
        {"activity_state": ""},
        {"target_ref": ""},
        {"target_ref": "has space"},
        {"target_ref": "x" * 129},
        {"target_ref": "door/../etc"},
    ):
        with pytest.raises(CityStateRefused, match="INVALID_ARGUMENT"):
            _heartbeat(writer, revision, **bad)
    assert len(transport.sent) == written, "a refusal writes nothing"
    assert _snapshot(transport).entities[0].revision == revision

    ok = "a" * 128
    assert _heartbeat(writer, revision, target_ref=ok) == revision + 1
    assert _snapshot(transport).entities[0].target_ref == ok
