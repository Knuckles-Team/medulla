"""Typed OTLP log ingest against a served engine with ``--obs-addr``.

Covers MEDULLA-CONTRACT-R050. Starts a dedicated ``medulla-server`` from the
prebuilt binary (``MEDULLA_TEST_BINARY``), posts one valid batch through the
client's ``ingest_otlp_logs`` sender, proves a malformed or oversize batch
raises ``ValidationError`` before sending, and posts a 4 MiB plus one byte
body directly to expect ``LOG_BATCH_TOO_LARGE``.

The listener admits ingest only from a verified telemetry writer, so the
module serves a loopback JWKS and mints an RS256 writer token (scope
``telemetry:write``, a tenant claim) with a key generated here; a post
without the token is refused with ``ACCESS_DENIED``.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import http.client
import http.server
import json
import os
import secrets
import socket
import subprocess
import threading
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
ISSUER = "https://identity.example.test/realms/otlp"
AUDIENCE = "medulla-obs"
KEY_ID = "otlp-writer"
# DER prefix of a PKCS#1 v1.5 SHA-256 DigestInfo (RFC 8017 section 9.2).
SHA256_DIGEST_INFO = bytes.fromhex("3031300d060960864801650304020105000420")


def _probable_prime(bits: int) -> int:
    while True:
        candidate = secrets.randbits(bits) | (1 << (bits - 1)) | (1 << (bits - 2)) | 1
        if all(candidate % p for p in (3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37)):
            d, r = candidate - 1, 0
            while d % 2 == 0:
                d, r = d // 2, r + 1
            for _ in range(32):
                x = pow(secrets.randbelow(candidate - 3) + 2, d, candidate)
                if x in (1, candidate - 1):
                    continue
                for _ in range(r - 1):
                    x = pow(x, 2, candidate)
                    if x == candidate - 1:
                        break
                else:
                    break
            else:
                return candidate


def _rsa_key() -> tuple[int, int, int]:
    """A 2048-bit RSA key ``(n, e, d)`` for the test writer."""
    e = 65537
    while True:
        p, q = _probable_prime(1024), _probable_prime(1024)
        phi = (p - 1) * (q - 1)
        if p != q and phi % e:
            return p * q, e, pow(e, -1, phi)


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def _int_bytes(value: int) -> bytes:
    return value.to_bytes((value.bit_length() + 7) // 8, "big")


def _rs256_token(key: tuple[int, int, int], claims: dict) -> str:
    n, _, d = key
    header = {"alg": "RS256", "typ": "JWT", "kid": KEY_ID}
    signing_input = (
        _b64url(json.dumps(header).encode())
        + "."
        + _b64url(json.dumps(claims).encode())
    )
    digest_info = SHA256_DIGEST_INFO + hashlib.sha256(signing_input.encode()).digest()
    size = (n.bit_length() + 7) // 8
    padded = (
        b"\x00\x01" + b"\xff" * (size - len(digest_info) - 3) + b"\x00" + digest_info
    )
    signature = pow(int.from_bytes(padded, "big"), d, n).to_bytes(size, "big")
    return signing_input + "." + _b64url(signature)


def _writer_token(key: tuple[int, int, int]) -> str:
    return _rs256_token(
        key,
        {
            "iss": ISSUER,
            "aud": AUDIENCE,
            "sub": "service:otlp-test-collector",
            "exp": int(time.time()) + 3600,
            "scope": "telemetry:write",
            "tenant": "tenant:test",
        },
    )


def _serve_jwks(key: tuple[int, int, int]) -> tuple[http.server.HTTPServer, str]:
    n, e, _ = key
    body = json.dumps(
        {
            "keys": [
                {
                    "kty": "RSA",
                    "kid": KEY_ID,
                    "use": "sig",
                    "alg": "RS256",
                    "n": _b64url(_int_bytes(n)),
                    "e": _b64url(_int_bytes(e)),
                }
            ]
        }
    ).encode()

    class Jwks(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args) -> None:
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Jwks)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}/jwks"


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
def writer_key():
    return _rsa_key()


@pytest.fixture(scope="module")
def obs_addr(tmp_path_factory, writer_key):
    binary = _prebuilt_test_binary()
    if binary is None:
        pytest.skip("requires a prebuilt MEDULLA_TEST_BINARY with the obs feature")
    jwks_server, jwks_url = _serve_jwks(writer_key)
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
        "MEDULLA_OBS_JWT_ISSUER": ISSUER,
        "MEDULLA_OBS_JWT_AUDIENCE": AUDIENCE,
        "MEDULLA_OBS_JWKS_URL": jwks_url,
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
        jwks_server.shutdown()


def _post_raw(addr: str, body: bytes, token: str) -> tuple[int, str]:
    host, _, port = addr.rpartition(":")
    conn = http.client.HTTPConnection(host, int(port), timeout=30)
    try:
        conn.request(
            "POST",
            "/v1/logs",
            body=body,
            headers={
                "content-type": "application/json",
                "authorization": f"Bearer {token}",
            },
        )
        response = conn.getresponse()
        return response.status, response.read().decode("utf-8", errors="replace")
    finally:
        conn.close()


@pytest.mark.spec("MEDULLA-CONTRACT-R050")
def test_ingest_otlp_logs_and_reject_malformed(obs_addr, writer_key):
    token = _writer_token(writer_key)
    result = asyncio.run(
        ingest_otlp_logs(
            OtlpLogBatch.model_validate(_batch(3)),
            obs_addr=obs_addr,
            bearer_token=token,
        )
    )
    assert result == {"partialSuccess": {}}

    # Without a verified writer the listener refuses the ingest.
    with pytest.raises(RuntimeError, match="ACCESS_DENIED"):
        asyncio.run(ingest_otlp_logs(_batch(3), obs_addr=obs_addr))

    # Malformed and oversize batches raise before anything is sent.
    with pytest.raises(ValidationError):
        OtlpLogBatch.model_validate({"resourceLogs": "not-a-list"})
    with pytest.raises(ValidationError):
        OtlpLogBatch.model_validate({"resourceLogs": [], "unexpected": 1})
    with pytest.raises(ValidationError):
        OtlpLogBatch.model_validate(_batch(MAX_BATCH_RECORDS + 1))
    with pytest.raises(ValidationError):
        asyncio.run(
            ingest_otlp_logs(
                _batch(MAX_BATCH_RECORDS + 1), obs_addr=obs_addr, bearer_token=token
            )
        )

    # The engine refuses a body above 4 MiB with the stable code.
    oversize = json.dumps(_batch(1, body="x" * MAX_BATCH_BYTES)).encode("utf-8")
    assert len(oversize) > MAX_BATCH_BYTES
    status, text = _post_raw(obs_addr, oversize, token)
    assert status == 413
    assert text.startswith("LOG_BATCH_TOO_LARGE")
