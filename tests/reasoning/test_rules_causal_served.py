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

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated import graph as graph_methods
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


async def _run_rules(
    client: MedullaClient, max_steps: int | None, explain: bool = False
) -> object:
    params: dict[str, object] = {
        "ontology_ttl": _chain_ttl(),
        "rules": RULES,
        "query_predicate": "ancestor",
        "derived_only": True,
        "explain": explain,
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


# proves R608.4: Refusal
# spec: MEDULLA-FEDERATED-QUERY-R608.4
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R608.4")
@pytest.mark.asyncio
async def test_rule_bound_flag_and_causal_refusals() -> None:
    client = await _connect()
    nodes_before = await graph_methods.send_node_count(client)

    # Reaching the step bound is a normal answer: the partial derivation, flagged.
    bounded = await _run_rules(client, 2)
    assert bounded.complete is False  # type: ignore[attr-defined]
    assert bounded.bound_code == "RULE_FIXPOINT_BOUND"  # type: ignore[attr-defined]
    assert 0 < len(bounded.facts) < len(_ancestor_oracle())  # type: ignore[attr-defined]

    # Fewer than eight observations are refused, exactly eight are answered.
    rows = _observations()
    with pytest.raises(EngineResponseError, match="CAUSAL_OBSERVATIONS_INSUFFICIENT"):
        await client.query.causal_verify(VARIABLES, rows[:7])
    assert (await client.query.causal_verify(VARIABLES, rows[:8]))["per_variable"]

    # A cyclic model and a model naming a missing parent are refused.
    cyclic = [
        {"id": "a", "parents": [["b", 1.0]], "bias": 0.0, "noise_var": 0.1},
        {"id": "b", "parents": [["a", 1.0]], "bias": 0.0, "noise_var": 0.1},
    ]
    cyclic_rows = [{"a": float(i), "b": float(i)} for i in range(12)]
    with pytest.raises(EngineResponseError, match="CAUSAL_MODEL_INVALID"):
        await client.query.causal_verify(cyclic, cyclic_rows)
    orphan = [{"id": "a", "parents": [["ghost", 1.0]], "bias": 0.0, "noise_var": 0.1}]
    with pytest.raises(EngineResponseError, match="CAUSAL_MODEL_INVALID"):
        await client.query.causal_verify(orphan, [{"a": float(i)} for i in range(12)])

    # Neither the bound nor a refusal wrote a node.
    assert await graph_methods.send_node_count(client) == nodes_before


def _leaf_rules(node: object) -> set[str]:
    if node.truncated:  # type: ignore[attr-defined]
        return set()  # a cut node is bounded by the proof budget, not a leaf
    premises = node.premises  # type: ignore[attr-defined]
    if not premises:
        return {node.rule}  # type: ignore[attr-defined]
    return set().union(*(_leaf_rules(p) for p in premises))


# proves R608.5: Evidence
# spec: MEDULLA-FEDERATED-QUERY-R608.5
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R608.5")
@pytest.mark.asyncio
async def test_evidence_after_success_and_bound() -> None:
    client = await _connect()

    done = await _run_rules(client, None, explain=True)
    assert done.complete is True  # type: ignore[attr-defined]
    assert done.steps > 0  # type: ignore[attr-defined]
    assert 0 < done.rules_fired <= done.steps  # type: ignore[attr-defined]
    derived = [fact for fact in done.facts if fact.derived]  # type: ignore[attr-defined]
    assert derived
    assert all(fact.proof is not None for fact in derived)
    # Every derived fact's proof names a rule and bottoms out in asserted leaves.
    for fact in derived:
        assert fact.proof.rule != "asserted"
        assert _leaf_rules(fact.proof) <= {"asserted"}

    stopped = await _run_rules(client, 3)
    assert stopped.complete is False  # type: ignore[attr-defined]
    assert stopped.steps == 3  # type: ignore[attr-defined]
    assert stopped.rules_fired <= stopped.steps  # type: ignore[attr-defined]
    assert stopped.steps < done.steps  # type: ignore[attr-defined]
    assert all(fact.proof is None for fact in stopped.facts)  # type: ignore[attr-defined]

    verdict = await client.query.causal_verify(VARIABLES, _observations())
    assert [fit["id"] for fit in verdict["per_variable"]] == ["z", "x", "y"]
    for fit in verdict["per_variable"]:
        assert fit["nrmse"] >= 0.0
        assert fit["limit"] > 0.0


# proves R608.6: Hot-path proof
# spec: MEDULLA-FEDERATED-QUERY-R608.6
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R608.6")
@pytest.mark.asyncio
async def test_bounded_rules_and_causal_verdict() -> None:
    """A small fixture end to end through the served `RunRules` and `CausalVerify`:
    the bounded fixpoint stops early under `RULE_FIXPOINT_BOUND`, the unbounded one
    equals the independent closure, and the causal verdict follows the oracle."""
    await test_both_methods_reach_the_kernels()
