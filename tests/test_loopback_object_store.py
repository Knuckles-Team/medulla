"""MEDULLA-OBJECT-STORAGE-BACKEND-R002.3: the loopback object-store fixture."""

from __future__ import annotations

import urllib.error
import urllib.request

import pytest
from _loopback_object_store import LoopbackObjectStore, loopback_object_store  # noqa: F401

pytestmark = pytest.mark.no_engine


def _request(
    store: LoopbackObjectStore,
    method: str,
    key: str,
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, str], bytes]:
    request = urllib.request.Request(
        f"{store.url}/{store.bucket}/{key}",
        data=body,
        method=method,
        headers=headers or {},
    )
    try:
        with urllib.request.urlopen(request) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


# proves R002.3: Loopback object-store fixture for served Python tests
@pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R002.3")
def test_conditional_puts_and_listing(
    loopback_object_store: LoopbackObjectStore,  # noqa: F811
) -> None:
    store = loopback_object_store
    status, headers, _ = _request(
        store, "PUT", "root/a", b"one", {"If-None-Match": "*"}
    )
    assert status == 200
    first_etag = headers["ETag"]
    assert _request(store, "PUT", "root/a", b"two", {"If-None-Match": "*"})[0] == 412

    assert _request(store, "PUT", "root/a", b"two", {"If-Match": '"stale"'})[0] == 412
    status, headers, _ = _request(
        store, "PUT", "root/a", b"two", {"If-Match": first_etag}
    )
    assert status == 200
    current_etag = headers["ETag"]
    assert current_etag != first_etag
    assert (
        _request(store, "PUT", "root/missing", b"x", {"If-Match": first_etag})[0] == 412
    )

    assert _request(store, "PUT", "root/b", b"three")[0] == 200
    assert _request(store, "PUT", "other/c", b"four")[0] == 200
    assert store.keys("root/") == ["root/a", "root/b"]
    listing = (
        urllib.request.urlopen(f"{store.url}/{store.bucket}?list-type=2&prefix=root/")
        .read()
        .decode()
    )
    assert "<Key>root/a</Key>" in listing and "<Key>root/b</Key>" in listing
    assert "other/c" not in listing
    assert "<IsTruncated>false</IsTruncated>" in listing

    assert _request(store, "GET", "root/nope")[0] == 404

    store.corrupt("root/a")
    status, headers, body = _request(store, "GET", "root/a")
    assert status == 200
    assert body != b"two" and len(body) == 3
    assert headers["ETag"] == current_etag
