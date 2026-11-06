"""MEDULLA-UNIFIED-DATA-PLANE-R038.3 served acceptance: one snapshot per source.

A query with two fragments of one source needs a single snapshot. A source that
cannot give one (a plain HTTP/JSON source) refuses the query with
``SNAPSHOT_UNAVAILABLE`` before any request reaches the source.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError


def scan(name: str) -> dict[str, object]:
    return {"ForeignScan": {"source": {"Named": {"name": name}}, "join": False}}


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.3.4")
def test_snapshot_refusals(clean_graph):
    clean_graph.query.register_foreign_source(
        "no-snapshot-source",
        {
            "HttpJson": {
                "url": "https://source.example.test/rows",
                "json_path": "",
                "field_map": {"id": "id"},
            }
        },
    )
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.unified(
            [scan("no-snapshot-source"), scan("no-snapshot-source")]
        )
    assert "SNAPSHOT_UNAVAILABLE" in str(refused.value)
