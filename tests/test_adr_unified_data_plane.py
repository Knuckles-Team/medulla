"""MEDULLA-UNIFIED-DATA-PLANE-R001: the unified data-plane ADR record.

The ADR (docs/architecture/unified_data_plane_adr.md) is the only artifact of
the row: it records five accepted decisions (attached-source default, one
converged engine, native hosting earned through the R026 gate, no synced
second store, pgrx no-go), so the test pins the record's shape — five `##
Decision` headings, the Accepted status, the no-synced-store decision and
the no-go wording — rather than re-litigating the decisions.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
ADR = ROOT / "docs" / "architecture" / "unified_data_plane_adr.md"


# spec: MEDULLA-UNIFIED-DATA-PLANE-R001
def test_adr_records_five_accepted_decisions() -> None:
    assert ADR.is_file(), "the unified data-plane ADR is the row's only artifact"
    text = ADR.read_text(encoding="utf-8")

    decisions = re.findall(r"^## Decision\b", text, flags=re.MULTILINE)
    assert len(decisions) == 5, "the ADR holds exactly five accepted decisions"

    status = next(line for line in text.splitlines() if line.startswith("**Status:**"))
    assert "Accepted" in status, f"the ADR must stand accepted: {status}"

    # The five recorded decisions, in the ADR's own headings.
    headings = re.findall(r"^## Decision \d+ — (.+)$", text, flags=re.MULTILINE)
    assert len(headings) == 5
    joined = " | ".join(headings).casefold()
    assert "attached sources" in joined and "default" in joined, joined
    assert "converged" in joined and "engine" in joined, joined
    assert "native hosting" in joined, joined

    # No synced second store: the single-engine decision rules out a parallel
    # storage authority, and only the change-captured accelerated copy is kept.
    decision_2 = text.split("## Decision 2")[1].split("## Decision 3")[0]
    assert "parallel storage authority" in decision_2, decision_2
    assert "accelerated copy" in decision_2, decision_2

    # Native hosting is earned per application through the R026 gate.
    decision_3 = text.split("## Decision 3")[1].split("## Decision 4")[0]
    assert "MEDULLA-UNIFIED-DATA-PLANE-R026" in decision_3, decision_3

    # No synced second store: the only copy of attached data allowed is the
    # R038 watermark-bound accelerated cache.
    decision_4 = text.split("## Decision 4")[1].split("## Decision 5")[0]
    assert "no synced second store" in decision_4.casefold(), decision_4
    assert "watermark" in decision_4.casefold(), decision_4
    assert "MEDULLA-UNIFIED-DATA-PLANE-R038" in decision_4, decision_4

    # The pgrx companion is a no-go here: its go/no-go is decided separately.
    decision_5 = text.split("## Decision 5")[1]
    assert "go/no-go" in decision_5, decision_5
    assert "MEDULLA-UNIFIED-DATA-PLANE-R025" in decision_5, decision_5
