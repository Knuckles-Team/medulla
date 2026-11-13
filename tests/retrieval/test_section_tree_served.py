"""Served vector-free document-section traversal (MEDULLA-FEDERATED-QUERY-R081.9.7.6).

A round with a `hierarchy` part of kind `document_section` walks stored
`SectionNode` rows breadth first and returns the records of the requested
granularity with their root-to-record paths. It makes no embedding, reranker
or vector call.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _round_served import client, put, request, run_round

_REVISION = "doc-rev-1"
_NO_ROWS = [{"Scan": {"label": "NoSuchLabel"}}]


def _section(
    cl: Any,
    graph: str,
    node_id: str,
    parent: str | None,
    children: list[str],
    granularity: str,
    span: tuple[int, int],
    revision: str = _REVISION,
) -> None:
    put(
        cl,
        graph,
        node_id,
        {
            "type": "SectionNode",
            "document_id": "doc-1",
            "document_revision": revision,
            "parent_id": parent,
            "children": children,
            "granularity": granularity,
            "source_start": span[0],
            "source_end": span[1],
            "summary": f"summary of {node_id}",
            "source_digest": f"src-{node_id}",
            "summary_digest": f"sum-{node_id}",
        },
    )


def _seed(cl: Any, graph: str) -> None:
    rows = [
        ("sec-doc", None, ["sec-a", "sec-b"], "document", (0, 100)),
        ("sec-a", "sec-doc", ["span-a1", "span-a2"], "section", (0, 60)),
        ("sec-b", "sec-doc", ["span-b1"], "section", (60, 100)),
        ("span-a1", "sec-a", [], "source_span", (0, 30)),
        ("span-a2", "sec-a", [], "source_span", (30, 60)),
        ("span-b1", "sec-b", [], "source_span", (60, 100)),
    ]
    for node_id, parent, children, granularity, span in rows:
        _section(cl, graph, node_id, parent, children, granularity, span)


def _put(cl: Any, graph: str, node_id: str, *rest: Any) -> None:
    _section(cl, graph, node_id, *rest)


def _part(root: str, granularity: str, **policy: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "kind": "document_section",
        "hierarchy_revision": "section-policy-1",
        "community_ids": [],
        "summary_ids": [],
        "entity_ids": [],
        "parent_links": [],
        "stage": "document_section",
        "score_provenance": [
            {
                "scorer": "stored-embedding-cosine",
                "scorer_revision": "1",
                "basis": "stored",
            }
        ],
        "beam_width": 1,
        "max_depth": 8,
        "max_visited_communities": 50,
        "max_entity_candidates": 1,
        "max_reranker_calls": 1,
        "max_tokens": 1,
        "max_total_cost_micros": 1,
        "section_nodes": [],
    }
    base.update(policy)
    return {
        "hierarchy": {
            "query": "",
            "policy": base,
            "section": {
                "root_id": root,
                "document_revision": _REVISION,
                "granularity": granularity,
            },
        }
    }


def _round(cl: Any, graph: str, part: dict[str, Any]) -> dict[str, Any]:
    return run_round(cl, graph, request(_NO_ROWS, **part))


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.9.7.6")
def test_descent_three_granularities_zero_inference(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R081.9.7.6: paths, refusals, zero inference."""
    graph = f"sections-{uuid.uuid4().hex[:8]}"
    other = f"sections-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    cl.tenants.create(other)
    try:
        _seed(cl, graph)
        spans = _round(cl, graph, _part("sec-doc", "source_span"))
        assert spans["hierarchy_selected"] == ["span-a1", "span-a2", "span-b1"]
        assert spans["sufficiency"]["verdict"] == "sufficient"
        assert spans["inference"]["embedding"] == 0
        assert spans["inference"]["reranker"] == 0
        assert spans["inference"]["selection"] == 0
        premise = next(
            p for p in spans["premises"] if p["name"] == "retrieval.sections"
        )
        receipt = premise["value"]["receipt"]
        assert receipt["paths"][0] == ["sec-doc", "sec-a", "span-a1"]
        assert receipt["selected_spans"][2] == {"start": 60, "end": 100}
        assert _round(cl, graph, _part("sec-doc", "section"))["hierarchy_selected"] == [
            "sec-a",
            "sec-b",
        ]
        assert _round(cl, graph, _part("sec-doc", "document"))[
            "hierarchy_selected"
        ] == ["sec-doc"]

        def refused(part: dict[str, Any], code: str) -> str:
            with pytest.raises(Exception, match=code) as raised:
                _round(cl, graph, part)
            return str(raised.value)

        stale = _part("sec-doc", "source_span")
        stale["hierarchy"]["section"]["document_revision"] = "doc-rev-0"
        refused(stale, "RETRIEVAL_HIERARCHY_INVALID")
        refused(_part("sec-none", "source_span"), "RETRIEVAL_HIERARCHY_INVALID")

        _put(cl, graph, "cyc-doc", None, ["cyc-sec"], "document", (0, 10))
        _put(cl, graph, "cyc-sec", "cyc-doc", ["cyc-sec"], "section", (0, 10))
        refused(_part("cyc-doc", "section"), "RETRIEVAL_HIERARCHY_INVALID")

        _put(cl, graph, "wide-doc", None, ["wide-sec"], "document", (0, 10))
        _put(cl, graph, "wide-sec", "wide-doc", [], "section", (5, 50))
        refused(_part("wide-doc", "section"), "RETRIEVAL_HIERARCHY_INVALID")

        message = refused(
            _part("sec-doc", "source_span", max_depth=1), "RETRIEVAL_BUDGET_EXCEEDED"
        )
        assert "depth" in message and "incomplete: true" in message

        # Community retrieval output is unchanged by sections: no community part, no
        # hierarchy selection.
        plain = run_round(cl, graph, request(_NO_ROWS))
        assert plain["rows"] == [] and not plain.get("hierarchy_selected")

        # A second tenant reads nothing.
        try:
            foreign = _round(cl, other, _part("sec-doc", "source_span"))
        except Exception:
            foreign = None
        if foreign is not None:
            assert foreign["rows"] == [] and not foreign.get("hierarchy_selected")
    finally:
        cl.tenants.delete(graph)
        cl.tenants.delete(other)
        cl.close()
