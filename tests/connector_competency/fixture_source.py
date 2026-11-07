"""Fixture contract source (MEDULLA-CQ-CONN-R067.8).

``FixtureSource`` serves recorded connector responses over loopback HTTP for
the contract kinds ``mcp_tool``, ``a2a_skill``, ``http_endpoint`` and
``graphql_query``. Recordings live in
``recordings/<connector>/<operation>.json`` and hold the real connector tool
output. The server counts every request, answers only the operations named in
the imported contract, and returns ``503`` after ``stop()``.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterable
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

KINDS = ("mcp_tool", "a2a_skill", "http_endpoint", "graphql_query")


class FixtureSource:
    """One loopback server for the recordings of one connector."""

    def __init__(
        self,
        kind: str,
        recordings_dir: Path,
        operations: Iterable[str] | None = None,
    ) -> None:
        if kind not in KINDS:
            raise ValueError(f"unknown contract kind {kind!r}")
        self.kind = kind
        self.recordings_dir = Path(recordings_dir)
        recorded = {p.stem: p for p in sorted(self.recordings_dir.glob("*.json"))}
        named = set(recorded) if operations is None else set(operations)
        self._recordings = {op: recorded[op] for op in named if op in recorded}
        self._lock = threading.Lock()
        self._count = 0
        self._stopped = False
        self._server: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None

    @property
    def request_count(self) -> int:
        with self._lock:
            return self._count

    @property
    def url(self) -> str:
        if self._server is None:
            raise RuntimeError("fixture source is not started")
        port = self._server.server_address[1]
        return f"http://127.0.0.1:{port}"

    def start(self) -> FixtureSource:
        source = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args: object) -> None:
                return

            def _answer(self, body: bytes | None) -> None:
                status, payload = source._respond(self.path, body)
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_GET(self) -> None:
                self._answer(None)

            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length") or 0)
                self._answer(self.rfile.read(length))

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self

    def stop(self) -> None:
        """Answer every later request with ``503``."""
        with self._lock:
            self._stopped = True

    def close(self) -> None:
        """Shut the loopback server down."""
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None

    def _operation(self, path: str, body: bytes | None) -> str | None:
        if self.kind == "http_endpoint":
            return path.strip("/").split("?", 1)[0] or None
        try:
            doc = json.loads(body or b"{}")
        except json.JSONDecodeError:
            return None
        if not isinstance(doc, dict):
            return None
        if self.kind == "mcp_tool":
            params = doc.get("params")
            return params.get("name") if isinstance(params, dict) else None
        if self.kind == "a2a_skill":
            return doc.get("skill")
        return doc.get("operationName")

    def _respond(self, path: str, body: bytes | None) -> tuple[int, bytes]:
        with self._lock:
            self._count += 1
            stopped = self._stopped
        if stopped:
            return 503, b'{"error":"fixture source stopped"}'
        operation = self._operation(path, body)
        recording = self._recordings.get(operation) if operation else None
        if recording is None:
            return 404, json.dumps(
                {"error": f"operation {operation!r} not in the contract"}
            ).encode()
        return 200, recording.read_bytes()
