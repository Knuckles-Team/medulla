"""MEDULLA-CQ-ORCH-R810.4 served proof: an ack keeps the message acked, a repeat ack by
the claimant answers true and writes nothing, and a lapsed claimant is refused."""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context


@pytest.mark.spec("MEDULLA-CQ-ORCH-R810.4")
@pytest.mark.asyncio
async def test_ack_is_idempotent_and_a_lapsed_claimant_is_refused(
    start_medulla_server,
) -> None:
    from medulla.client import MedullaClient, TransitionRefusedError

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    suffix = uuid.uuid4().hex[:8]
    exchange, queue = f"x-{suffix}", f"q-{suffix}"
    now = 1_000_000
    try:
        await client.broker.declare_exchange(exchange, "direct")
        await client.broker.declare_queue(queue)
        await client.broker.bind_queue(exchange, queue, "k")
        assert await client.broker.publish(exchange, "k", b"payload") == 1

        claimed = await client.broker.consume(
            queue, group="g", consumer="c1", now_ms=now, lease_ms=100
        )
        assert claimed is not None
        node_id, _props = claimed

        # The lease lapses; c1's ack is refused by the machine.
        with pytest.raises(TransitionRefusedError) as refused:
            await client.broker.ack(queue, node_id, consumer="c1", now_ms=now + 100)
        assert refused.value.code == "TRANSITION_REFUSED"
        assert refused.value.machine == "BrokerMessageMachine"
        assert refused.value.from_state == "claimed"
        assert refused.value.event == "Ack"
        assert refused.value.reason == "guard_failed"

        # c2 claims the lapsed message; its first and second acks both answer true.
        again = await client.broker.consume(
            queue, group="g", consumer="c2", now_ms=now + 100, lease_ms=100
        )
        assert again is not None and again[0] == node_id
        assert await client.broker.ack(queue, node_id, consumer="c2", now_ms=now + 150)
        first = await client.graph.properties(node_id)
        assert await client.broker.ack(queue, node_id, consumer="c2", now_ms=now + 160)
        second = await client.graph.properties(node_id)

        assert first is not None and second is not None
        assert second["status"] == "acked"
        assert second["acked_at_ms"] == first["acked_at_ms"] == now + 150
    finally:
        await client.close()
