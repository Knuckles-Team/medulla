"""Isolated-mode runtime qualification over a private socket (MEDULLA-IDENTITY-R003.3).

Repeats the native TCP cases of R003.1 over a private Unix socket: create,
write, read and restart, then the refusal cases (remote listener, alternate
transport, store reuse, mode mismatch). A second run repeats the boundaries
on the default feature matrix built with AST repository enrichment, named by
`MEDULLA_TEST_AST_BINARY`.
"""

import contextlib
import os
import socket
import stat
import subprocess
import sys
import time

import msgpack
import pytest
from test_native_no_auth_served import binary, clean_env, node_params, ok

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its real isolated process.
    pytest.mark.spec("MEDULLA-IDENTITY-R003.3"),
    pytest.mark.skipif(
        not hasattr(socket, "AF_UNIX") or sys.platform == "win32",
        reason="the runner does not permit Unix sockets",
    ),
]


def ast_binary():
    path = os.environ.get("MEDULLA_TEST_AST_BINARY", "").strip()
    if not path:
        pytest.skip("MEDULLA_TEST_AST_BINARY names the default+ast server build")
    if not os.access(path, os.X_OK):
        pytest.fail("MEDULLA_TEST_AST_BINARY is not an executable server artifact")
    return path


def socket_path(tmp_path):
    # Unix socket paths are length bounded; keep a short private directory.
    private = tmp_path / "s"
    private.mkdir(mode=0o700, parents=True, exist_ok=True)
    return str(private / "eg.sock")


@contextlib.contextmanager
def uds_server(tmp_path, *, server_binary=None, extra=(), extra_env=None):
    tmp_path.mkdir(parents=True, exist_ok=True)
    path = socket_path(tmp_path)
    store = str(tmp_path / "data")
    with (tmp_path / "server.log").open("ab") as log:
        proc = subprocess.Popen(
            [
                server_binary or binary(),
                "--auth-mode",
                "none",
                "--persist-dir",
                store,
                *extra,
            ],
            env=clean_env() | {"GRAPH_SERVICE_SOCKET": path} | (extra_env or {}),
            stdout=log,
            stderr=log,
        )
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    pytest.fail((tmp_path / "server.log").read_text())
                if os.path.exists(path):
                    with socket.socket(socket.AF_UNIX) as probe:
                        try:
                            probe.connect(path)
                            break
                        except OSError:
                            pass
                time.sleep(0.05)
            else:
                pytest.fail("native engine did not become ready on the private socket")
            yield path
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=20)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def rpc(path, method, params=None, *, key=None, token=None, actor=None):
    carrier = (
        token
        if token is not None
        else ("" if key is None else "anon1." + anon_key(key))
    )
    request = {
        "id": 1,
        "graph": "__commons__",
        "auth_token": carrier,
        "method": method,
        "agent_id": actor,
    }
    if params is not None:
        request["params"] = params
    payload = msgpack.packb(request, use_bin_type=True)
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(20)
        connection.connect(path)
        connection.sendall(len(payload).to_bytes(4, "big") + payload)

        def exact(n):
            data = b""
            while len(data) < n:
                chunk = connection.recv(n - len(data))
                assert chunk, "native engine closed connection"
                data += chunk
            return data

        return msgpack.unpackb(exact(int.from_bytes(exact(4), "big")), raw=False)


def anon_key(key):
    import json

    return json.dumps({"idempotency_key": key}, separators=(",", ":"))


def assert_private_socket(path):
    mode = stat.S_IMODE(os.stat(path).st_mode)
    assert mode == 0o600, oct(mode)


def assert_origin_store_and_loopback(tmp_path, server_binary=None):
    with uds_server(tmp_path, server_binary=server_binary) as path:
        assert_private_socket(path)
        ok(rpc(path, "AddNode", node_params(), key="uds-write"))
        assert ok(rpc(path, "HasNode", {"node_id": "anonymous-proof"})) is True
        ok(rpc(path, "AddNode", node_params(), key="uds-write"))
        assert rpc(path, "AddNode", node_params(2), key="uds-write").get("error")
        # A claimed or signed identity is refused, never adopted.
        for token, actor in [("eg2.not-a-credential", None), ("", "claimed-admin")]:
            refused = rpc(path, "Ping", token=token, actor=actor)
            assert refused.get("error"), refused
            assert "ACCESS_DENIED" in str(refused.get("error")) + str(
                refused.get("error_detail")
            ), refused
    with uds_server(tmp_path, server_binary=server_binary) as path:
        assert_private_socket(path)
        assert ok(rpc(path, "HasNode", {"node_id": "anonymous-proof"})) is True
        assert rpc(path, "AddNode", node_params(2), key="uds-write").get("error")


def assert_refusals(tmp_path, server_binary=None):
    cases = [
        ("remote-listener", ["--tcp-addr", "0.0.0.0:9100"], {}),
        ("alternate-transport", [], {"MEDULLA_PGWIRE_ADDR": "127.0.0.1:5432"}),
        ("mode-mismatch", ["--socket-mode", "0660"], {}),
    ]
    for name, extra, env in cases:
        case = tmp_path / name
        case.mkdir(parents=True)
        result = subprocess.run(
            [
                server_binary or binary(),
                "--auth-mode",
                "none",
                "--persist-dir",
                str(case / "data"),
                *extra,
            ],
            env=clean_env() | {"GRAPH_SERVICE_SOCKET": socket_path(case)} | env,
            capture_output=True,
            timeout=20,
        )
        assert result.returncode != 0, name
        assert not (case / "data" / "anonymous-store-v1").exists(), name
    reuse = tmp_path / "store-reuse"
    store = reuse / "data"
    store.mkdir(parents=True)
    (store / "protected-state").write_text("unchanged")
    result = subprocess.run(
        [server_binary or binary(), "--auth-mode", "none", "--persist-dir", str(store)],
        env=clean_env() | {"GRAPH_SERVICE_SOCKET": socket_path(reuse)},
        capture_output=True,
        timeout=20,
    )
    assert result.returncode != 0
    assert (store / "protected-state").read_text() == "unchanged"


def test_anonymous_mode_over_private_socket(tmp_path):
    assert_origin_store_and_loopback(tmp_path / "served")
    assert_refusals(tmp_path / "refused")


def test_anonymous_mode_with_ast_enrichment(tmp_path):
    server_binary = ast_binary()
    assert_origin_store_and_loopback(tmp_path / "served", server_binary)
    assert_refusals(tmp_path / "refused", server_binary)
