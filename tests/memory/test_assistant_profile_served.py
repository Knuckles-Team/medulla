"""MEDULLA-DECISION-ENGINE-R656.2 served assistant profiles: a profile and a
document set are saved, shared by role and read back over the wire through the
``assistant_profile`` client method.

Every save raises the revision by one under a compare-and-set; a stale save is
refused. A caller that holds no share reads the same non-disclosing envelope as
for an absent profile; a Read share lets the holder read and list; only the
owner or an Admin share may save.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import TEST_AGENT_ID, TEST_SIGNER_KEY, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient


async def _refusal(call: Any) -> str:
    try:
        await call
    except Exception as error:  # the typed refusal carries the code
        return str(error)
    raise AssertionError("the call was expected to be refused")


async def _journey(tag: str, role: str) -> dict[str, Any]:
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    owner_id, reader_id, outsider_id = (
        f"service:{tag}-owner",
        f"service:{tag}-reader",
        f"service:{tag}-outsider",
    )
    system = await MedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    clients = [system]
    try:
        await system.rbac.add_role(role)
        for agent_id, roles in (
            (owner_id, []),
            (reader_id, [role]),
            (outsider_id, []),
        ):
            await system.consensus.register_identity(
                agent_id,
                "Agent",
                [],
                roles,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )

        async def client_for(agent_id: str) -> dc.DecisionClient:
            connected = await MedullaClient.connect(
                socket_path=socket_path,
                verified_context=request_context(agent_id=agent_id),
            )
            clients.append(connected)
            return dc.DecisionClient(client=connected)

        owner = await client_for(owner_id)
        reader = await client_for(reader_id)
        outsider = await client_for(outsider_id)

        profile_id = f"profile-{tag}"
        base = dict(
            profile_id=profile_id,
            name="Research helper",
            owner_principal=owner_id,
            instructions="Answer from the cited documents only.",
            tool_allowlist=["web_search"],
            preferred_model_profile_refs=["large"],
            max_context_tokens=32_000,
        )
        saved = await owner.assistant_profile(
            dc.put_profile_op(dc.assistant_profile_wire(**base))
        )
        resaved = await owner.assistant_profile(
            dc.put_profile_op(
                dc.assistant_profile_wire(
                    **base,
                    revision=saved["revision"],
                    shares=[dc.profile_share(role, "Read")],
                )
            )
        )
        stale = await _refusal(
            owner.assistant_profile(
                dc.put_profile_op(
                    dc.assistant_profile_wire(**base, revision=saved["revision"])
                )
            )
        )
        outsider_read = await _refusal(
            outsider.assistant_profile(dc.get_profile_op(profile_id))
        )
        absent_read = await _refusal(
            outsider.assistant_profile(dc.get_profile_op(f"absent-{tag}"))
        )
        shared_read = await reader.assistant_profile(dc.get_profile_op(profile_id))
        shared_list = await reader.assistant_profile(dc.list_profiles_op())
        outsider_list = await outsider.assistant_profile(dc.list_profiles_op())
        reader_save = await _refusal(
            reader.assistant_profile(
                dc.put_profile_op(
                    dc.assistant_profile_wire(
                        **{**base, "name": "Renamed"},
                        revision=resaved["revision"],
                        shares=[dc.profile_share(role, "Read")],
                    )
                )
            )
        )
        admin_saved = await owner.assistant_profile(
            dc.put_profile_op(
                dc.assistant_profile_wire(
                    **base,
                    revision=resaved["revision"],
                    shares=[
                        dc.profile_share(role, "Read"),
                        dc.profile_share(role, "Admin"),
                    ],
                )
            )
        )
        reader_admin_save = await reader.assistant_profile(
            dc.put_profile_op(
                dc.assistant_profile_wire(
                    **{**base, "name": "Renamed"},
                    revision=admin_saved["revision"],
                    shares=[
                        dc.profile_share(role, "Read"),
                        dc.profile_share(role, "Admin"),
                    ],
                )
            )
        )

        set_id = f"set-{tag}"
        members = ["urn:doc:1", "urn:doc:2"]
        set_saved = await owner.assistant_profile(
            dc.put_document_set_op(
                dc.document_set_wire(
                    set_id=set_id,
                    name="Policies",
                    owner_principal=owner_id,
                    members=members,
                    shares=[dc.profile_share(role, "Read")],
                )
            )
        )
        set_read = await reader.assistant_profile(dc.get_document_set_op(set_id))
        set_list = await reader.assistant_profile(dc.list_document_sets_op())
        set_outsider = await _refusal(
            outsider.assistant_profile(dc.get_document_set_op(set_id))
        )
        not_read_only = await _refusal(
            owner.assistant_profile(
                dc.put_document_set_op(
                    dc.document_set_wire(
                        set_id=f"query-{tag}",
                        name="Mutating query",
                        owner_principal=owner_id,
                        saved_query="INSERT INTO x VALUES (1)",
                    )
                )
            )
        )
        return {
            "saved": saved,
            "resaved": resaved,
            "stale": stale,
            "outsider_read": outsider_read,
            "absent_read": absent_read,
            "shared_read": shared_read,
            "shared_list": shared_list,
            "outsider_list": outsider_list,
            "reader_save": reader_save,
            "admin_saved": admin_saved,
            "reader_admin_save": reader_admin_save,
            "set_saved": set_saved,
            "set_read": set_read,
            "set_list": set_list,
            "set_outsider": set_outsider,
            "not_read_only": not_read_only,
            "profile_id": profile_id,
            "set_id": set_id,
            "members": members,
            "owner_id": owner_id,
        }
    finally:
        for connected in clients:
            await connected.close()


# spec: MEDULLA-DECISION-ENGINE-R656.2
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R656.2")
def test_profile_and_document_set_round_trip(clean_graph):
    tag = uuid.uuid4().hex[:10]
    outcome = asyncio.run(_journey(tag, f"analyst-{tag}"))

    # Each save raises the revision by one.
    assert outcome["saved"]["revision"] == 1
    assert outcome["resaved"]["revision"] == 2
    assert outcome["admin_saved"]["revision"] == 3
    assert outcome["saved"]["owner_principal"] == outcome["owner_id"]
    assert "PROFILE_REVISION_CONFLICT" in outcome["stale"]

    # No share: the caller reads the one non-disclosing envelope, present or not.
    assert "PROFILE_NOT_VISIBLE" in outcome["outsider_read"]
    assert "PROFILE_NOT_VISIBLE" in outcome["absent_read"]
    assert outcome["outsider_list"] == []

    # A Read share reads and lists the profile, but cannot save it.
    assert outcome["shared_read"] == outcome["resaved"]
    assert [p["profile_id"] for p in outcome["shared_list"]] == [outcome["profile_id"]]
    assert "ACCESS_DENIED" in outcome["reader_save"]

    # An Admin share may save.
    assert outcome["reader_admin_save"]["revision"] == 4
    assert outcome["reader_admin_save"]["name"] == "Renamed"
    assert outcome["reader_admin_save"]["owner_principal"] == outcome["owner_id"]

    # A document set round-trips with its members and sharing.
    assert outcome["set_saved"]["kind"] == {"members": outcome["members"]}
    assert outcome["set_read"] == outcome["set_saved"]
    assert [s["set_id"] for s in outcome["set_list"]] == [outcome["set_id"]]
    assert "DOCUMENT_SET_NOT_FOUND" in outcome["set_outsider"]
    assert "DOCUMENT_SET_QUERY_NOT_READ_ONLY" in outcome["not_read_only"]
