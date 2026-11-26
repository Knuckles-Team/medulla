"""Layered configuration read (MEDULLA-IDENTITY-R740).

L4: every call goes through the running engine. The company layer, the layer of a
team the caller belongs to and the caller's own user layer come back in application
order; a layer of a team the caller is not in, another tenant's layer and an absent
layer are left out alike.
"""

from __future__ import annotations

import os
import uuid

import msgpack
import pytest
from conftest import request_context

from medulla.client import MedullaClient

GRAPH = "__commons__"


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _layer(tenant: str, layer: str, scope: str, settings: dict) -> dict:
    node_id = f"config-layer:{len(tenant)}:{tenant}:{layer}:{scope}"
    return {
        "op": "upsert_node",
        "id": node_id,
        "properties": {
            "type": "ConfigLayer",
            "tenant_id": tenant,
            "layer": layer,
            "scope": scope,
            "revision": 2,
            "settings": settings,
            "_visibility": "public",
        },
    }


@pytest.mark.spec("MEDULLA-IDENTITY-R740")
@pytest.mark.asyncio
async def test_layers_are_company_member_teams_and_own_user_in_order(
    start_medulla_server,
) -> None:
    from conftest import TEST_TENANT

    suffix = uuid.uuid4().hex[:8]
    person = f"layered-{suffix}"
    context = request_context(agent_id=person, principal=person)
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"], verified_context=context
    )
    try:
        mine, other = f"team-mine-{suffix}", f"team-other-{suffix}"
        user_scope = client_persistence_id(person)
        await client.lifecycle.batch_update(
            [
                {"op": "upsert_node", "id": person, "properties": {"type": "Entity"}},
                {"op": "upsert_node", "id": mine, "properties": {"type": "Entity"}},
                {
                    "op": "upsert_edge",
                    "source": person,
                    "target": mine,
                    "properties": {"relationship": "memberOf"},
                },
                _layer(TEST_TENANT, "company", "", {"skills": ["a"]}),
                _layer(TEST_TENANT, "team", mine, {"skills": ["b"]}),
                _layer(TEST_TENANT, "team", other, {"skills": ["c"]}),
                _layer("some-other-tenant", "company", "", {"skills": ["z"]}),
                _layer(TEST_TENANT, "user", user_scope, {"hooks": []}),
            ]
        )
        answer = _body(await client._send("ConfigLayers", {"request": {}}, GRAPH))
        order = [(row["layer"], row["scope"]) for row in answer["layers"]]
        assert order == [("company", ""), ("team", mine), ("user", user_scope)], order
        assert answer["layers"][0]["settings"] == {"skills": ["a"]}
        assert answer["layers"][0]["revision"] == 2
    finally:
        await client.close()


def client_persistence_id(principal: str) -> str:
    import hashlib

    return "principal:sha256:" + hashlib.sha256(principal.encode()).hexdigest()
