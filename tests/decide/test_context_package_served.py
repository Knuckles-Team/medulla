"""MEDULLA-DECISION-ENGINE-R137.12 served proof of certified package selection.

A `context_package` question goes over the real wire (`Decide`). The fixture
holds three evidence units with two resolutions each, so there are 27 ways to
pick at most one resolution per unit. The test computes the optimum by
exhaustive search over all 27 and asserts the served selection equals it, with
the fewer-token tie-break. Further cases cover the reserved output, money and
latency ceilings, fractional rounding, zero capacity, unknown rates, a stale
source and a rejected certificate.

The rejected-certificate case needs the served engine to run a corrupting
solver for one named question. That seam exists only in an engine built with the
test-only `test-solver-seam` cargo feature (the served-test build in
`conftest.SERVED_TEST_FEATURES`); a release build has no such path. A
`MEDULLA_TEST_BINARY` supplied by a caller must be built with the same features.
"""

from __future__ import annotations

import itertools
import uuid

import pytest
from _decision_resident_fixture import publish
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError, _pack_binary_msgpack

DIGEST = "0123456789abcdef" * 4

# The question id the seam answers with a corrupted certificate.
CORRUPT_QUESTION_ID = "context.package.corrupt-certificate"

# (candidate id, evidence unit, resolution, tokens, value). Unit `c` ties on
# value across its two resolutions, so the fewer-token resolution must win.
POOL = [
    ("a0", "a", 0, 10, 100),
    ("a1", "a", 1, 40, 260),
    ("b0", "b", 0, 15, 90),
    ("b1", "b", 1, 50, 300),
    ("c0", "c", 0, 20, 120),
    ("c1", "c", 1, 30, 120),
]


def _candidate(row, source_id="source:a", revision=1, digest=DIGEST):
    cid, unit, resolution, tokens, value = row
    return {
        "candidate_id": cid,
        "evidence_unit": unit,
        "resolution": resolution,
        "source_id": source_id,
        "cited_start": 0,
        "cited_end": 10,
        "source_revision": revision,
        "content_digest": digest,
        "tokenizer_id": "tok",
        "tokenizer_version": "1",
        "token_weight": tokens,
        "value_micro": value,
        "measurement_provenance": "measured",
    }


def _budget(window=100, **ceilings):
    budget = {
        "window_tokens": window,
        "reserved_output_tokens": 0,
        "caller_ceiling_tokens": None,
        "money_ceiling_micro": None,
        "price_micro_per_token": None,
        "latency_ceiling_ms": None,
        "tokens_per_second": None,
    }
    budget.update(ceilings)
    return budget


def _package(budget, candidates=None):
    rows = candidates if candidates is not None else [_candidate(r) for r in POOL]
    return {
        "budget": budget,
        "candidates": rows,
        "value_floor_micro_per_token": 0,
        "node_budget": 100_000,
    }


def _capacity(budget):
    """The effective token capacity, computed here from the published rule."""
    capacity = max(budget["window_tokens"] - budget["reserved_output_tokens"], 0)
    if budget["caller_ceiling_tokens"] is not None:
        capacity = min(capacity, budget["caller_ceiling_tokens"])
    if budget["money_ceiling_micro"] is not None and budget["price_micro_per_token"]:
        capacity = min(
            capacity, budget["money_ceiling_micro"] // budget["price_micro_per_token"]
        )
    if budget["latency_ceiling_ms"] is not None:
        rate = budget["tokens_per_second"]
        capacity = min(capacity, budget["latency_ceiling_ms"] * rate // 1_000)
    return capacity


def _oracle(capacity):
    """Exhaustive search over every pick of at most one resolution per unit.

    Returns the optimal (value, tokens) and every id set that attains it: the
    best value first, then the fewest tokens.
    """
    units = sorted({row[1] for row in POOL})
    options = [[None] + [r for r in POOL if r[1] == unit] for unit in units]
    combinations = list(itertools.product(*options))
    assert len(combinations) == 27
    best = (0, 0)
    sets: set[frozenset] = {frozenset()}
    for combo in combinations:
        picked = [row for row in combo if row is not None]
        tokens = sum(row[3] for row in picked)
        value = sum(row[4] for row in picked)
        if tokens > capacity:
            continue
        if value > best[0] or (value == best[0] and tokens < best[1]):
            best, sets = (value, tokens), set()
        if (value, tokens) == best:
            sets.add(frozenset(row[0] for row in picked))
    return best, sets


@pytest.fixture
def served(clean_graph):
    """The published schema pin and a source graph holding `source:a`."""
    schema = publish(
        clean_graph,
        "pkg-schema",
        "feature_schema",
        ds.feature_schema_body([ds.feature("signal", ds.number("signal"))]),
    )
    graph = f"pkg-sources-{uuid.uuid4().hex[:8]}"
    clean_graph.tenants.create(graph, "Global")
    clean_graph._send(
        "AddNode",
        {
            "node_id": "source:a",
            "properties_msgpack": _pack_binary_msgpack(
                {"revision": 1, "content_digest": DIGEST}
            ),
        },
        graph=graph,
    )
    return clean_graph, schema, graph


def _decide(served_kit, package, question_id="context.package"):
    client, schema, graph = served_kit
    options = [
        dc.DeclaredOption(c["candidate_id"], numbers={"signal": 0})
        for c in package["candidates"]
    ]
    request = dc.decide_request(
        TEST_TENANT,
        (question_id, "context_package", "ordinary"),
        dc.declared(options),
        schema,
        params=[
            dc.param("context_packages", "context_packages", package),
            dc.param("source_graph", "text", graph),
        ],
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


def _chosen(record):
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return frozenset(score["option_id"] for score in outcome["scores"])


def _facts(record):
    return {premise["fact"]: premise["class"] for premise in record["premises"]}


def _refusal(served_kit, package, code, question_id="context.package"):
    with pytest.raises(EngineResponseError) as refused:
        _decide(served_kit, package, question_id)
    assert code in str(refused.value), refused.value


def _assert_optimum(served_kit, budget):
    record = _decide(served_kit, _package(budget))
    (_, tokens), optima = _oracle(_capacity(budget))
    chosen = _chosen(record)
    assert chosen in optima, (sorted(chosen), [sorted(s) for s in optima])
    assert sum(r[3] for r in POOL if r[0] in chosen) == tokens
    return record


# spec: MEDULLA-DECISION-ENGINE-R137.12
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R137.12")
def test_certified_package_end_to_end(served):
    # The window alone: the served selection is the exhaustive optimum, sealed
    # as a verified optimization whose totals carry no price or throughput.
    record = _assert_optimum(served, _budget(100))
    facts = _facts(record)
    assert facts["context_package.certificate_status=verified_optimal"] == "proof"
    assert "context_package.total_cost_micro=unknown" in facts
    assert "context_package.total_latency_ms=unknown" in facts
    assert str(record["resolution_kind"]).lower() == "optimization"
    assert str(record["evidence_class"]).lower() == "proof"

    # The tie-break: unit c's two resolutions are worth the same, so the
    # optimum takes the one that costs fewer tokens.
    unit_c = [_candidate(r) for r in POOL if r[1] == "c"]
    assert _chosen(_decide(served, _package(_budget(100), unit_c))) == {"c0"}

    # Reserved output comes off the window; the caller ceiling is a minimum.
    _assert_optimum(served, _budget(100, reserved_output_tokens=60))
    _assert_optimum(served, _budget(100, caller_ceiling_tokens=70))

    # The money ceiling converts through the price, the latency ceiling through
    # the throughput; both round down to whole tokens.
    _assert_optimum(
        served, _budget(100, money_ceiling_micro=600, price_micro_per_token=10)
    )
    _assert_optimum(served, _budget(100, latency_ceiling_ms=2500, tokens_per_second=20))
    money = _budget(100, money_ceiling_micro=609, price_micro_per_token=10)
    assert _capacity(money) == 60
    _assert_optimum(served, money)
    latency = _budget(100, latency_ceiling_ms=2999, tokens_per_second=20)
    assert _capacity(latency) == 59
    record = _assert_optimum(served, latency)
    assert "context_package.capacity_tokens=59" in _facts(record)

    # With both rates known the totals are recomputed, not unknown.
    priced = _assert_optimum(
        served,
        _budget(
            100,
            money_ceiling_micro=1000,
            price_micro_per_token=10,
            tokens_per_second=10,
        ),
    )
    assert "context_package.total_cost_micro=unknown" not in _facts(priced)

    # Zero capacity selects nothing and abstains with the registered code.
    for budget in (
        _budget(100, reserved_output_tokens=100),
        _budget(100, latency_ceiling_ms=1000, tokens_per_second=0),
    ):
        record = _decide(served, _package(budget))
        assert record["outcome"]["outcome"] == "abstained", record["outcome"]
        codes = [r.get("code") for r in record["outcome"]["reasons"]]
        assert "CONTEXT_PACKAGE_NONE_ADMISSIBLE" in codes, codes

    # A ceiling whose rate is unknown is refused, never assumed.
    _refusal(
        served,
        _package(_budget(100, money_ceiling_micro=600)),
        "CONTEXT_PACKAGE_MISSING_FACT",
    )
    _refusal(
        served,
        _package(_budget(100, latency_ceiling_ms=2500)),
        "CONTEXT_PACKAGE_MISSING_FACT",
    )

    # A source that moved, changed or is not visible is stale.
    for stale in (
        [_candidate(r, revision=2) if r[0] == "a0" else _candidate(r) for r in POOL],
        [
            _candidate(r, digest="f" * 64) if r[0] == "a0" else _candidate(r)
            for r in POOL
        ],
        [
            _candidate(r, source_id="source:absent") if r[0] == "a0" else _candidate(r)
            for r in POOL
        ],
    ):
        _refusal(served, _package(_budget(100), stale), "CONTEXT_PACKAGE_STALE_SOURCE")

    # A certificate the independent verifier rejects is refused, and the same
    # package under an ordinary question is answered: only the seam's question
    # is corrupted.
    _refusal(
        served,
        _package(_budget(100)),
        "CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE",
        CORRUPT_QUESTION_ID,
    )
    _, optima = _oracle(100)
    assert _chosen(_decide(served, _package(_budget(100)))) in optima
