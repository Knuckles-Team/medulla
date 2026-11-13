"""MEDULLA-DECISION-ENGINE-R170.3 served receipt: team records in the graph.

A team, its members and its tasks are written through the generated client
write (``ApplyChangeEnvelopes``; ``changes.apply_batch`` compiles to it), the
same entry point the contrast and concentration tests use. The members and the
task statuses are read back by graph query. The shape check of ``a2a-v1`` is
reached through the served ``ShaclValidate``: a ``TeamMember`` without
``boundToTeam`` is refused, a bound one conforms.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import msgpack
import pytest
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op

TEAM = "team:ops"
MEMBERS = ["member:alpha", "member:beta"]
TASKS = {"task:one": "working", "task:two": "completed", "task:three": "working"}
SHAPES = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "medulla-core"
    / "ontology"
    / "a2a-v1.shapes.ttl"
)
PREFIXES = (
    "@prefix eg: <https://knuckles-team.github.io/ontology/core#> .\n"
    "@prefix ex: <http://example.org/team/> .\n"
)


def _write_team(clean_graph: Any) -> None:
    nodes: list[tuple[str, dict[str, Any]]] = [(TEAM, {"type": "Team", "_team": True})]
    nodes.extend(
        (m, {"type": "TeamMember", "_team_member": True, "boundToTeam": TEAM})
        for m in MEMBERS
    )
    nodes.extend(
        (t, {"type": "TeamTask", "boundToTeam": TEAM, "taskStatus": status})
        for t, status in TASKS.items()
    )
    ops = [
        op(
            ordinal,
            "AddNode",
            {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        )
        for ordinal, (node_id, props) in enumerate(nodes)
    ]
    envelope = _envelope(_graph_name(clean_graph), "r170-3:team", "r170-3:team", 1, ops)
    results = clean_graph.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results


def _ids(clean_graph: Any, text: str, params: dict[str, Any]) -> list[str]:
    result = clean_graph.query.uql(text, params)
    assert result["kind"] == "rows", result
    return sorted(row["id"] for row in result["rows"])


# spec: MEDULLA-DECISION-ENGINE-R170.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R170.3")
def test_team_nodes_validate_and_query(clean_graph):
    _write_team(clean_graph)

    members = _ids(
        clean_graph, "MATCH (:TeamMember) WHERE boundToTeam = $team", {"team": TEAM}
    )
    assert members == sorted(MEMBERS)

    for status in sorted(set(TASKS.values())):
        expected = sorted(t for t, s in TASKS.items() if s == status)
        got = _ids(
            clean_graph,
            "MATCH (:TeamTask) WHERE taskStatus = $status",
            {"status": status},
        )
        assert got == expected, status

    shapes = SHAPES.read_text()
    bound = clean_graph.rdf.validate_shacl(
        shapes, PREFIXES + "ex:member a eg:TeamMember ; eg:boundToTeam ex:ops ."
    )
    assert bound["conforms"] is True, bound

    unbound = clean_graph.rdf.validate_shacl(
        shapes, PREFIXES + "ex:member a eg:TeamMember ."
    )
    assert unbound["conforms"] is False, unbound
    assert unbound["results"], "the refusal must name the violated constraint"
