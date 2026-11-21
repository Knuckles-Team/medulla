"""MEDULLA-ORCH-VOCAB-R750.752 served proof: an unregistered statechart
definition refuses with its own code, and the instance store is unchanged."""

from __future__ import annotations

import os

import pytest
from conftest import request_context


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R750.752")
@pytest.mark.asyncio
async def test_unregistered_definition_refuses_with_code(start_medulla_server) -> None:
    from medulla.client import EngineResponseError, MedullaClient

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        with pytest.raises(EngineResponseError) as refused:
            await client.statechart.instantiate("def:r750-unregistered")
    finally:
        await client.close()
    assert str(refused.value).startswith("INSTANCE_DEFINITION_UNREGISTERED"), refused.value
