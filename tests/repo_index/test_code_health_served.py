"""Served code-health sweep: MEDULLA-REPO-INGEST-R700.3 and R700.6.

`CodeHealthSweep` reads the same code index `CodeContext` reads and reports the symbols
the caller's declared entry points do not reach. With no declared entry point every
finding is `unknown`, never `unreachable`.
"""

from __future__ import annotations

from typing import Any

import pytest
from test_repository_enrich_served import _index

_FIXTURE = [
    (
        "app/main.py",
        b"def a():\n    return 1\n\ndef main():\n    return a()\n\ndef b():\n    return 2\n",
    ),
]


def _symbol_names(client: Any) -> dict[str, str]:
    """Short name to the symbol name the index holds, from the committed symbol rows."""
    names: dict[str, str] = {}
    for node_id, _ in client.nodes.list_by_label("SYMBOL", 0):
        props = client.nodes.properties(node_id) or {}
        names[props.get("name", "")] = props.get("qualified_name") or node_id
    return names


def _sweep(client: Any, entry_points: list[str]) -> dict[str, Any]:
    return client._send(
        "CodeHealthSweep",
        {"request": {"repo": "repo-health", "entry_points": entry_points}},
    )


@pytest.mark.spec("MEDULLA-REPO-INGEST-R700.3")
def test_declared_entry_points_report_unreachable_symbols(clean_graph) -> None:
    _index(clean_graph, "repo-health", _FIXTURE)
    names = _symbol_names(clean_graph)
    assert {"main", "a", "b"} <= set(names), names

    report = _sweep(clean_graph, [names["main"]])
    assert report["repo"] == "repo-health"
    unreachable = [
        finding for finding in report["findings"] if finding["kind"] == "unreachable"
    ]
    assert [finding["symbol"] for finding in unreachable] == [names["b"]]
    assert names["main"] in unreachable[0]["evidence"]["entry_points_tried"]


@pytest.mark.spec("MEDULLA-REPO-INGEST-R700.6")
def test_no_entry_points_reports_unknown(clean_graph) -> None:
    _index(clean_graph, "repo-health", _FIXTURE)
    report = _sweep(clean_graph, [])
    assert report["findings"], "the index holds symbols"
    assert {finding["kind"] for finding in report["findings"]} == {"unknown"}
