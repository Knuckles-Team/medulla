"""MEDULLA-DECISION-ENGINE-R146.6 served temporal basis: three documents carry
distinct storage, event and narrative dates; a first query over the storage
clock returns two rows; `Decide` names the event basis and the widened fallback
window; running that window through the served SQL query returns more rows."""

from __future__ import annotations

import hashlib
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT

from medulla import decision_client as dc

DAY = 86_400_000

# (id, storage, event, narrative) in days since the epoch.
DOCS = [
    ("doc-a", 101, 95, 103),
    ("doc-b", 104, 96, 108),
    ("doc-c", 130, 115, 90),
]
WINDOW = (100 * DAY, 110 * DAY)


def _seed(client: Any) -> None:
    for doc_id, storage, event, narrative in DOCS:
        client.nodes.add(
            doc_id,
            {
                "type": "TemporalDoc",
                "storage_ms": storage * DAY,
                "event_ms": event * DAY,
                "narrative_ms": narrative * DAY,
            },
        )


def _rows(client: Any, column: str, lo: int, hi: int) -> list[str]:
    result = client.query.uql(
        f"MATCH (:TemporalDoc) WHERE {column} BETWEEN $lo AND $hi",
        {"lo": lo, "hi": hi},
    )
    return sorted(row["id"] for row in result["rows"])


def _decide(client: Any, schema: dict[str, Any], first_pass_rows: int) -> dict:
    query = {
        "schema_version": 1,
        "query_digest": hashlib.sha256(b"when was the audit").hexdigest(),
        "window_from_ms": WINDOW[0],
        "window_to_ms": WINDOW[1],
        "first_pass_rows": first_pass_rows,
        "context": {
            "storage_ms": 120 * DAY,
            "event_ms": 90 * DAY,
            "narrative_ms": 105 * DAY,
        },
    }
    request = dc.decide_request(
        TEST_TENANT,
        ("time.basis", "temporal_basis", "ordinary"),
        dc.declared(
            dc.DeclaredOption(name) for name in ("event", "narrative", "storage")
        ),
        schema,
        params=[dc.param("temporal_query", "temporal_query", query)],
        max_records=1,
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


# spec: MEDULLA-DECISION-ENGINE-R146.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R146.6")
def test_three_clock_confusion_and_fallback(clean_graph):
    _seed(clean_graph)
    schema = publish(clean_graph, "time-schema", "feature_schema", schema_body())

    first = _rows(clean_graph, "storage_ms", *WINDOW)
    assert first == ["doc-a", "doc-b"]

    record = _decide(clean_graph, schema, len(first))
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    assert [score["option_id"] for score in outcome["scores"]] == ["event"]
    facts = {premise["fact"] for premise in record["premises"]}
    assert "temporal:basis=event" in facts
    assert "temporal:confusion=true" in facts
    assert "temporal:fallback_built=true" in facts
    # The default widen is 7 days on each side of the asked window.
    from_ms, to_ms = 93 * DAY, 117 * DAY
    assert f"temporal:fallback_from_ms={from_ms}" in facts
    assert f"temporal:fallback_to_ms={to_ms}" in facts

    # The caller runs the returned window on the chosen basis: more rows.
    fallback = _rows(clean_graph, "event_ms", from_ms, to_ms)
    assert fallback == ["doc-a", "doc-b", "doc-c"]
    assert len(fallback) > len(first)

    # A sufficient first pass builds no fallback window.
    enough = _decide(clean_graph, schema, 3)
    enough_facts = {premise["fact"] for premise in enough["premises"]}
    assert "temporal:fallback_built=false" in enough_facts
    assert not any(f.startswith("temporal:fallback_from_ms") for f in enough_facts)
