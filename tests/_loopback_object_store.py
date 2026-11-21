"""Loopback object store and object-store engine launcher for served tests.

MEDULLA-OBJECT-STORAGE-BACKEND-R002.3. `LoopbackObjectStore` is an in-memory,
path-style S3 endpoint bound to 127.0.0.1 only, so it accepts any request
signature. `spawn_object_store_engine` starts a real engine whose
`MEDULLA_STORAGE_BACKEND` spec points at it. No production type is added.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import subprocess
import threading
from collections.abc import Iterator
from email.utils import formatdate
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit
from xml.sax.saxutils import escape

import pytest
from conftest import (
    request_context,
    find_server_binary,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import SyncMedullaClient

BUCKET = "medulla-test"
ENDPOINT_VARIABLE = "MEDULLA_TEST_OBJECT_ENDPOINT"
BUCKET_VARIABLE = "MEDULLA_TEST_OBJECT_BUCKET"
ROOT_VARIABLE = "MEDULLA_TEST_OBJECT_ROOT"
_REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


class HeadHold:
    """A held conditional `HEAD` update (MEDULLA-OBJECT-STORAGE-BACKEND-R003.1).

    The loopback store parks the next conditional `HEAD` put without applying or
    answering it: the engine has uploaded the segment and the next manifest and is
    waiting on the fence. `reached` is set once the put is parked; `key` names the
    `HEAD` object it targeted. The hold lasts until `release()`.
    """

    def __init__(self) -> None:
        self.reached = threading.Event()
        self.key = ""
        self._released = threading.Event()

    def release(self) -> None:
        self._released.set()


def _etag(body: bytes) -> str:
    return '"' + hashlib.sha256(body).hexdigest()[:32] + '"'


class LoopbackObjectStore:
    """An in-memory S3-compatible store with conditional puts and listing."""

    bucket = BUCKET

    def __init__(self) -> None:
        self._objects: dict[str, bytes] = {}
        self._etags: dict[str, str] = {}
        self._lock = threading.Lock()
        self._head_hold: HeadHold | None = None
        self.engines: list[subprocess.Popen] = []
        store = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, format: str, *args: object) -> None:
                return

            def _key(self) -> tuple[str, str]:
                parts = urlsplit(self.path)
                path = unquote(parts.path).lstrip("/")
                bucket, _, key = path.partition("/")
                return bucket, key

            def _reply(
                self,
                status: int,
                body: bytes = b"",
                headers: dict[str, str] | None = None,
                head: bool = False,
            ) -> None:
                self.send_response(status)
                for name, value in (headers or {}).items():
                    self.send_header(name, value)
                if "Content-Length" not in (headers or {}):
                    self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                if not head and body:
                    self.wfile.write(body)

            def _read_body(self) -> bytes:
                if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                    chunks = []
                    while True:
                        size = int(self.rfile.readline().strip().split(b";")[0], 16)
                        if size == 0:
                            self.rfile.readline()
                            break
                        chunks.append(self.rfile.read(size))
                        self.rfile.readline()
                    return b"".join(chunks)
                return self.rfile.read(int(self.headers.get("Content-Length", "0")))

            def do_PUT(self) -> None:  # noqa: N802
                _, key = self._key()
                body = self._read_body()
                if_none_match = self.headers.get("If-None-Match")
                if_match = self.headers.get("If-Match")
                if if_match is not None and key.endswith("/HEAD"):
                    with store._lock:
                        hold, store._head_hold = store._head_hold, None
                    if hold is not None:
                        hold.key = key
                        hold.reached.set()
                        hold._released.wait(timeout=120)
                        self.close_connection = True
                        return
                with store._lock:
                    current = store._etags.get(key)
                    if if_none_match == "*" and key in store._objects:
                        return self._reply(412)
                    if if_match is not None and current != if_match:
                        return self._reply(412)
                    store._objects[key] = body
                    store._etags[key] = _etag(body)
                    etag = store._etags[key]
                self._reply(200, headers={"ETag": etag})

            def _get(self, head: bool) -> None:
                bucket, key = self._key()
                query = parse_qs(urlsplit(self.path).query)
                if not key and query.get("list-type"):
                    return self._list(bucket, query.get("prefix", [""])[0], head)
                with store._lock:
                    body = store._objects.get(key)
                    etag = store._etags.get(key, "")
                if body is None:
                    error = (
                        '<?xml version="1.0" encoding="UTF-8"?><Error>'
                        "<Code>NoSuchKey</Code><Message>The specified key does not "
                        f"exist.</Message><Key>{escape(key)}</Key></Error>"
                    ).encode()
                    return self._reply(
                        404, error, {"Content-Type": "application/xml"}, head=head
                    )
                headers = {
                    "ETag": etag,
                    "Content-Length": str(len(body)),
                    "Last-Modified": formatdate(usegmt=True),
                    "Accept-Ranges": "none",
                }
                self._reply(200, body, headers, head=head)

            def _list(self, bucket: str, prefix: str, head: bool) -> None:
                with store._lock:
                    entries = [
                        (key, len(store._objects[key]), store._etags[key])
                        for key in sorted(store._objects)
                        if key.startswith(prefix)
                    ]
                contents = "".join(
                    f"<Contents><Key>{escape(key)}</Key>"
                    f"<LastModified>2026-01-01T00:00:00.000Z</LastModified>"
                    f"<ETag>{escape(etag)}</ETag><Size>{size}</Size>"
                    f"<StorageClass>STANDARD</StorageClass></Contents>"
                    for key, size, etag in entries
                )
                body = (
                    '<?xml version="1.0" encoding="UTF-8"?>'
                    "<ListBucketResult>"
                    f"<Name>{escape(bucket)}</Name><Prefix>{escape(prefix)}</Prefix>"
                    f"<KeyCount>{len(entries)}</KeyCount><MaxKeys>1000</MaxKeys>"
                    f"<IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
                ).encode()
                self._reply(200, body, {"Content-Type": "application/xml"}, head=head)

            def do_GET(self) -> None:  # noqa: N802
                self._get(head=False)

            def do_HEAD(self) -> None:  # noqa: N802
                self._get(head=True)

            def do_DELETE(self) -> None:  # noqa: N802
                _, key = self._key()
                with store._lock:
                    store._objects.pop(key, None)
                    store._etags.pop(key, None)
                self._reply(204)

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self._server.server_address[1]}"

    def keys(self, prefix: str) -> list[str]:
        with self._lock:
            return sorted(key for key in self._objects if key.startswith(prefix))

    def get(self, key: str) -> bytes:
        with self._lock:
            return self._objects[key]

    def hold_head_update(self) -> HeadHold:
        """Park the next conditional `HEAD` put (the commit fence, step 4): the
        kill hook of the commit tests. Create-only puts (a bootstrap) pass."""

        hold = HeadHold()
        with self._lock:
            self._head_hold = hold
        return hold

    def kill_engine(self) -> None:
        """SIGKILL the engine started last by `spawn_object_store_engine`."""

        process = self.engines[-1]
        process.kill()
        process.wait()

    def corrupt(self, key: str) -> None:
        """Flip the first byte of `key` and keep its ETag."""

        with self._lock:
            body = self._objects[key]
            self._objects[key] = bytes([body[0] ^ 0xFF]) + body[1:]

    def stop(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)


@pytest.fixture
def loopback_object_store() -> Iterator[LoopbackObjectStore]:
    store = LoopbackObjectStore()
    try:
        yield store
    finally:
        store.stop()


def _server_command(*arguments: str) -> list[str]:
    binary = find_server_binary()
    if binary is not None:
        return [binary, *arguments]
    return [
        "cargo", "run", "--features", "full", "--bin", "medulla-server", "--",
        *arguments,
    ]  # fmt: skip


_graph_shard_digest: str | None = None


def graph_shard_layout_digest() -> str:
    """The `graph_shard` line of `medulla-server store-upgrade layouts`, once per session."""

    global _graph_shard_digest
    if _graph_shard_digest is None:
        output = subprocess.run(
            _server_command("store-upgrade", "layouts"),
            cwd=_REPO_ROOT,
            capture_output=True,
            text=True,
            check=True,
            timeout=600,
        ).stdout
        for line in output.splitlines():
            name, _, digest = line.partition(" ")
            if name == "graph_shard":
                _graph_shard_digest = digest.strip()
                break
        else:
            raise AssertionError(f"no graph_shard layout line in:\n{output}")
    return _graph_shard_digest


@contextlib.contextmanager
def spawn_object_store_engine(
    runtime_dir: Path,
    persist_dir: Path,
    store: LoopbackObjectStore,
    root: str,
    *,
    expect_exit: bool = False,
) -> Iterator[SyncMedullaClient | tuple[int, str]]:
    """Start an engine whose durable store is the object-store root `root`.

    Yields a client, or with `expect_exit=True` waits for the process to exit
    and yields `(returncode, log_text)`.
    """

    runtime_dir = Path(runtime_dir)
    persist_dir = Path(persist_dir)
    runtime_dir.mkdir(parents=True, exist_ok=True)
    persist_dir.mkdir(parents=True, exist_ok=True)
    socket_path = str(runtime_dir / "engine.sock")
    log_path = runtime_dir / "engine.log"
    env = strict_server_env(
        str(runtime_dir / "security"),
        auth_secret="test-medulla-secret",
        persist_dir=str(persist_dir),
    )
    env.setdefault("MEDULLA_ENCRYPTION_KEY", "test-medulla-encryption-key")
    env.update(
        {
            ENDPOINT_VARIABLE: store.url,
            BUCKET_VARIABLE: store.bucket,
            ROOT_VARIABLE: root,
            "AWS_ACCESS_KEY_ID": "test",
            "AWS_SECRET_ACCESS_KEY": "test",
            "AWS_REGION": "us-east-1",
            "MEDULLA_STORAGE_BACKEND": json.dumps(
                {
                    "variant": "object_store",
                    "endpoint": ENDPOINT_VARIABLE,
                    "bucket": BUCKET_VARIABLE,
                    "root": ROOT_VARIABLE,
                    "durability": "object_store_conditional_put",
                    "layout_digest": graph_shard_layout_digest(),
                }
            ),
        }
    )
    process = spawn_server(
        _server_command("--socket-path", socket_path),
        cwd=_REPO_ROOT,
        env={**os.environ, **env},
        log_path=log_path,
    )
    client = None
    store.engines.append(process)
    try:
        if expect_exit:
            from conftest import server_timeout

            returncode = process.wait(timeout=server_timeout())
            yield returncode, log_path.read_text(errors="replace")
            return
        wait_for_server(process, socket_path, name="object-store engine", log_path=log_path)
        client = SyncMedullaClient.connect(
            socket_path=socket_path, verified_context=request_context()
        )
        yield client
    finally:
        if client is not None:
            with contextlib.suppress(Exception):
                client.close()
        stop_server(process, name="object-store engine", log_path=log_path)
