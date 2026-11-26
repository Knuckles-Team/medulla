"""Served trust mark (MEDULLA-SRC-AUTH-R011.3).

The mutation gateway stamps the trust mark of content that enters through `AddNode`:
a tool
result is `untrusted`, a forged mark key is dropped, and the mark is sticky: lowering
it to
`trusted` through `CompareAndSetNodeFields` needs an operator approval id.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import register_commons_agents, request_context, tenant_request_context

from medulla.client import EngineResponseError, SyncMedullaClient


def _agent_client() -> SyncMedullaClient:
    """A client for a registered, non-System agent: the gateway stamps its writes."""
    agent = f"trust-mark-{uuid.uuid4().hex[:8]}"
    register_commons_agents(agent)
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent),
    )


# proves R011.6: Hot-path proof for the trust mark
@pytest.mark.spec("MEDULLA-SRC-AUTH-R011.3")
@pytest.mark.spec("MEDULLA-SRC-AUTH-R011.6")
def test_tool_result_is_untrusted_and_sticky() -> None:
    client = _agent_client()
    try:
        node = f"trust-doc-{uuid.uuid4().hex[:8]}"
        client.nodes.add(
            node,
            {
                "type": "Document",
                "content_origin": "tool_result",
                "trust_mark": "trusted",
                "marked_by": "forged",
                "trust_mark_approval_id": "forged-approval",
            },
        )
        stored = client.nodes.properties(node)
        assert stored["trust_mark"] == "untrusted"
        assert stored["marked_by"] == "tool_result_untrusted"
        assert isinstance(stored["marked_at_generation"], int)
        assert "trust_mark_approval_id" not in stored

        # An unknown origin is refused and stores nothing.
        refused_node = f"trust-bad-{uuid.uuid4().hex[:8]}"
        with pytest.raises(EngineResponseError) as unknown:
            client.nodes.add(refused_node, {"content_origin": "telepathy"})
        assert "CONTENT_ORIGIN_UNKNOWN" in str(unknown.value)
        assert client.nodes.properties(refused_node) is None

        # Sticky: lowering to trusted needs an operator approval id.
        with pytest.raises(EngineResponseError) as downgrade:
            client.nodes.compare_and_set(node, {}, {"trust_mark": "trusted"})
        assert "TRUST_MARK_DOWNGRADE_REFUSED" in str(downgrade.value)
        assert client.nodes.properties(node)["trust_mark"] == "untrusted"

        # A reserved field without the mark is refused.
        with pytest.raises(EngineResponseError) as lone:
            client.nodes.compare_and_set(node, {}, {"marked_by": "me"})
        assert "TRUST_MARK_FIELDS_RESERVED" in str(lone.value)

        # With an approval id the mark lowers and records the approval.
        assert client.nodes.compare_and_set(
            node,
            {},
            {"trust_mark": "trusted", "trust_mark_approval_id": "operator-approval-1"},
        )
        approved = client.nodes.properties(node)
        assert approved["trust_mark"] == "trusted"
        assert approved["marked_by"] == "operator_approval"
        assert approved["trust_mark_approval_id"] == "operator-approval-1"

        # Another tenant cannot read the node: its context is refused at the door.
        with pytest.raises(EngineResponseError) as foreign:
            stranger = SyncMedullaClient.connect(
                socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
                verified_context=tenant_request_context("tenant-other"),
            )
            try:
                stranger.nodes.properties(node)
            finally:
                stranger.close()
        assert "AUTH_TENANT_MISMATCH" in str(foreign.value)
    finally:
        client.close()
