"""An out-of-range engine knob refuses startup before any listener binds."""

import os
import subprocess

import pytest
from conftest import find_server_binary, server_timeout, strict_server_env

RUST_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
SERVER_BIN = find_server_binary() or os.path.join(
    RUST_DIR, "target", "debug", "medulla-server"
)

pytestmark = pytest.mark.no_engine  # Each case spawns its own engine.


def _run(tmp_path, *extra_args, **env_overrides):
    socket_path = str(tmp_path / "knob.sock")
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith("MEDULLA_TOPOLOGY_") and k != "GRAPH_SERVICE_SOCKET"
    }
    env.update(
        strict_server_env(
            str(tmp_path / "security"),
            auth_secret="test-knob-secret",
            persist_dir=str(tmp_path / "persist"),
        )
    )
    env.update(env_overrides)
    proc = subprocess.run(
        [SERVER_BIN, "--socket-path", socket_path, *extra_args],
        env=env,
        capture_output=True,
        timeout=server_timeout(),
        check=False,
    )
    return proc, socket_path


@pytest.mark.spec("MEDULLA-CONTRACT-R058")
def test_out_of_range_knob_refuses_startup(tmp_path):
    proc, socket_path = _run(tmp_path, MEDULLA_TOPOLOGY_MAX_WIDTH="17")
    assert proc.returncode != 0
    assert "KNOB_OUT_OF_RANGE: knob topology.max_width" in proc.stderr.decode()
    assert not os.path.exists(socket_path), "refused before the listener bound"

    proc, socket_path = _run(tmp_path, "--knob", "no.such_knob=1")
    assert proc.returncode != 0
    assert "KNOB_UNKNOWN: knob no.such_knob" in proc.stderr.decode()
    assert not os.path.exists(socket_path)

    proc, socket_path = _run(tmp_path, "--knob", "topology.max_width=wide")
    assert proc.returncode != 0
    assert "KNOB_INVALID: knob topology.max_width" in proc.stderr.decode()
    assert not os.path.exists(socket_path)
