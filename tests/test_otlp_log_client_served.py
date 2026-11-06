"""Typed OTLP log ingest against a served engine with ``--obs-addr``.

Covers MEDULLA-CONTRACT-R050. Starts a dedicated ``medulla-server`` from the
prebuilt binary (``MEDULLA_TEST_BINARY``), posts one valid batch through the
client's ``ingest_otlp_logs`` sender, proves a malformed or oversize batch
raises ``ValidationError`` before sending, and posts a 4 MiB plus one byte
body directly to expect ``LOG_BATCH_TOO_LARGE``.
"""

from __future__ import annotations

import asyncio
import http.client
import json
import os
import socket
import subprocess
import time

import pytest
from conftest import _prebuilt_test_binary
from pydantic import ValidationError

from medulla.otlp_logs import (
    MAX_BATCH_BYTES,
    MAX_BATCH_RECORDS,
    OtlpLogBatch,
    ingest_otlp_logs,
)

pytestmark = pytest.mark.no_engine

STARTUP_TIMEOUT_S = 60


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _batch(count: int, body: str = "hello") -> dict:
    return {
        "resourceLogs": [
            {
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": "otlp-client"}}
                    ]
                },
                "scopeLogs": [
                    {
                        "logRecords": [
                            {
                                "timeUnixNano": str(1_700_000_000_000_000_000 + i),
                                "severityText": "INFO",
                                "body": {"stringValue": f"{body} {i}"},
                            }
                            for i in range(count)
                        ]
                    }
                ],
            }
        ]
    }


@pytest.fixture(scope="module")
def obs_addr(tmp_path_factory):
    binary = _prebuilt_test_binary()
    if binary is None:
        pytest.skip("requires a prebuilt MEDULLA_TEST_BINARY with the obs feature")
    persist = tmp_path_factory.mktemp("otlp-log-persist")
    sock = str(tmp_path_factory.mktemp("otlp-log-sock") / "engine.sock")
    state = str(tmp_path_factory.mktemp("otlp-log-security"))
    port = _free_port()
    env = {
        **os.environ,
        "GRAPH_SERVICE_AUTH_SECRET": "test-otlp-log-secret",  # sanitizer:ignore
        "MEDULLA_REQUIRE_OIDC": "false",
        "MEDULLA_AUDIENCE": "medulla-test",
        "MEDULLA_TENANT": "tenant:test",
        "MEDULLA_POLICY_VERSION": "policy:test",
        "MEDULLA_SECURITY_STATE_DIR": state,
        "MEDULLA_SIGNER_KEYS_JSON": json.dumps({"service:test-suite": "test-key"}),
        "GRAPH_SERVICE_PERSIST_DIR": str(persist),
        "MEDULLA_REDB_SHARDS": "1",
    }
    proc = subprocess.Popen(
        [str(binary), "--socket-path", sock, "--obs-addr", f"127.0.0.1:{port}"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        deadline = time.monotonic() + STARTUP_TIMEOUT_S
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                out, err = proc.communicate()
                pytest.fail(f"medulla-server exited early: {out}\n{err}")
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
                probe.settimeout(0.5)
                if probe.connect_ex(("127.0.0.1", port)) == 0:
                    break
            time.sleep(0.5)
        else:
            pytest.fail(f"--obs-addr 127.0.0.1:{port} never accepted a connection")
        yield f"127.0.0.1:{port}"
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


def _post_raw(addr: str, body: bytes) -> tuple[int, str]:
    host, _, port = addr.rpartition(":")
    conn = http.client.HTTPConnection(host, int(port), timeout=30)
    try:
        conn.request(
            "POST", "/v1/logs", body=body, headers={"content-type": "application/json"}
        )
        response = conn.getresponse()
        return response.status, response.read().decode("utf-8", errors="replace")
    finally:
        conn.close()


@pytest.mark.spec("MEDULLA-CONTRACT-R050")
def test_ingest_otlp_logs_and_reject_malformed(obs_addr):
    result = asyncio.run(
        ingest_otlp_logs(OtlpLogBatch.model_validate(_batch(3)), obs_addr=obs_addr)
    )
    assert result == {"partialSuccess": {}}

    # Malformed and oversize batches raise before anything is sent.
    with pytest.raises(ValidationError):
        OtlpLogBatch.model_validate({"resourceLogs": "not-a-list"})
    with pytest.raises(ValidationError):
        OtlpLogBatch.model_validate({"resourceLogs": [], "unexpected": 1})
    with pytest.raises(ValidationError):
        OtlpLogBatch.model_validate(_batch(MAX_BATCH_RECORDS + 1))
    with pytest.raises(ValidationError):
        asyncio.run(ingest_otlp_logs(_batch(MAX_BATCH_RECORDS + 1), obs_addr=obs_addr))

    # The engine refuses a body above 4 MiB with the stable code.
    oversize = json.dumps(_batch(1, body="x" * MAX_BATCH_BYTES)).encode("utf-8")
    assert len(oversize) > MAX_BATCH_BYTES
    status, text = _post_raw(obs_addr, oversize)
    assert status == 413
    assert text.startswith("LOG_BATCH_TOO_LARGE")
