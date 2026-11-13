"""MEDULLA-ORCH-VOCAB-R010.101 served usage totals: ``run_trace_usage`` over the
caller's ``RunTrace`` nodes, through ``Method::Uql``.

Traces carry the four disjoint token counters, the project and agent they are
attributed to, and their completion time; a ``PriceTable`` node pins one
revision of integer minor-unit rates per million tokens. Totals are summed
exactly per group and category and rounded once, half to even.
"""

from __future__ import annotations

import json
import uuid
from fractions import Fraction

import pytest

CATEGORIES = ["uncached_input", "cached_input", "output", "reasoning_output"]
PROPERTY = {
    "uncached_input": "uncachedInputTokens",
    "cached_input": "cachedInputTokens",
    "output": "outputTokens",
    "reasoning_output": "reasoningOutputTokens",
}
RATES = {
    "model-a": {
        "uncached_input": 300,
        "cached_input": 30,
        "output": 1_500,
        "reasoning_output": 1_500,
    },
    "model-b": {
        "uncached_input": 80,
        "cached_input": 8,
        "output": 400,
        "reasoning_output": 400,
    },
}


def _price_table(client, revision: str, rates: dict) -> None:
    table = {
        "schema_version": 1,
        "version": 1,
        "currency": "USD",
        "minor_unit_exponent": 2,
        "token_rates": rates,
    }
    client.nodes.add(
        f"price-table:{revision}",
        {"node_type": "PriceTable", "revision": revision, "table": json.dumps(table)},
    )


def _trace(client, model: str, project: str, agent: str, tokens: list[int], at: int):
    props = {
        "node_type": "RunTrace",
        "status": "completed",
        "runModel": model,
        "projectId": project,
        "agentId": agent,
        "accountingRevision": "acct-1",
        "completedAtMs": at,
    }
    for category, count in zip(CATEGORIES, tokens, strict=True):
        props[PROPERTY[category]] = count
    client.nodes.add(f"trace-usage-{uuid.uuid4().hex[:10]}", props)
    return {"model": model, "project": project, "agent": agent, "tokens": tokens}


def _usage(client, revision: str, grouping: str, **extra) -> dict:
    params = {
        "window_start_ms": 1_000,
        "window_end_ms": 2_000,
        "grouping": grouping,
        "currency": "USD",
        "price_table_revision": revision,
        **extra,
    }
    result = client.query.uql("run_trace_usage", params)
    assert result["kind"] == "rows"
    return result


def _half_even(value: Fraction) -> int:
    return round(value)  # Python rounds a Fraction half to even.


def _ledger(traces: list[dict], key: str) -> dict[str, dict[str, int]]:
    """The reference ledger: exact numerators per group and category, rounded once."""
    groups: dict[str, dict[str, Fraction]] = {}
    tokens: dict[str, dict[str, int]] = {}
    for trace in traces:
        group = trace[key]
        cost = groups.setdefault(group, dict.fromkeys(CATEGORIES, Fraction(0)))
        count = tokens.setdefault(group, dict.fromkeys(CATEGORIES, 0))
        for category, used in zip(CATEGORIES, trace["tokens"], strict=True):
            cost[category] += Fraction(
                used * RATES[trace["model"]][category], 1_000_000
            )
            count[category] += used
    out = {}
    for group, cost in groups.items():
        row = {f"{c}_tokens": tokens[group][c] for c in CATEGORIES}
        row["total_tokens"] = sum(tokens[group].values())
        rounded = {c: _half_even(cost[c]) for c in CATEGORIES}
        row.update({f"{c}_cost_minor": rounded[c] for c in CATEGORIES})
        row["total_cost_minor"] = sum(rounded.values())
        out[group] = row
    return out


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.101")
def test_grouped_totals_match_reference_ledger(clean_graph):
    revision = f"prices-{uuid.uuid4().hex[:8]}"
    _price_table(clean_graph, revision, RATES)
    traces = [
        _trace(
            clean_graph,
            "model-a",
            "proj-1",
            "agent-x",
            [120_000, 40_000, 9_000, 3_000],
            1_100,
        ),
        _trace(
            clean_graph, "model-b", "proj-1", "agent-y", [55_555, 0, 7_777, 0], 1_200
        ),
        _trace(
            clean_graph,
            "model-a",
            "proj-2",
            "agent-x",
            [1_000_001, 333, 50_000, 12_345],
            1_300,
        ),
        _trace(
            clean_graph, "model-b", "proj-2", "agent-y", [999, 999, 999, 999], 1_999
        ),
    ]
    # Outside the window: never counted.
    _trace(clean_graph, "model-a", "proj-1", "agent-x", [10**6] * 4, 2_000)

    for grouping, key in [
        ("project", "project"),
        ("agent", "agent"),
        ("model", "model"),
    ]:
        result = _usage(clean_graph, revision, grouping)
        got = {row["id"]: row["channels"] for row in result["rows"]}
        assert got == _ledger(traces, key), grouping
        assert result["warnings"] == []

    # Paging: one group per page, resumed by the snapshot cursor.
    first = _usage(clean_graph, revision, "project", page_size=1)
    assert [row["id"] for row in first["rows"]] == ["proj-1"]
    cursor = f"{first['snapshot_revision']}:{first['rows'][-1]['id']}"
    rest = _usage(clean_graph, revision, "project", page_size=1, cursor=cursor)
    assert [row["id"] for row in rest["rows"]] == ["proj-2"]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.101")
def test_unknown_price_and_half_even_boundaries(clean_graph):
    revision = f"prices-{uuid.uuid4().hex[:8]}"
    # One minor unit per million tokens: 2.5 and 3.5 minor units sit on the
    # half-even boundary.
    _price_table(clean_graph, revision, {"model-h": dict.fromkeys(CATEGORIES, 1)})
    _trace(clean_graph, "model-h", "even", "agent-h", [1_250_000, 0, 0, 0], 1_100)
    _trace(clean_graph, "model-h", "even", "agent-h", [1_250_000, 0, 0, 0], 1_200)
    _trace(clean_graph, "model-h", "odd", "agent-h", [3_500_000, 0, 0, 0], 1_300)
    _trace(
        clean_graph, "model-unpriced", "unpriced", "agent-u", [10, 20, 30, 40], 1_400
    )

    result = _usage(clean_graph, revision, "project")
    rows = {row["id"]: row["channels"] for row in result["rows"]}
    # Two traces of 1.25 sum to 2.5 exactly and round once to 2; per-trace
    # rounding would have said 2 as well but only by accident of 1 + 1.
    assert rows["even"]["uncached_input_cost_minor"] == 2
    assert rows["odd"]["uncached_input_cost_minor"] == 4
    # A model without a rate: its tokens are known, its cost is unknown, never zero.
    assert rows["unpriced"]["total_tokens"] == 100
    assert rows["unpriced"]["total_cost_minor"] is None
    assert any("PRICE_UNKNOWN: unpriced|" in w for w in result["warnings"])

    # A trace without the grouping attribute is reported, not folded into zero.
    clean_graph.nodes.add(
        f"trace-usage-{uuid.uuid4().hex[:10]}",
        {
            "node_type": "RunTrace",
            "status": "completed",
            "runModel": "model-h",
            "completedAtMs": 1_500,
            "uncachedInputTokens": 1,
            "cachedInputTokens": 1,
            "outputTokens": 1,
            "reasoningOutputTokens": 1,
        },
    )
    attributed = _usage(clean_graph, revision, "project")
    assert any("ATTRIBUTION_UNKNOWN" in w for w in attributed["warnings"])

    # Another currency without a pinned FX revision: every cost is unknown.
    other = _usage(clean_graph, revision, "project", currency="EUR")
    assert all(row["channels"]["total_cost_minor"] is None for row in other["rows"])
    assert any("CURRENCY_MISMATCH" in w for w in other["warnings"])

    # An unknown table revision: costs unknown with the reason.
    missing = _usage(clean_graph, "prices-absent", "project")
    assert all(row["channels"]["total_cost_minor"] is None for row in missing["rows"])
    assert any("PRICE_TABLE_UNKNOWN" in w for w in missing["warnings"])
