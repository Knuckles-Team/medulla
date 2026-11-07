"""Fixture contract source (MEDULLA-CQ-CONN-R067.8)."""

from __future__ import annotations

import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from fixture_source import FixtureSource

pytestmark = pytest.mark.no_engine

BODIES: dict[str, tuple[dict | None, str | None]] = {
    "mcp_tool": (
        {"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "list_models"}},
        None,
    ),
    "a2a_skill": ({"skill": "list_models"}, None),
    "graphql_query": (
        {"operationName": "list_models", "query": "{ models { id } }"},
        None,
    ),
    "http_endpoint": (None, "/list_models"),
}


def _request(url: str, body: dict | None) -> tuple[int, bytes]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        url, data=data, method="GET" if data is None else "POST"
    )
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.8")
@pytest.mark.parametrize("kind", sorted(BODIES))
def test_serves_recording_counts_requests_and_stops(tmp_path: Path, kind: str) -> None:
    recording = {"models": [{"id": "m1", "name": "Invoice approval"}]}
    (tmp_path / "list_models.json").write_text(json.dumps(recording), encoding="utf-8")
    (tmp_path / "delete_model.json").write_text("{}", encoding="utf-8")
    source = FixtureSource(kind, tmp_path, operations=["list_models"]).start()
    try:
        body, path = BODIES[kind]
        url = source.url + (path or "/")
        status, payload = _request(url, body)
        assert status == 200
        assert json.loads(payload) == recording
        assert source.request_count == 1

        other = (
            json.loads(json.dumps(body).replace("list_models", "delete_model"))
            if body
            else None
        )
        status, _ = _request(source.url + ("/delete_model" if path else "/"), other)
        assert status == 404, "an operation outside the contract is not answered"
        assert source.request_count == 2

        source.stop()
        status, _ = _request(url, body)
        assert status == 503
        assert source.request_count == 3
    finally:
        source.close()


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.8")
def test_unknown_kind_is_refused(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="unknown contract kind"):
        FixtureSource("soap_call", tmp_path)
