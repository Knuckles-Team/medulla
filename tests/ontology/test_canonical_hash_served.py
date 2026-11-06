"""MEDULLA-TYPED-PACKS-R809 acceptance over the real compiled engine.

The session fixture in conftest starts and authenticates the native server;
the served `OntologyCanonicalHash` method is called over its socket.
"""

from __future__ import annotations

import os

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, MedullaClient

FIRST = """@prefix ex: <http://example.org/> .
ex:a ex:p _:x .
_:x ex:q "v" .
ex:b ex:p ex:a .
"""

# Another prefix, another statement order and another blank-node label.
SECOND = """@prefix e: <http://example.org/> .
e:b e:p e:a .
_:renamed e:q "v" .
e:a e:p _:renamed .
"""

DIFFERENT = '<http://example.org/a> <http://example.org/p> "other" .'


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R809")
@pytest.mark.asyncio
async def test_reordered_serializations_hash_equal() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        first = await client._send("OntologyCanonicalHash", {"turtle": FIRST})
        second = await client._send("OntologyCanonicalHash", {"turtle": SECOND})
        other = await client._send("OntologyCanonicalHash", {"turtle": DIFFERENT})
        assert first["hash"] == second["hash"]
        assert len(first["hash"]) == 64
        assert first["hash"] == first["hash"].lower()
        assert other["hash"] != first["hash"]

        with pytest.raises(EngineResponseError, match="TURTLE_INVALID"):
            await client._send("OntologyCanonicalHash", {"turtle": "<a> <b>"})
    finally:
        await client.close()
