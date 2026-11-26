"""MEDULLA-DECISION-ENGINE-R617.9 served participant writes: a participant is
put, joins a session that a served ingest created, and is scoped and observed,
all over the wire through the ``participant`` client method.

An equal replay changes nothing, an update changes the name and ``acts_as``
only, the kind never changes, a session with no thread is unknown, and another
graph never sees the participant.
"""

from __future__ import annotations

import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from conftest import TEST_AGENT_ID
from ingest.test_source_batch_kind_served import _transcript_batch
from ingest.test_source_stream_versions_served import (
    _close,
    _connector,
    _ingest_client,
    _submit,
)

from medulla import decision_client as dc

SESSION = "session-1"


async def _refusal(call) -> str:
    try:
        await call
    except Exception as error:  # the typed refusal carries the code
        return str(error)
    raise AssertionError("the call was expected to be refused")


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R617.9")
@pytest.mark.asyncio
async def test_participant_put_and_member_round_trip() -> None:
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    other_connector = _connector()
    other, other_graph = await _ingest_client(other_connector)
    participants = dc.DecisionClient(client=client, graph=graph)
    foreign = dc.DecisionClient(client=other, graph=other_graph)
    try:
        wire = dc.participant_wire(
            participant_id="ada-1",
            workspace=connector,
            kind="person",
            display_name="Ada",
            represents="urn:person:ada",
            acts_as=[TEST_AGENT_ID],
        )
        created = await participants.participant(dc.put_participant_op(wire))
        replay = await participants.participant(dc.put_participant_op(wire))
        assert replay == created

        renamed = await participants.participant(
            dc.put_participant_op({**wire, "display_name": "Ada L."})
        )
        assert renamed["display_name"] == "Ada L."
        assert renamed["kind"] == "person"

        kind_change = await _refusal(
            participants.participant(dc.put_participant_op({**wire, "kind": "agent"}))
        )
        assert "PARTICIPANT_KIND_INVALID" in kind_change

        # No thread has that session yet.
        unknown = await _refusal(
            participants.participant(dc.add_member_op(SESSION, "ada-1"))
        )
        assert "MEMBERSHIP_SESSION_UNKNOWN" in unknown

        await client.nodes.add(concept, {"type": "Concept"})
        receipt = await _submit(
            client, _transcript_batch(connector, 1, None, concept, "claude"), "members"
        )
        assert receipt.committed_graph_version > 0
        member = await participants.participant(dc.add_member_op(SESSION, "ada-1"))
        again = await participants.participant(dc.add_member_op(SESSION, "ada-1"))
        assert member["session_id"] == SESSION and member["participant_id"] == "ada-1"
        assert again["participant_id"] == member["participant_id"]

        scope = await participants.participant(
            dc.put_scope_op(
                scope_id="scope-1",
                name="work",
                owner_principal=TEST_AGENT_ID,
                session_ids=[SESSION],
            )
        )
        assert scope["session_ids"] == [SESSION]
        observed = await participants.participant(
            dc.set_observation_op(SESSION, "ada-1", observe_others=True)
        )
        assert observed["observe_others"] is True

        # Another graph sees neither the participant nor the session.
        hidden = await _refusal(foreign.participant(dc.add_member_op(SESSION, "ada-1")))
        assert "MEMBERSHIP_SESSION_UNKNOWN" in hidden
    finally:
        await _close(other, other_graph)
        await _close(client, graph)
