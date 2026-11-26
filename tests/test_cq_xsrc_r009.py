"""High-confidence agent claims against a new external report (MEDULLA-CQ-XSRC-R009).

Two fixture programs share one selection and the typed params `$agent_id`,
`$report_id` and the vector `$report_vec`. Their literals (confidence 0.8, a 30 day
valid window, similarity 0.75, three clusters, a seven day half-life) are query text,
not knobs. The selection picks the claims of one agent that touch the report, by a
`:mentions` edge or by similarity to the report vector. The fixture `xsrc_r009.uql`
then clusters the evidence of those claims; `xsrc_r009_decay.uql` reads their
contradictions and decays each by age. An engine has one result per program, so the
two readings are two programs of the one selection.
"""

from __future__ import annotations

import time
import uuid
from pathlib import Path
from typing import Any

import pytest

from medulla.client import EngineResponseError

FIXTURES = Path(__file__).parent / "fixtures" / "cq"
FIXTURE = FIXTURES / "xsrc_r009.uql"
DECAY_FIXTURE = FIXTURES / "xsrc_r009_decay.uql"

REPORT_VEC = [1.0, 0.0, 0.0, 0.0]
HALF_LIFE_MS = 604_800_000
DAY_MS = 86_400_000


def _program(path: Path = FIXTURE, mode: str = "") -> str:
    text = path.read_text(encoding="utf-8").strip()
    # The pragma stays first; EXPLAIN or PROFILE follows it.
    return text.replace("UQL 1;", f"UQL 1; {mode}".rstrip(), 1)


def _params(tag: str, report: str = "report") -> dict[str, Any]:
    return {
        "agent_id": f"agent:{tag}",
        "report_id": f"{report}:{tag}",
        "report_vec": REPORT_VEC,
    }


@pytest.mark.spec("MEDULLA-CQ-XSRC-R009.1")
def test_fixture_compiles_to_one_plan(clean_graph) -> None:
    """Verifies MEDULLA-CQ-XSRC-R009.1: each fixture compiles to one plan DAG with one
    root, and its literals are program text."""
    text = FIXTURE.read_text(encoding="utf-8")
    for literal in ("0.8", "30 d", "0.75", "K 3", "$agent_id", "$report_id"):
        assert literal in text, literal
    assert "$report_vec" in text
    assert "604800" in DECAY_FIXTURE.read_text(encoding="utf-8")
    params = _params("compile")
    for path in (FIXTURE, DECAY_FIXTURE):
        explained = clean_graph.query.uql(_program(path, "EXPLAIN"), params)
        assert explained["kind"] == "explain", explained
        assert explained["plan_root_count"] == 1, explained
        assert explained["plan_node_count"] == len(explained["stages"]), explained
        assert explained["plan_id_count"] == 1, explained


def _claim(client, tag: str, name: str, **props: Any) -> str:
    node = f"claim:{name}:{tag}"
    properties = {"type": "Claim", "agent_id": f"agent:{tag}", "confidence": 0.99}
    client.nodes.add(node, properties | props)
    return node


def _node(client, node: str, kind: str, **props: Any) -> str:
    client.nodes.add(node, {"type": kind} | props)
    return node


def _ingest(client, tag: str) -> dict[str, str]:
    """One agent's claims against one report, with evidence and contradictions.

    Selected: `mention-a` and `mention-b` (a `:mentions` edge) and `similar` (no edge, a
    vector near the report vector). Not selected: `low` (confidence 0.2), `far` (no
    edge, a vector orthogonal to the report vector) and `foreign` (another agent's).
    """
    report = _node(client, f"report:{tag}", "ExternalReport")
    ids: dict[str, str] = {"report": report}
    far = [0.0, 1.0, 0.0, 0.0]
    for name, props, vector, mentions in (
        ("mention-a", {}, far, True),
        ("mention-b", {}, far, True),
        ("similar", {}, [0.9, 0.1, 0.0, 0.0], False),
        ("low", {"confidence": 0.2}, far, True),
        ("far", {}, far, False),
        ("foreign", {"agent_id": "agent:someone-else"}, far, True),
    ):
        ids[name] = _claim(client, tag, name, **props)
        client.graph.add_embedding(ids[name], vector)
        if mentions:
            client.edges.add(ids[name], report, {"relationship": "mentions"})
    # Evidence: two supporters per claim, one direction per claim, so the three
    # clusters are the three selected claims' evidence; `low` is not selected.
    directions = {
        "mention-a": [0.0, 0.0, 1.0, 0.0],
        "mention-b": [0.0, 0.0, 0.0, 1.0],
        "similar": [0.0, 0.7, 0.7, 0.0],
        "low": [0.5, 0.5, 0.5, 0.5],
    }
    for claim, vector in directions.items():
        for index in (1, 2):
            evidence = _node(
                client, f"evidence:{claim}-{index}:{tag}", "Evidence", confidence=0.95
            )
            client.graph.add_embedding(evidence, vector)
            client.edges.add(evidence, ids[claim], {"relationship": "SUPPORTS"})
            ids[f"evidence:{claim}-{index}"] = evidence
    return ids


def _contradict(client, tag: str, ids: dict[str, str], now_ms: int) -> dict[str, int]:
    """One contradiction per claim, at a fixed age; returns the age in ms by claim."""
    ages = {
        "mention-a": 7 * DAY_MS,
        "mention-b": 0,
        "similar": 14 * DAY_MS,
        "low": 7 * DAY_MS,
    }
    for claim, age in ages.items():
        contradiction = _node(
            client,
            f"contradiction:{claim}:{tag}",
            "Evidence",
            confidence=0.3,
            valid_time_ms=now_ms - age,
        )
        client.edges.add(contradiction, ids[claim], {"relationship": "CONTRADICTS"})
        ids[f"contradiction:{claim}"] = contradiction
    return ages


@pytest.mark.spec("MEDULLA-CQ-XSRC-R009.2")
def test_evidence_clusters_by_embedding(clean_graph) -> None:
    """Verifies MEDULLA-CQ-XSRC-R009.2: the evidence of the selected claims, and only
    theirs, comes back with a cluster id and a centroid distance; evidence that points
    the same way shares a cluster and the three directions fall in three clusters.

    The cluster numbers come from the seeded clustering, so the expected value is the
    partition (which evidence shares a cluster), not the numbers themselves."""
    tag = uuid.uuid4().hex[:8]
    ids = _ingest(clean_graph, tag)
    result = clean_graph.query.uql(_program(), _params(tag))
    assert result["kind"] == "rows", result
    rows = {row["id"]: row["channels"] for row in result["rows"]}
    expected = {
        ids[f"evidence:{claim}-{index}"]
        for claim in ("mention-a", "mention-b", "similar")
        for index in (1, 2)
    }
    assert set(rows) == expected, rows
    for channels in rows.values():
        assert "cluster_id" in channels and "centroid_distance" in channels, channels
    for claim in ("mention-a", "mention-b", "similar"):
        first, second = (ids[f"evidence:{claim}-{index}"] for index in (1, 2))
        assert rows[first]["cluster_id"] == rows[second]["cluster_id"], claim
    clusters = {
        rows[ids[f"evidence:{claim}-1"]]["cluster_id"]
        for claim in ("mention-a", "mention-b", "similar")
    }
    assert len(clusters) == 3, rows


def _decay_weight(now_ms: int, valid_time_ms: int) -> float:
    return 0.5 ** (((now_ms - valid_time_ms) / 1000.0) / (HALF_LIFE_MS / 1000.0))


@pytest.mark.spec("MEDULLA-CQ-XSRC-R009.3")
def test_contradiction_weights_decay_by_age(clean_graph) -> None:
    """Verifies MEDULLA-CQ-XSRC-R009.3: each contradiction of a selected claim comes
    back with its decayed weight `0.5^(age / half_life)`, and a contradiction of an
    unselected claim does not.

    The decay reads the wall clock, so the weight is bracketed by the weight at the
    clock readings taken before and after the query (scores are 32-bit floats, hence
    the 1e-6 slack)."""
    tag = uuid.uuid4().hex[:8]
    ids = _ingest(clean_graph, tag)
    start_ms = int(time.time() * 1000)
    ages = _contradict(clean_graph, tag, ids, start_ms)
    before_ms = int(time.time() * 1000)
    result = clean_graph.query.uql(_program(DECAY_FIXTURE), _params(tag))
    after_ms = int(time.time() * 1000)
    assert result["kind"] == "rows", result
    scores = {row["id"]: row["score"] for row in result["rows"]}
    assert set(scores) == {
        ids[f"contradiction:{claim}"] for claim in ("mention-a", "mention-b", "similar")
    }, scores
    for claim in ("mention-a", "mention-b", "similar"):
        valid_time_ms = start_ms - ages[claim]
        high = _decay_weight(before_ms, valid_time_ms)
        low = _decay_weight(after_ms, valid_time_ms)
        score = scores[ids[f"contradiction:{claim}"]]
        assert low - 1e-6 <= score <= high + 1e-6, (claim, score, low, high)
    # A contradiction of one half-life of age weighs one half.
    assert abs(scores[ids["contradiction:mention-a"]] - 0.5) < 1e-5, scores


@pytest.mark.spec("MEDULLA-CQ-XSRC-R009.5")
def test_unknown_report_refuses(clean_graph) -> None:
    """Verifies MEDULLA-CQ-XSRC-R009.5: an unknown report id refuses with
    `CQ_REPORT_NOT_FOUND` and returns no rows."""
    tag = uuid.uuid4().hex[:8]
    _ingest(clean_graph, tag)
    params = _params(tag) | {"report_id": f"report:missing-{tag}"}
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.uql(_program(), params)
    assert "CQ_REPORT_NOT_FOUND" in str(refused.value)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R009.5")
def test_empty_window_is_empty_result(clean_graph) -> None:
    """Verifies MEDULLA-CQ-XSRC-R009.5: a known report whose claims are all outside
    the window returns an empty typed result and no error."""
    tag = uuid.uuid4().hex[:8]
    report = _node(clean_graph, f"report:{tag}", "ExternalReport")
    claim = _claim(clean_graph, tag, "expired", valid_until=1)
    clean_graph.graph.add_embedding(claim, REPORT_VEC)
    clean_graph.edges.add(claim, report, {"relationship": "mentions"})
    result = clean_graph.query.uql(_program(), _params(tag))
    assert result["kind"] == "rows", result
    assert result["rows"] == [], result
