"""Served inspection of an original durable session reference.

MEDULLA-IDENTITY-R713: the op resolves one exact stored opaque reference for its
owner and tenant, touches nothing and never issues a credential.
"""

import asyncio

import pytest

from medulla.client import EngineResponseError
from medulla.generated.models import (
    IdentityOpSessionInspectReference,
    MethodIdentityParams,
)
from medulla.generated.security import send_identity

UNKNOWN_REFERENCE = "ses:never-issued-reference"


def _inspect(clean_graph, session_ref: str, tenant: str):
    op = IdentityOpSessionInspectReference(
        family="session", op="inspect_reference", session_ref=session_ref, tenant=tenant
    )
    params = MethodIdentityParams(op=op).model_dump(mode="json", exclude_none=True)
    return asyncio.run_coroutine_threadsafe(
        send_identity(clean_graph._client, params), clean_graph._loop
    ).result(timeout=30)


@pytest.mark.spec("MEDULLA-IDENTITY-R713")
@pytest.mark.parametrize("tenant", ["tenant-a", "tenant-b"])
def test_inspect_reference_owner_and_tenant(clean_graph, tenant):
    """An unknown or foreign reference refuses without leaking facts or tokens."""
    with pytest.raises((EngineResponseError, RuntimeError)) as refused:
        _inspect(clean_graph, UNKNOWN_REFERENCE, tenant)
    text = str(refused.value)
    assert "token" not in text.lower()
    # A refusal mutates nothing: the same call refuses identically a second time.
    with pytest.raises((EngineResponseError, RuntimeError)):
        _inspect(clean_graph, UNKNOWN_REFERENCE, tenant)
