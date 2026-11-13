"""A served engine wired to a scripted chat endpoint, for the expansion port tests
(MEDULLA-FEDERATED-QUERY-R651.3, R651.7, R090.3 to R090.5).

The stub counts every chat call it receives, so a test can assert how many times the
one expansion port was asked.
"""

from __future__ import annotations

import json
import os
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    bootstrap_context,
    find_server_binary,
    request_context,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import SyncMedullaClient

RUST_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SECRET = "test-expansion-secret"  # sanitizer:ignore


class ChatStub:
    """A chat-completions endpoint that answers a scripted list and counts calls."""

    def __init__(self, answer: list[str] | None, status: int = 200) -> None:
        self.answer = answer
        self.status = status
        self.calls = 0
        stub = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length", "0"))
                self.rfile.read(length)
                stub.calls += 1
                body = json.dumps(
                    {
                        "choices": [
                            {"message": {"content": json.dumps(stub.answer or [])}}
                        ]
                    }
                ).encode()
                self.send_response(stub.status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args: Any) -> None:
                return

        self._server = HTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def endpoint(self) -> str:
        return f"http://127.0.0.1:{self._server.server_address[1]}/v1/chat/completions"

    def close(self) -> None:
        self._server.shutdown()


@contextmanager
def served_engine(tmp_path_factory: Any, stub: ChatStub | None) -> Iterator[str]:
    """Run an engine; with a `stub` its expansion port asks that endpoint, without one
    the port is unconfigured. Yields the socket path."""
    runtime = tmp_path_factory.mktemp("expansion")
    sock = str(runtime / "engine.sock")
    persist_dir = str(runtime / "persist")
    os.makedirs(persist_dir, exist_ok=True)
    log_path = runtime / "server.log"
    env = {
        **os.environ,
        **strict_server_env(
            str(runtime / "security"), auth_secret=SECRET, persist_dir=persist_dir
        ),
        "MEDULLA_UQL_TEXT_EMBEDDER": "hash",
        "MEDULLA_ENCRYPTION_KEY": "test-medulla-encryption-key",
    }
    env.pop("MEDULLA_NL_ENDPOINT", None)
    if stub is not None:
        env["MEDULLA_NL_ENDPOINT"] = stub.endpoint
        env["MEDULLA_NL_MODEL"] = "stub"
    binary = find_server_binary() or os.path.join(
        RUST_DIR, "target", "debug", "medulla-server"
    )
    proc = spawn_server([binary, "--socket-path", sock], env=env, log_path=log_path)
    wait_for_server(proc, sock, name="expansion server", log_path=log_path)
    bootstrap = SyncMedullaClient.connect(
        socket_path=sock, auth_secret=SECRET, verified_context=bootstrap_context()
    )
    try:
        bootstrap.consensus.bootstrap_system_identity(
            agent_id=TEST_AGENT_ID, signer_id=TEST_AGENT_ID, signer_key=TEST_SIGNER_KEY
        )
    finally:
        bootstrap.close()
    try:
        yield sock
    finally:
        stop_server(proc, name="expansion server", log_path=log_path)


def connect(sock: str) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=sock, auth_secret=SECRET, verified_context=request_context()
    )
