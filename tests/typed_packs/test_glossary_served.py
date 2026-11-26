"""Served business glossary (MEDULLA-TYPED-PACKS-R801).

Drives `Glossary` through the real served surface: a scheme is imported with its
terms, a term moves through review, and an approved term binds to an asset.
"""

# ruff: noqa: E501

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
import stewardship_support as support
from conftest import request_context
from stewardship_support import REVIEWER

from medulla.client import SyncMedullaClient

SCHEME = "urn:scheme:finance"
REVENUE = "urn:term:revenue"
ASSET = "col:orders.total"


def _connect(graph: str | None = None) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _op(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return _decode(client._send("Glossary", {"op": op}, graph))


def _props(client: Any, graph: str, node_id: str) -> dict[str, Any]:
    return _decode(client._send("GetNodeProperties", {"node_id": node_id}, graph))


def _term(status: str = "draft") -> dict[str, Any]:
    return {
        "scheme": SCHEME,
        "pref_label": "Revenue",
        "alt_labels": ["Turnover"],
        "definition": "Income from sales.",
        "status": status,
    }


def _approve(client: Any, graph: str, iri: str) -> str:
    """Submit a term, then approve it under an approved change request; returns the request id."""
    submitted = _op(client, graph, {"op": "transition", "iri": iri, "event": "submit"})
    assert submitted["status"] == "in_review"
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        request_id = support.approved_request(client, reviewer, graph, iri)
    finally:
        reviewer.close()
    approved = _op(
        client,
        graph,
        {
            "op": "transition",
            "iri": iri,
            "event": "approve",
            "change_request": request_id,
        },
    )
    assert approved["status"] == "approved"
    return request_id


@pytest.fixture
def served():
    graph = f"medulla-r801-{uuid.uuid4().hex}"
    client = _connect(None)
    created = False
    try:
        client.tenants.create(graph, graph_type="Global")
        created = True
        support.register_agents(graph, REVIEWER)
        client._send(
            "AddNode",
            {
                "node_id": ASSET,
                "properties_msgpack": msgpack.packb(
                    {"type": "Column"}, use_bin_type=True
                ),
            },
            graph,
        )
        yield graph, client
    finally:
        try:
            if created:
                client.tenants.delete(graph)
        finally:
            client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801.3")
def test_import_scheme_and_bind_term(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R801.3: import a scheme, review a term, bind it."""
    graph, client = served
    put = _op(
        client,
        graph,
        {
            "op": "put_scheme",
            "scheme": SCHEME,
            "terms": [{"iri": REVENUE, "term": _term()}],
        },
    )
    assert put["iri"] == SCHEME and put["written"] == 2
    assert _props(client, graph, REVENUE)["status"] == "draft"

    # A put never moves a status: a new term must start in draft.
    with pytest.raises(Exception, match="draft"):
        _op(
            client,
            graph,
            {"op": "put_term", "iri": "urn:term:cost", "term": _term("approved")},
        )

    _approve(client, graph, REVENUE)
    assert _props(client, graph, REVENUE)["status"] == "approved"

    bound = _op(client, graph, {"op": "bind", "term": REVENUE, "asset": ASSET})
    assert bound["iri"] == REVENUE


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801.4")
def test_glossary_refusals(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R801.4: each code is refused and nothing changes."""
    graph, client = served
    other = "urn:term:turnover"
    _op(
        client,
        graph,
        {
            "op": "put_scheme",
            "scheme": SCHEME,
            "terms": [
                {"iri": REVENUE, "term": _term()},
                {"iri": other, "term": _term()},
            ],
        },
    )
    # GLOSSARY_CYCLE: two terms that name each other as broader.
    cyclic = [
        {"iri": REVENUE, "term": {**_term(), "broader": [other]}},
        {"iri": other, "term": {**_term(), "broader": [REVENUE]}},
    ]
    with pytest.raises(Exception, match="GLOSSARY_CYCLE"):
        _op(client, graph, {"op": "put_scheme", "scheme": SCHEME, "terms": cyclic})
    assert _props(client, graph, REVENUE)["broader"] == []

    # GLOSSARY_UNREVIEWED: approving needs an approved change request naming the term.
    _op(client, graph, {"op": "transition", "iri": REVENUE, "event": "submit"})
    with pytest.raises(Exception, match="GLOSSARY_UNREVIEWED"):
        _op(client, graph, {"op": "transition", "iri": REVENUE, "event": "approve"})
    assert _props(client, graph, REVENUE)["status"] == "in_review"

    # GLOSSARY_LABEL_TAKEN: a second term with the label of an approved one.
    _approve_in_review(client, graph, REVENUE)
    _op(client, graph, {"op": "transition", "iri": other, "event": "submit"})
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        request_id = support.approved_request(client, reviewer, graph, other)
    finally:
        reviewer.close()
    with pytest.raises(Exception, match="GLOSSARY_LABEL_TAKEN"):
        _op(
            client,
            graph,
            {
                "op": "transition",
                "iri": other,
                "event": "approve",
                "change_request": request_id,
            },
        )
    assert _props(client, graph, other)["status"] == "in_review"


def _approve_in_review(client: Any, graph: str, iri: str) -> None:
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        request_id = support.approved_request(client, reviewer, graph, iri)
    finally:
        reviewer.close()
    _op(
        client,
        graph,
        {
            "op": "transition",
            "iri": iri,
            "event": "approve",
            "change_request": request_id,
        },
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801.5")
def test_status_history_reads_back(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R801.5: each move records its actor and change request."""
    graph, client = served
    _op(
        client,
        graph,
        {
            "op": "put_scheme",
            "scheme": SCHEME,
            "terms": [{"iri": REVENUE, "term": _term()}],
        },
    )
    request_id = _approve(client, graph, REVENUE)
    actor = request_context()["agent_id"]
    approved = _props(
        client, graph, f"glossary_move:{request_context()['tenant']}:{REVENUE}:approved"
    )
    assert approved["subject"] == REVENUE
    assert approved["subject_kind"] == "term"
    assert (approved["from"], approved["to"]) == ("in_review", "approved")
    assert approved["actor"] == actor
    assert approved["change_request"] == request_id
    submitted = _props(
        client,
        graph,
        f"glossary_move:{request_context()['tenant']}:{REVENUE}:in_review",
    )
    assert (submitted["from"], submitted["to"]) == ("draft", "in_review")
    assert "supersedes" not in submitted
    assert (
        approved["supersedes"]
        == f"glossary_move:{request_context()['tenant']}:{REVENUE}:in_review"
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801.7")
def test_proposed_description_needs_review(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R801.7: a proposal becomes the description only once reviewed."""
    graph, client = served
    inputs = {"lineage_refs": ["col:orders.subtotal"]}
    proposal = {
        "target": ASSET,
        "text": "Order total in the order currency.",
        "inputs": inputs,
        "state": "proposed",
    }
    # No input at all is refused under its own code.
    with pytest.raises(Exception, match="PROPOSAL_INPUTS_MISSING"):
        _op(
            client,
            graph,
            {"op": "propose_description", "proposal": {**proposal, "inputs": {}}},
        )
    # Nobody governs the asset yet, so there is nobody to review.
    with pytest.raises(Exception, match="STEWARD_ROLE_MISSING"):
        _op(client, graph, {"op": "propose_description", "proposal": proposal})
    support.assign_stewards(
        client, graph, ASSET, support.principal(), support.principal(REVIEWER)
    )

    proposed = _op(client, graph, {"op": "propose_description", "proposal": proposal})
    proposal_id = proposed["iri"]
    stored = _props(client, graph, proposal_id)
    assert stored["state"] == "proposed" and stored["lineage_refs"] == [
        "col:orders.subtotal"
    ]
    assert "description" not in _props(client, graph, ASSET)

    reviewer = support.connect(agent_id=REVIEWER)
    try:
        # Approving before the change request is approved is refused; nothing is described.
        with pytest.raises(Exception, match="(?i)review"):
            _decode(
                reviewer._send(
                    "Glossary",
                    {
                        "op": {
                            "op": "review_description",
                            "proposal": proposal_id,
                            "approve": True,
                        }
                    },
                    graph,
                )
            )
        assert "description" not in _props(client, graph, ASSET)
        support.stewardship(
            reviewer,
            graph,
            {"op": "review", "request_id": stored["change_request"], "approve": True},
        )
        approved = _decode(
            reviewer._send(
                "Glossary",
                {
                    "op": {
                        "op": "review_description",
                        "proposal": proposal_id,
                        "approve": True,
                    }
                },
                graph,
            )
        )
        assert approved["written"] == 2
    finally:
        reviewer.close()
    assert _props(client, graph, proposal_id)["state"] == "approved"
    assert _props(client, graph, ASSET)["description"] == proposal["text"]


CATALOG = "https://knuckles-team.github.io/ontology/catalog#"


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801.6")
def test_import_scheme_bind_term_and_review_lifecycle(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R801.6: scheme, review lifecycle, bind and search."""
    graph, client = served
    _op(
        client,
        graph,
        {
            "op": "put_scheme",
            "scheme": SCHEME,
            "terms": [{"iri": REVENUE, "term": _term()}],
        },
    )

    # The term moves through a change request and binds to the column.
    _approve(client, graph, REVENUE)
    bound = _op(client, graph, {"op": "bind", "term": REVENUE, "asset": ASSET})
    assert bound["iri"] == REVENUE

    # One served query lists the glossary terms of the graph, and the term is found by its
    # alternative label.
    rows = client.query.uql(f"MATCH () WHERE type = '{CATALOG}CatalogGlossaryTerm'")
    assert rows["kind"] == "rows", rows
    by_alt = [
        row["id"]
        for row in rows["rows"]
        if "turnover"
        in [
            str(label).lower()
            for label in _props(client, graph, row["id"])["alt_labels"]
        ]
    ]
    assert by_alt == [REVENUE]
    assert _props(client, graph, REVENUE)["status"] == "approved"
