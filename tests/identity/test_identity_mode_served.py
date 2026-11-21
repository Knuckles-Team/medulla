"""Served identity-mode transition authority (MEDULLA-IDENTITY-R003.7).

The served ``Identity`` method's mode transition needs the ``identity:admin`` scope. A
principal that holds only a read scope is refused, and the refusal leaves the engine's
identity configuration unchanged.
"""

import asyncio
import os

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient
from medulla.generated.models import (
    AuthMode,
    IdentityOpConfigGet,
    IdentityOpConfigTransition,
    MethodIdentityParams,
    ModeTransition,
)
from medulla.generated.security import send_identity

try:
    from conftest import request_context
except ModuleNotFoundError:  # package-root import
    from tests.conftest import request_context


def _send(client, op):
    params = MethodIdentityParams(op=op).model_dump(mode="json", exclude_none=True)
    return asyncio.run_coroutine_threadsafe(
        send_identity(client._client, params), client._loop
    ).result(timeout=30)


def _connect(scopes):
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal="mode-reader", scopes=scopes),
    )


@pytest.mark.spec("MEDULLA-IDENTITY-R003.7")
def test_non_admin_cannot_transition():
    reader = _connect(["identity:read"])
    transition = IdentityOpConfigTransition(
        family="config",
        op="transition",
        request=ModeTransition(
            expected_epoch=0, issuer_kid="mode-test-kid", to=AuthMode.EXTERNAL
        ),
    )
    with pytest.raises((EngineResponseError, RuntimeError)):
        _send(reader, transition)

    # The refusal changed nothing: the identity configuration reads as it did.
    get = IdentityOpConfigGet(family="config", op="get")
    before = _send(reader, get)
    with pytest.raises((EngineResponseError, RuntimeError)):
        _send(reader, transition)
    assert _send(reader, get) == before
