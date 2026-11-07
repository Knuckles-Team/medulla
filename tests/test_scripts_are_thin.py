"""Proofs for MEDULLA-SDLC-R022: gate logic leaves scripts/ and callers follow."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
CALLERS = [ROOT / ".config" / "pre-commit.yaml", ROOT / ".mergequeue.yaml"] + sorted(
    (ROOT / ".github").rglob("*.yml")
)
SCRIPT_REF = re.compile(r"(?<![\w./-])scripts/([A-Za-z0-9_]+\.(?:py|sh))\b")
# Callers that read a frozen checkout of an older release, not this tree.
FROZEN_PREFIX = ".frozen-source/"


@pytest.mark.spec("MEDULLA-SDLC-R022")
def test_no_caller_names_a_script_that_was_moved_out() -> None:
    """Every scripts/<name> a CI workflow or hook runs exists in the tree."""

    missing: list[str] = []
    for caller in CALLERS:
        if not caller.is_file():
            continue
        for line in caller.read_text().splitlines():
            if line.lstrip().startswith("#") or FROZEN_PREFIX in line:
                continue
            for name in SCRIPT_REF.findall(line):
                if not (ROOT / "scripts" / name).is_file():
                    missing.append(f"{caller.relative_to(ROOT)}: scripts/{name}")
    assert not missing, missing
