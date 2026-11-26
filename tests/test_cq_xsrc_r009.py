"""High-confidence agent claims against a new external report (MEDULLA-CQ-XSRC-R009).

The fixture `tests/fixtures/cq/xsrc_r009.uql` is one UQL program with the typed
params `$agent_id` and `$report_id`. Its literals (confidence 0.8, a 30 day valid
window, three clusters) are query text, not knobs. Stage one selects the claims of
one agent that touch the report; stage two clusters the evidence of those claims.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

FIXTURE = Path(__file__).parent / "fixtures" / "cq" / "xsrc_r009.uql"


def _program(mode: str = "") -> str:
    text = FIXTURE.read_text(encoding="utf-8").strip()
    # The pragma stays first; EXPLAIN or PROFILE follows it.
    return text.replace("UQL 1;", f"UQL 1; {mode}".rstrip(), 1)


PARAMS: dict[str, Any] = {"agent_id": "agent:cq-r009", "report_id": "report:cq-r009"}


@pytest.mark.spec("MEDULLA-CQ-XSRC-R009.1")
def test_fixture_compiles_to_one_plan(clean_graph) -> None:
    """Verifies MEDULLA-CQ-XSRC-R009.1: the fixture compiles to one plan DAG with one
    root, and its literals are program text."""
    text = FIXTURE.read_text(encoding="utf-8")
    for literal in ("0.8", "30 d", "K 3", "$agent_id", "$report_id"):
        assert literal in text, literal
    explained = clean_graph.query.uql(_program("EXPLAIN"), PARAMS)
    assert explained["kind"] == "explain", explained
    assert explained["plan_root_count"] == 1, explained
    assert explained["plan_node_count"] == len(explained["stages"]), explained
    assert explained["plan_id_count"] == 1, explained
