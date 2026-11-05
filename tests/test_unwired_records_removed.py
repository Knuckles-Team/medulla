"""MEDULLA-DURABLE-KERNEL-R868: unwired topic-log and agent-peer records are gone."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R868")]


def test_topic_log_and_agent_peer_records_are_deleted() -> None:
    assert not (ROOT / "crates/medulla-durable/src/topic.rs").exists()
    assert not (ROOT / "crates/medulla-agent-types/src/agent_peer.rs").exists()
    assert (ROOT / "crates/medulla-durable/src/agent_bus.rs").exists()
    hits = subprocess.run(
        ["git", "grep", "-n", r"TopicEntry\|AgentPeer", "--", "src", "crates"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert hits.returncode == 1, hits.stdout
