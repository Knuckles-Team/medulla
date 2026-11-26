"""MEDULLA-IDENTITY-R744: the delegation claim is a tag-3 envelope trailer."""

from __future__ import annotations

import pytest

from medulla import client as medulla_client

pytestmark = pytest.mark.no_engine


def _claims(**extra: object) -> dict[str, object]:
    return {
        "principal": "service:host",
        "tenant": "tenant:test",
        "audience": "medulla",
        "agent_id": "service:host",
        "roles": [],
        "scopes": ["identity:delegate"],
        "policy_version": "policy-test",
        "delegation": [],
        **extra,
    }


def _envelope(context: dict[str, object]) -> medulla_client._EnvelopeV2:
    return medulla_client._EnvelopeV2(
        request_id=7,
        graph="g",
        method="Ping",
        body_hash="hash",
        context=context,  # type: ignore[arg-type]
        roles=[],
        scopes=["identity:delegate"],
        delegation=[],
        timestamp=111,
        nonce="nonce",
        idempotency_key="idem",
        node_id=None,
        priority=None,
        delegation_id=medulla_client._stripped_claim(context, "delegation_id"),
    )


@pytest.mark.spec("MEDULLA-IDENTITY-R744")
def test_the_delegation_claim_is_a_tag_three_trailer() -> None:
    absent = _envelope(_claims())
    delegated = _envelope(_claims(delegation_id="delegation:run-1"))
    plain_bytes = medulla_client._envelope_v2_bytes(absent)
    with_bytes = medulla_client._envelope_v2_bytes(delegated)
    assert with_bytes.startswith(plain_bytes)
    assert with_bytes[len(plain_bytes)] == 3
    assert with_bytes.endswith(b"delegation:run-1")
    other = _envelope(_claims(delegation_id="delegation:run-2"))
    assert medulla_client._envelope_v2_bytes(other) != with_bytes
    assert "delegation_id" not in medulla_client._envelope_v2_context_payload(absent)
    payload = medulla_client._envelope_v2_context_payload(delegated)
    assert payload["delegation_id"] == "delegation:run-1"


@pytest.mark.spec("MEDULLA-IDENTITY-R744")
def test_the_delegation_claim_is_a_non_empty_optional_string() -> None:
    accepted = medulla_client.validate_request_context(
        _claims(delegation_id="delegation:run-1")
    )
    assert accepted["delegation_id"] == "delegation:run-1"
    assert "delegation_id" not in medulla_client.validate_request_context(_claims())
    for bad in ("", "  ", 7):
        with pytest.raises(ValueError, match="delegation_id"):
            medulla_client.validate_request_context(_claims(delegation_id=bad))
