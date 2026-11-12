"""Served credential observation: resolve with ``refresh=false``.

MEDULLA-IDENTITY-R712: ``SessionOp::Resolve`` carries an optional ``refresh``
flag. The served ``Identity`` method accepts ``refresh=false`` and an omitted
flag alike, and an unresolvable credential is refused identically in both
spellings without leaking the credential or changing engine state.
"""

import asyncio

import pytest

from medulla.client import EngineResponseError
from medulla.generated.models import (
    IdentityOpSessionResolve,
    MethodIdentityParams,
    SessionTouch,
)
from medulla.generated.security import send_identity

UNKNOWN_TOKEN = "never-issued-session-token"


def _resolve(clean_graph, refresh):
    request = SessionTouch(session_token=UNKNOWN_TOKEN, refresh=refresh)
    op = IdentityOpSessionResolve(family="session", op="resolve", request=request)
    params = MethodIdentityParams(op=op).model_dump(mode="json", exclude_none=True)
    return asyncio.run_coroutine_threadsafe(
        send_identity(clean_graph._client, params), clean_graph._loop
    ).result(timeout=30)


@pytest.mark.spec("MEDULLA-IDENTITY-R712")
def test_refresh_false_does_not_touch(clean_graph):
    """Observe-only resolve refuses an unknown credential like a refreshing one."""
    messages = []
    for refresh in (False, None, True):
        with pytest.raises((EngineResponseError, RuntimeError)) as refused:
            _resolve(clean_graph, refresh)
        text = str(refused.value)
        assert UNKNOWN_TOKEN not in text
        messages.append(text)
    # The refusal does not depend on the refresh flag, and repeating the
    # observation changes nothing: it refuses the same way a second time.
    assert len(set(messages)) == 1
    with pytest.raises((EngineResponseError, RuntimeError)):
        _resolve(clean_graph, False)
