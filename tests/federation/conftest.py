"""Fixtures of the served federation acceptance tests.

``pg_source`` is a disposable PostgreSQL container the served engine reads as an
external SQL source (MEDULLA-UNIFIED-DATA-PLANE-R038.3.6). The engine's outbound
allow-list is read from the environment of the server process, which the session
fixture starts from this process's environment, so the loopback endpoint is named
here, at collection time.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import threading
import time
import uuid
from collections.abc import Iterator
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


PG_PORT = _free_port()
PG_PASSWORD = "federation-test"
GATEWAY_PORT = _free_port()
os.environ["MEDULLA_FEDERATION_ALLOW"] = ",".join(
    part
    for part in (
        os.environ.get("MEDULLA_FEDERATION_ALLOW", ""),
        f"127.0.0.1:{PG_PORT}",
        f"127.0.0.1:{GATEWAY_PORT}",
    )
    if part
)
# The served engine reaches connector tools through the fleet gateway; the tests stand a
# loopback gateway on this port. The web cache TTL is two seconds so a test crosses it.
os.environ["MEDULLA_CONNECTOR_GATEWAY_URL"] = f"http://127.0.0.1:{GATEWAY_PORT}/gateway"
os.environ["MEDULLA_FEDERATION_WEB_CACHE_TTL_S"] = "2"


@dataclass(frozen=True)
class PgSource:
    """A running PostgreSQL container and the ways a test reaches it."""

    container: str
    dsn: str

    def execute(self, sql: str) -> str:
        """Run one statement as the superuser and return its output."""
        done = subprocess.run(
            [
                "docker",
                "exec",
                self.container,
                "psql",
                "-U",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
                "-tA",
                "-c",
                sql,
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        return done.stdout.strip()


@pytest.fixture(scope="session")
def pg_source() -> Iterator[PgSource]:
    name = f"medulla-fed-pg-{uuid.uuid4().hex[:8]}"
    # The source is a real PostgreSQL container: a host with no reachable
    # container runtime cannot host it, so the test reports a skip with the
    # reason instead of an error. Hosts that have docker (hosted CI) run it.
    if shutil.which("docker") is None:
        pytest.skip("docker is not installed: no PostgreSQL source container")
    info = subprocess.run(["docker", "info"], capture_output=True)
    if info.returncode != 0:
        pytest.skip("the docker daemon is unreachable: no PostgreSQL source container")
    started = subprocess.run(
        [
            "docker",
            "run",
            "-d",
            "--rm",
            "--name",
            name,
            "-e",
            f"POSTGRES_PASSWORD={PG_PASSWORD}",
            "-p",
            f"127.0.0.1:{PG_PORT}:5432",
            "postgres:16-alpine",
        ],
        capture_output=True,
    )
    if started.returncode != 0:
        pytest.skip(
            "the PostgreSQL source container did not start: "
            + started.stderr.decode(errors="replace").strip()[-300:]
        )
    try:
        deadline = time.monotonic() + 60
        while True:
            ready = subprocess.run(
                ["docker", "exec", name, "pg_isready", "-U", "postgres"],
                capture_output=True,
            )
            if ready.returncode == 0:
                probe = subprocess.run(
                    [
                        "docker",
                        "exec",
                        name,
                        "psql",
                        "-U",
                        "postgres",
                        "-c",
                        "SELECT 1",
                    ],
                    capture_output=True,
                )
                if probe.returncode == 0:
                    break
            if time.monotonic() > deadline:
                raise RuntimeError("the PostgreSQL container did not become ready")
            time.sleep(0.5)
        yield PgSource(
            container=name,
            dsn=f"postgres://postgres:{PG_PASSWORD}@127.0.0.1:{PG_PORT}/postgres",
        )
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


WEB_SERVER = "web-stub-mcp"
WEB_TOOL = "web_search"
RECORDED = json.loads(
    (Path(__file__).parent / "web_search_recorded.json").read_text(encoding="utf-8")
)


@dataclass
class WebGateway:
    """A loopback fleet gateway: replays a recorded `web_search` reply, counts calls."""

    calls: list[dict] = field(default_factory=list)

    @property
    def row_calls(self) -> int:
        return len(self.calls)


@pytest.fixture(scope="session")
def web_gateway() -> Iterator[WebGateway]:
    gateway = WebGateway()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            body = json.loads(
                self.rfile.read(int(self.headers.get("Content-Length", "0")))
            )
            known = body.get("server") == WEB_SERVER and body.get("tool") == WEB_TOOL
            op = body["op"]
            if op == "declared_arguments":
                reply: dict[str, Any] = {
                    "arguments": (
                        ["query", "categories", "engines", "language", "pageno"]
                        if known
                        else None
                    )
                }
            elif op == "web_capability":
                reply = {"capability": RECORDED["capability"] if known else None}
            else:
                request = body["request"]
                gateway.calls.append(request)
                reply = {
                    "source_label": request["source_label"],
                    "rows": RECORDED["rows"],
                    "truncated": False,
                    "missing_source": None,
                }
            payload = json.dumps(reply).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *args: object) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", GATEWAY_PORT), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield gateway
    finally:
        server.shutdown()
