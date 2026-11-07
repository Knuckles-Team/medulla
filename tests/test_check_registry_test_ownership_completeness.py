"""MEDULLA-CONTRACT-R023.1: every tracked `tests/` file has a recognized owner in
this repository's real `architecture/component-registry.yml`, so the
pre-commit test-ownership hook (part of MEDULLA-CONTRACT-R023's full clean run)
cannot fail on an unclaimed file.

The checker is `cargo xtask registry-test-ownership`; its real-tree case is a
Rust test in tools/xtask. This file keeps the registry-shape case.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine


@pytest.mark.spec("MEDULLA-CONTRACT-R023.1", "MEDULLA-DURABLE-KERNEL-R014.2")
def test_cluster_load_harness_has_exact_binary_composition_owner() -> None:
    document = yaml.safe_load(
        (ROOT / "architecture/component-registry.yml").read_text()
    )
    path = "tests/test_validate_raft_cluster.py"
    owners = [
        component["component_id"]
        for component in document["components"]
        if path in component.get("test_roots", [])
    ]
    assert owners == ["medulla.binary-composition"]
