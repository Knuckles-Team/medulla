"""MEDULLA-DECISION-ENGINE-R617.9 served participant writes: a participant is
put, joins a session that a served ingest created, and is scoped and observed,
all over the wire through the ``participant`` client method.

An equal replay changes nothing, an update changes the name and ``acts_as``
only, the kind never changes, a session with no thread is unknown, and another
graph never sees the participant.
"""

from __future__ import annotations

import hashlib
import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from conftest import TEST_AGENT_ID, TEST_TENANT
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


def _turn(name: str, session: str, speaker: str | None = None) -> dict:
    record: dict = {
        "memory_id": name,
        "tenant": TEST_TENANT,
        "kind": "fact",
        "tier": "episodic",
        "text": f"note of {name}",
        "session_id": session,
        "timestamp_ms": 1_000,
        "importance": 1.0,
        "token_estimate": 10,
    }
    if speaker is not None:
        record["speaker"] = speaker
    return record


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R651.9")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R155.7")
@pytest.mark.asyncio
async def test_memory_scope_narrows_recall() -> None:
    """A recall that names a memory scope only returns records of the scope's sessions,
    never a session the caller cannot see, and reports the scope and its session
    count."""
    connector = _connector()
    client, graph = await _ingest_client(connector)
    decision = dc.DecisionClient(client=client, graph=graph)
    prefix = uuid.uuid4().hex[:8]
    try:
        sessions = {"a": f"s-{prefix}-a", "b": f"s-{prefix}-b", "c": f"s-{prefix}-c"}
        for key, session in sessions.items():
            await decision.memory_remember(
                _turn(f"{prefix}-{key}", session), [1.0, 0.0]
            )
        request = {
            "query_embedding": [1.0, 0.0],
            "budget": 10_000,
            "keywords": ["note"],
        }

        everything = await decision.memory_recall(request)
        assert {hit["record"]["memory_id"] for hit in everything["hits"]} >= {
            f"{prefix}-a",
            f"{prefix}-b",
            f"{prefix}-c",
        }

        # The scope names two visible sessions and one the caller has no record in.
        await decision.participant(
            dc.put_scope_op(
                scope_id=f"scope-{prefix}",
                name="work",
                owner_principal=TEST_AGENT_ID,
                session_ids=[sessions["a"], sessions["c"], f"s-{prefix}-missing"],
            )
        )
        narrowed = await decision.memory_recall(
            {**request, "memory_scope": {"scope_id": f"scope-{prefix}"}}
        )
        held = {hit["record"]["memory_id"] for hit in narrowed["hits"]}
        assert held == {f"{prefix}-a", f"{prefix}-c"}
        assert narrowed["memory_scope"] == f"scope-{prefix}"
        assert narrowed["scope_session_count"] == 2

        # A scope that does not exist is refused, and refuses before anything is read.
        missing = await _refusal(
            decision.memory_recall({**request, "memory_scope": {"scope_id": "absent"}})
        )
        assert "SCOPE_NOT_FOUND" in missing
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R617.8")
@pytest.mark.asyncio
async def test_membership_batch_replays_under_the_thread_key() -> None:
    """The ingest writes a thread's memberships and the thread under one key; a
    replay of the same batch under that key is accepted by the replay ledger and commits
    nothing new."""
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    participants = dc.DecisionClient(client=client, graph=graph)
    try:
        wire = dc.participant_wire(
            participant_id="ada-1",
            workspace=connector,
            kind="person",
            display_name="Ada",
            represents="urn:person:ada",
            acts_as=[TEST_AGENT_ID],
        )
        await participants.participant(dc.put_participant_op(wire))
        await client.nodes.add(concept, {"type": "Concept"})
        batch = _transcript_batch(connector, 1, None, concept, "claude")
        for item in batch["extension"]["transcripts"]:
            if "message" in item:
                item["message"]["speaker"] = "ada-1"

        key = f"membership-replay:{uuid.uuid4().hex}"
        first = await client._send(
            "SourceIngest", {"request": batch}, idempotency_key=key
        )
        second = await client._send(
            "SourceIngest", {"request": batch}, idempotency_key=key
        )
        assert first["outcome"] == "accepted" and second["outcome"] == "accepted"
        assert (
            second["receipt"]["committed_graph_version"]
            == first["receipt"]["committed_graph_version"]
        )
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R655.6")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R655.4")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R655.3")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R655.2")
@pytest.mark.asyncio
async def test_ask_card_and_status_journey() -> None:
    """Two participants in one session; turns queue a derive job; a worker
    proposes a claim with turn evidence; the claim moves, so the card is stored;
    then the participant is asked
    about, the card read back and the backlog read."""
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    participants = dc.DecisionClient(client=client, graph=graph)
    ada = {"participant": "ada-1"}
    try:
        for slug in ("ada-1", "bo-2"):
            await participants.participant(
                dc.put_participant_op(
                    dc.participant_wire(
                        participant_id=slug,
                        workspace=connector,
                        kind="person",
                        display_name=slug,
                        represents=f"urn:person:{slug}",
                        acts_as=[TEST_AGENT_ID],
                    )
                )
            )
        await client.nodes.add(concept, {"type": "Concept"})
        await _submit(
            client, _transcript_batch(connector, 1, None, concept, "claude"), "journey"
        )
        for slug in ("ada-1", "bo-2"):
            await participants.participant(dc.add_member_op(SESSION, slug))

        # Turns of the session queue derive jobs the status reports.
        for n, speaker in enumerate(("ada-1", "bo-2", "ada-1")):
            await participants.memory_remember(
                _turn(f"journey-turn-{n}", SESSION, speaker), [1.0, 0.0]
            )
        status = await participants.participant_read(dc.status_op())
        waiting = [row for row in status if row["subject"] == "ada-1"]
        assert waiting and all(row["session_id"] == SESSION for row in waiting)
        assert sum(row["pending"] for row in waiting) >= 1

        # The worker proposes a conclusion with its turn evidence; the claim then moves.
        claim_id = f"claim-{uuid.uuid4().hex[:8]}"
        await participants.claims(
            dc.claim_propose_op(
                claim_id, subject="ada-1", observer=ada, statement="Prefers green tea"
            )
        )
        for index, channel in enumerate(
            ("outcome", "optimization", "health", "process", "mined")
        ):
            await participants.claims(
                dc.claim_evidence_op(
                    f"{claim_id}:e{index}",
                    claim_id,
                    channel,
                    supports=True,
                    weight_ppm=1_000_000,
                    source_digest="turn:journey-turn-0"
                    if channel == "mined"
                    else f"sha256:{index:064x}",
                    session_id=SESSION,
                )
            )
        await participants.claims(dc.claim_advance_op(claim_id, "supported"))

        answer = await participants.participant_read(
            dc.ask_op("ada-1", "what tea does she prefer", observer=ada)
        )
        assert answer["text"] == "Prefers green tea."
        assert answer["citations"][0]["claim_id"] == claim_id
        assert answer["citations"][0]["turn_ids"] == ["journey-turn-0"]
        unrelated = await participants.participant_read(
            dc.ask_op("ada-1", "where was she born", observer=ada)
        )
        assert unrelated["abstained"] is not None and unrelated["text"] == ""

        card = await participants.participant_read(dc.card_op("ada-1", observer=ada))
        assert [claim["text"] for claim in card["claims"]] == ["Prefers green tea"]
        assert card["digest"].startswith("sha256:")

        # No claim, no card; the refusal is the same non-disclosing envelope.
        absent = await _refusal(
            participants.participant_read(
                dc.card_op("bo-2", observer={"participant": "bo-2"})
            )
        )
        assert "CARD_ABSENT" in absent
    finally:
        await _close(client, graph)


def _payload_ref(
    tenant: str, session: str, subject: str, observer: str = "system"
) -> str:
    """The graph node id of a derive key's payload: the length-framed SHA-256
    of the key (scope, session, subject, observer, empty first turn), as the
    engine builds it."""
    digest = hashlib.sha256()
    for part in ("", session, subject, observer, ""):
        raw = part.encode()
        digest.update(len(raw).to_bytes(8, "big"))
        digest.update(raw)
    return f"derive_payload:{tenant}:{digest.hexdigest()}"


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R651.12")
@pytest.mark.asyncio
async def test_turns_become_perspective_claims() -> None:
    """An external worker reads a queued derive task's input, proposes claims
    with turn evidence over the wire and completes the range: the checkpoint
    advances once, a repeat changes nothing and a count above the knob is
    refused."""
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    participants = dc.DecisionClient(client=client, graph=graph)
    session = f"s-{uuid.uuid4().hex[:8]}"
    try:
        for slug in ("ada-1", "bo-2"):
            await participants.participant(
                dc.put_participant_op(
                    dc.participant_wire(
                        participant_id=slug,
                        workspace=connector,
                        kind="person",
                        display_name=slug,
                        represents=f"urn:person:{slug}",
                        acts_as=[TEST_AGENT_ID],
                    )
                )
            )
        await client.nodes.add(concept, {"type": "Concept"})
        await _submit(
            client, _transcript_batch(connector, 1, None, concept, "claude"), "derive"
        )
        for n, speaker in enumerate(("ada-1", "bo-2", "ada-1")):
            record = _turn(f"{session}-t{n}", session, speaker)
            record["timestamp_ms"] = 1_000 + n
            await participants.memory_remember(record, [1.0, 0.0])

        ref = _payload_ref(TEST_TENANT, session, "ada-1")
        task = await participants.participant_read(dc.derive_input_op(ref))
        assert task["subject"] == "ada-1" and task["observer"] == "system"
        assert [turn["turn_id"] for turn in task["turns"]] == [
            f"{session}-t0",
            f"{session}-t2",
        ]
        assert 1 <= task["max_conclusions"]

        claim_id = f"derived-{uuid.uuid4().hex[:8]}"
        await participants.claims(
            dc.claim_propose_op(
                claim_id,
                subject="ada-1",
                observer="system",
                statement="Prefers green tea",
            )
        )
        for turn in task["turns"]:
            await participants.claims(
                dc.claim_evidence_op(
                    f"{claim_id}:{turn['turn_id']}",
                    claim_id,
                    "mined",
                    supports=True,
                    weight_ppm=200_000,
                    source_digest=turn["turn_id"],
                    session_id=session,
                )
            )

        done = await participants.participant(dc.complete_derivation_op(ref, 1))
        assert done["claims_written"] == 1
        assert done["last_turn_id"] == f"{session}-t2"
        again = await participants.participant(dc.complete_derivation_op(ref, 1))
        assert again == done
        too_many = await _refusal(
            participants.participant(dc.complete_derivation_op(ref, 10_000))
        )
        assert "DERIVE_CONCLUSION_LIMIT" in too_many
        unknown = await _refusal(
            participants.participant_read(dc.derive_input_op("derive_payload:x:y"))
        )
        assert "DERIVE_PAYLOAD_NOT_FOUND" in unknown
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R073.7")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R073.6")
@pytest.mark.asyncio
async def test_events_pull_and_ack() -> None:
    """A claim transition writes one `memory.claim.changed` event to the graph
    outbox after it commits; a service consumer leases it with `Pull` and
    advances its cursor with `Ack`; another consumer name sees its own cursor,
    and a topic outside the three is refused."""
    connector = _connector()
    client, graph = await _ingest_client(connector)
    participants = dc.DecisionClient(client=client, graph=graph)
    target = {"target": "graph", "graph": graph}
    topic = "memory.claim.changed"
    try:
        await participants.participant(
            dc.put_participant_op(
                dc.participant_wire(
                    participant_id="ada-1",
                    workspace=connector,
                    kind="person",
                    display_name="Ada",
                    represents="urn:person:ada",
                    acts_as=[TEST_AGENT_ID],
                )
            )
        )

        async def pull(consumer: str, topics: list[str]) -> dict:
            result = await client._send(
                "MutationOutbox",
                {
                    "op": {
                        "op": "pull",
                        "target": target,
                        "consumer": consumer,
                        "topics": topics,
                        "limit": 10,
                    }
                },
                graph,
            )
            return dict(result)

        # The first pull subscribes the consumer; nothing has happened yet.
        assert (await pull("memory-webhooks", [topic]))["leases"] == []

        claim_id = f"claim-{uuid.uuid4().hex[:8]}"
        await participants.claims(
            dc.claim_propose_op(
                claim_id, subject="ada-1", observer="system", statement="Prefers tea"
            )
        )
        for index, channel in enumerate(
            ("outcome", "optimization", "health", "process", "mined")
        ):
            await participants.claims(
                dc.claim_evidence_op(
                    f"{claim_id}:e{index}",
                    claim_id,
                    channel,
                    supports=True,
                    weight_ppm=1_000_000,
                    source_digest=f"sha256:{index:064x}",
                )
            )
        await participants.claims(dc.claim_advance_op(claim_id, "supported"))

        pulled = await pull("memory-webhooks", [topic])
        assert len(pulled["leases"]) == 1
        lease = pulled["leases"][0]
        assert lease["record"]["intent"]["topic"] == topic
        assert lease["record"]["intent"]["key"] == "ada-1"
        acked = await client._send(
            "MutationOutbox",
            {
                "op": {
                    "op": "ack",
                    "target": target,
                    "consumer": "memory-webhooks",
                    "lease": lease,
                }
            },
            graph,
        )
        assert dict(acked)["consumer"] == "memory-webhooks"
        assert (await pull("memory-webhooks", [topic]))["leases"] == []

        # Another consumer name keeps its own cursor, untouched by the ack above.
        other = await pull("other-hooks", [topic])
        assert "leases" in other

        refused = await _refusal(pull("memory-webhooks", ["engine.projection.rebuild"]))
        assert "ACCESS_DENIED" in refused
    finally:
        await _close(client, graph)
