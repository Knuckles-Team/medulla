"""MEDULLA-SDLC-R020.1: no generated artifact is committed; no freshness gate exists.

Every generated artifact is produced where it is consumed (``build.rs`` into
``OUT_DIR``, the language-clients CI job, or computed inside the test that needs
it). This test fails if a cut artifact is tracked again, if any tracked file
carries a generated-file header, or if a freshness gate comes back.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-SDLC-R020.1")]

# Every artifact the cutover removed from git. None may be tracked again.
CUT_PATHS = (
    ".config/generated-artifacts.toml",
    "scripts/generated_artifacts.py",
    "clients/go/medulla_method_codec.wasm",
    "clients/js/medulla_method_codec.wasm",
    "clients/medulla_method_codec.provenance.json",
    "crates/medulla-core/ontology/decision.ttl",
    "crates/medulla-core/ontology/decision.shapes.ttl",
    "docs/reference/skill-graph.generated.md",
    "docs/operations/owner-store-formats.md",
    "docs/concept_reservations.yaml",
    "tests/fixtures/method_policy_registry_v1.json",
)
# A generated-file header in the first lines of a tracked file.
HEADER = re.compile(
    r"do not (hand-)?edit|@generated|^\W{0,4}generated (by|from)\b",
    re.IGNORECASE | re.MULTILINE,
)
# Third-party sources and lockfiles carry their own upstream headers.
HEADER_EXEMPT = re.compile(r"^(vendor/|.*Cargo\.lock$|\.specify/templates/)")
# Mechanisms that rewrite a committed copy or fail when it drifts.
FRESHNESS = (
    "generated-artifacts.toml",
    "build_method_codec_wasm.py --check",
    "MEDULLA_REGENERATE_",
    "MEDULLA_WRITE_VECTORS",
)


def _tracked() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True
    ).stdout
    return [p for p in out.decode().split("\0") if p]


def test_no_cut_generated_artifact_is_tracked() -> None:
    tracked = set(_tracked())
    assert sorted(tracked & set(CUT_PATHS)) == []
    assert [p for p in tracked if ".generated." in Path(p).name] == []


def test_no_tracked_file_carries_a_generated_header() -> None:
    offenders = []
    for path in _tracked():
        if HEADER_EXEMPT.match(path):
            continue
        try:
            with (ROOT / path).open(encoding="utf-8") as stream:
                head = "".join(stream.readline() for _ in range(5))
        except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
            continue
        if HEADER.search(head):
            offenders.append(path)
    assert offenders == []


def test_no_freshness_gate_remains() -> None:
    gates = [
        ROOT / ".config/pre-commit.yaml",
        *sorted((ROOT / ".github/workflows").glob("*.yml")),
    ]
    gates += [
        ROOT / p for p in _tracked() if p.startswith("crates/") and p.endswith(".rs")
    ]
    offenders = []
    for path in gates:
        text = path.read_text(encoding="utf-8", errors="replace")
        offenders += [
            f"{path.relative_to(ROOT)}: {needle}"
            for needle in FRESHNESS
            if needle in text
        ]
    assert offenders == []
