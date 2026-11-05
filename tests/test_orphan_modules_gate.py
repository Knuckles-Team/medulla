"""The unwired topic-log and agent-peer record code is gone and every tracked
Rust file stays reachable from a cargo target."""

from __future__ import annotations

import pathlib
import subprocess
import sys

import pytest

# Pure/static test: runs a repository script and git, never the shared engine.
pytestmark = pytest.mark.no_engine

_ROOT = pathlib.Path(__file__).resolve().parents[1]


# spec: MEDULLA-DURABLE-KERNEL-R868
def test_topic_log_and_agent_peer_are_gone() -> None:
    gate = subprocess.run(
        [sys.executable, str(_ROOT / "scripts" / "check_orphan_modules.py")],
        cwd=_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert gate.returncode == 0, gate.stdout + gate.stderr
    for path in (
        "crates/medulla-durable/src/topic.rs",
        "crates/medulla-agent-types/src/agent_peer.rs",
    ):
        assert not (_ROOT / path).exists(), path
    grep = subprocess.run(
        ["git", "grep", "-n", r"TopicEntry\|AgentPeer", "--", "src", "crates"],
        cwd=_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert grep.returncode == 1, grep.stdout
