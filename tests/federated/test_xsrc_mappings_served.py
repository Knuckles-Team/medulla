"""Served reads of the recorded connector responses behind the mapping fixtures.

MEDULLA-CQ-XSRC-R002.7, R003.4, R003.5: a registered operation-bound `Mcp` source
is read by a served `UnifiedQuery` scan; the rows equal the recording of the
connector response the mapping fixture is written over
(`fixtures/xsrc_recorded/`). A loopback fleet gateway replays the recording; the
engine holds no connector client of its own.
"""

from __future__ import annotations

import json
import os
import socket
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

import pytest

RECORDED_DIR = Path(__file__).parent / "fixtures" / "xsrc_recorded"
RECORDINGS = {
    path.stem: json.loads(path.read_text(encoding="utf-8"))
    for path in sorted(RECORDED_DIR.glob("*.json"))
}


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


# The served engine reads the gateway URL and the outbound allow-list from the
# environment of the server process, which the session fixture starts from this
# process's environment, so the loopback endpoint is named here, at collection
# time, unless a sibling module already named it.
if "MEDULLA_CONNECTOR_GATEWAY_URL" not in os.environ:
    _PORT = _free_port()
    os.environ["MEDULLA_CONNECTOR_GATEWAY_URL"] = f"http://127.0.0.1:{_PORT}/gateway"
    os.environ["MEDULLA_FEDERATION_ALLOW"] = ",".join(
        part
        for part in (
            os.environ.get("MEDULLA_FEDERATION_ALLOW", ""),
            f"127.0.0.1:{_PORT}",
        )
        if part
    )


def _recording_for(server: str, tool: str) -> dict | None:
    for recording in RECORDINGS.values():
        if recording["server"] == server and recording["tool"] == tool:
            return recording
    return None


@pytest.fixture(scope="module")
def recorded_gateway() -> Iterator[None]:
    port = urlparse(os.environ["MEDULLA_CONNECTOR_GATEWAY_URL"]).port

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            body = json.loads(
                self.rfile.read(int(self.headers.get("Content-Length", "0")))
            )
            op = body["op"]
            if op == "declared_arguments":
                recording = _recording_for(body.get("server"), body.get("tool"))
                reply: dict = {
                    "arguments": recording["arguments"] if recording else None
                }
            elif op == "web_capability":
                reply = {"capability": None}
            else:
                request = body["request"]
                recording = _recording_for(request["server"], request["tool"])
                reply = {
                    "source_label": request["source_label"],
                    "rows": recording["rows"] if recording else [],
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

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield None
    finally:
        server.shutdown()
        server.server_close()


def _scan(name: str) -> list[dict[str, object]]:
    return [{"ForeignScan": {"source": {"Named": {"name": name}}, "join": False}}]


def _read_recorded(clean_graph, label: str, recording: dict) -> list[dict]:
    clean_graph.query.register_foreign_source(
        label, {"Mcp": {"server": recording["server"], "tool": recording["tool"]}}
    )
    return clean_graph.query.unified(_scan(label))


def _assert_rows_equal_recording(rows: list[dict], recording: dict) -> None:
    by_id = {row["id"]: row for row in rows}
    assert sorted(by_id) == sorted(row["id"] for row in recording["rows"])
    for recorded in recording["rows"]:
        served = json.dumps(by_id[recorded["id"]], sort_keys=True)
        for text in recorded["texts"].values():
            assert text in served, f"{text!r} missing from the served row"


@pytest.mark.spec("MEDULLA-CQ-XSRC-R002.7")
def test_erpnext_mapping_reads_recorded_rows(clean_graph, recorded_gateway):
    recording = RECORDINGS["erpnext_suppliers"]
    rows = _read_recorded(clean_graph, "xsrc-erpnext-suppliers", recording)
    _assert_rows_equal_recording(rows, recording)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.4")
def test_uptime_mapping_reads_recorded_rows(clean_graph, recorded_gateway):
    recording = RECORDINGS["uptime_monitors"]
    rows = _read_recorded(clean_graph, "xsrc-uptime-monitors", recording)
    _assert_rows_equal_recording(rows, recording)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.5")
def test_container_manager_mapping_reads_recorded_rows(clean_graph, recorded_gateway):
    recording = RECORDINGS["container_manager_workloads"]
    rows = _read_recorded(clean_graph, "xsrc-container-manager-workloads", recording)
    _assert_rows_equal_recording(rows, recording)
