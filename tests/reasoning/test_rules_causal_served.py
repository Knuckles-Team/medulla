"""Bounded rule fixpoint and causal verification over the served wire (R608).

MEDULLA-FEDERATED-QUERY-R608.

The session fixture in conftest starts and authenticates the native server. The
tests drive the served `RunRules` and `CausalVerify` methods and compare their
output with an independent oracle that implements the same published formulas.
"""

from __future__ import annotations

import math
import os

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import reasoning

CHAIN_LENGTH = 12
RULES = [
    "base: parent(?x,?y) -> ancestor(?x,?y)",
    "step: ancestor(?x,?y) ^ parent(?y,?z) -> ancestor(?x,?z)",
]


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _chain_ttl() -> str:
    lines = ["@prefix ex: <http://ex/> ."]
    lines += [f"ex:n{i} ex:parent ex:n{i + 1} ." for i in range(CHAIN_LENGTH)]
    return "\n".join(lines) + "\n"


def _local(term: str) -> str:
    return term.rstrip(">").rsplit("/", 1)[-1]


def _ancestor_oracle() -> set[tuple[str, str]]:
    """Every (ancestor, descendant) pair of the parent chain n0 -> n1 -> ..."""
    return {
        (f"n{i}", f"n{j}")
        for i in range(CHAIN_LENGTH + 1)
        for j in range(i + 1, CHAIN_LENGTH + 1)
    }


async def _run_rules(client: MedullaClient, max_steps: int | None) -> object:
    params: dict[str, object] = {
        "ontology_ttl": _chain_ttl(),
        "rules": RULES,
        "query_predicate": "ancestor",
        "derived_only": True,
    }
    if max_steps is not None:
        params["max_steps"] = max_steps
    result = await reasoning.send_run_rules(client, params)
    return reasoning.decode_run_rules(result)


def _derived_pairs(response: object) -> set[tuple[str, str]]:
    return {
        (_local(fact.args[0]), _local(fact.args[1]))
        for fact in response.facts  # type: ignore[attr-defined]
        if fact.derived and fact.predicate.rstrip(">").rsplit("/", 1)[-1] == "ancestor"
    }


VARIABLES = [
    {"id": "z", "parents": [], "bias": 0.0, "noise_var": 1.0},
    {"id": "x", "parents": [["z", 2.0]], "bias": 1.0, "noise_var": 0.01},
    {"id": "y", "parents": [["x", 0.5]], "bias": 0.0, "noise_var": 0.01},
]


def _observations() -> list[dict[str, float]]:
    rows = []
    for i in range(12):
        z = float(i)
        x = 1.0 + 2.0 * z + (0.1 if i % 2 == 0 else -0.1)
        y = 0.5 * x + (0.05 if i % 3 == 0 else -0.05)
        rows.append({"z": z, "x": x, "y": y})
    return rows


def _nrmse_oracle(
    variables: list[dict], rows: list[dict[str, float]]
) -> dict[str, float]:
    """RMSE of each variable's structural-equation residuals over the population
    standard deviation of the observed variable."""
    out = {}
    for var in variables:
        observed = [row[var["id"]] for row in rows]
        residuals = []
        for row in rows:
            predicted = var["bias"] + sum(w * row[p] for p, w in var["parents"])
            residuals.append(row[var["id"]] - predicted)
        rmse = math.sqrt(sum(r * r for r in residuals) / len(rows))
        mean = sum(observed) / len(observed)
        std = math.sqrt(sum((v - mean) ** 2 for v in observed) / len(observed))
        out[var["id"]] = rmse / std if std > 0 else (0.0 if rmse == 0 else math.inf)
    return out


# spec: MEDULLA-FEDERATED-QUERY-R608.3
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R608.3")
@pytest.mark.asyncio
async def test_both_methods_reach_the_kernels() -> None:
    client = await _connect()

    # RunRules: the unbounded fixpoint equals the independent closure; a bound of
    # one step reaches the same loop through `max_steps` and stops early.
    full = await _run_rules(client, None)
    assert full.complete is True  # type: ignore[attr-defined]
    assert full.bound_code is None  # type: ignore[attr-defined]
    assert _derived_pairs(full) == _ancestor_oracle()
    bounded = await _run_rules(client, 1)
    assert bounded.complete is False  # type: ignore[attr-defined]
    assert bounded.steps == 1  # type: ignore[attr-defined]
    assert bounded.bound_code == "RULE_FIXPOINT_BOUND"  # type: ignore[attr-defined]
    assert _derived_pairs(bounded) < _ancestor_oracle()

    # CausalVerify: the served per-variable nrmse equals the oracle's, and the
    # verdict follows the served limit.
    rows = _observations()
    verdict = await client.query.causal_verify(VARIABLES, rows)
    expected = _nrmse_oracle(VARIABLES, rows)
    assert [fit["id"] for fit in verdict["per_variable"]] == ["z", "x", "y"]
    for fit in verdict["per_variable"]:
        assert fit["nrmse"] == pytest.approx(expected[fit["id"]], rel=1e-9, abs=1e-12)
        assert fit["limit"] == verdict["per_variable"][0]["limit"]
    limit = verdict["per_variable"][0]["limit"]
    assert verdict["verdict"] == (
        "consistent" if all(v <= limit for v in expected.values()) else "inconsistent"
    )
    assert verdict["worst"] == max(expected, key=lambda k: expected[k])

    # A wrongly weighted model is inconsistent and names its worst variable.
    wrong = [dict(v) for v in VARIABLES]
    wrong[2] = {"id": "y", "parents": [["x", 5.0]], "bias": 0.0, "noise_var": 0.01}
    bad = await client.query.causal_verify(wrong, rows)
    bad_expected = _nrmse_oracle(wrong, rows)
    assert bad["verdict"] == "inconsistent"
    assert bad["worst"] == "y"
    assert [f["nrmse"] for f in bad["per_variable"] if f["id"] == "y"][
        0
    ] == pytest.approx(bad_expected["y"], rel=1e-9)
