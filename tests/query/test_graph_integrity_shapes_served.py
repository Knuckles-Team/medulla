"""Graph integrity shapes over the served UQL wire (MEDULLA-FEDERATED-QUERY-R005.700,
MEDULLA-FEDERATED-QUERY-R005.701).

`VALIDATE SHAPE "integrity:<name>[:<key>]"` runs a named integrity shape in one pass over
the queried snapshot. A fatal shape (`invalid_reference`, `dangling_edge`) reports the
violating rows; a repaired graph reports none, and the scan writes nothing.
"""

from __future__ import annotations

import uuid

import pytest


def _violating(clean_graph, label: str, shape: str) -> set[str]:
    result = clean_graph.query.uql(
        f'MATCH (:{label}) |> VALIDATE SHAPE "{shape}" KEEP VIOLATING |> LIMIT 100000'
    )
    return {row["id"] for row in result["rows"]}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R005.700", "MEDULLA-FEDERATED-QUERY-R005.701")
def test_fatal_finding_blocks(clean_graph) -> None:
    tag = uuid.uuid4().hex[:8]
    label = f"IntegrityDoc{tag}"
    good, bad = f"integrity-good-{tag}", f"integrity-bad-{tag}"
    clean_graph.nodes.add(good, {"type": label, "owner": good})
    clean_graph.nodes.add(bad, {"type": label, "owner": f"integrity-missing-{tag}"})
    shape = "integrity:invalid_reference:owner"

    # The fatal shape flags the node whose reference names no node, and only that node.
    assert _violating(clean_graph, label, shape) == {bad}
    # A warn-level shape is independent: both nodes carry the property.
    assert _violating(clean_graph, label, "integrity:required_property:owner") == set()
    missing = f"integrity-noowner-{tag}"
    clean_graph.nodes.add(missing, {"type": label})
    assert _violating(clean_graph, label, "integrity:required_property:owner") == {missing}

    # The conforming side of the same shape keeps the clean rows.
    conforming = clean_graph.query.uql(
        f'MATCH (:{label}) |> VALIDATE SHAPE "{shape}" KEEP CONFORMING |> LIMIT 100000'
    )
    assert {row["id"] for row in conforming["rows"]} == {good, missing}

    # The repaired control (the reference now names a node) has no finding, and the scan
    # changed no node: the same three nodes remain.
    clean_graph.nodes.add(f"integrity-missing-{tag}", {"type": "Target"})
    assert _violating(clean_graph, label, shape) == set()
    everything = clean_graph.query.uql(f"MATCH (:{label}) |> LIMIT 100000")
    assert {row["id"] for row in everything["rows"]} == {good, bad, missing}

    # MEDULLA-FEDERATED-QUERY-R005.701: the opt-in `FAIL ON FATAL` refusal. Restore the
    # dangling reference first: the fatal shape has a finding again.
    clean_graph.nodes.add(bad, {"type": label, "owner": f"integrity-gone-{tag}"})
    statement = f'MATCH (:{label}) |> VALIDATE SHAPE "{shape}" KEEP VIOLATING'
    # Without the clause the blocking finding is data on the row, never a refusal.
    plain = clean_graph.query.uql(statement + " |> LIMIT 100000")
    assert {row["id"] for row in plain["rows"]} == {bad}
    assert plain["rows"][0]["provenance"]["shape.blocks"] == "true"
    # With the clause a blocking finding refuses and names the code and the shape.
    with pytest.raises(Exception) as refused:
        clean_graph.query.uql(statement + " FAIL ON FATAL |> LIMIT 100000")
    assert "SHAPE_FATAL_FINDING" in str(refused.value)
    assert "invalid_reference" in str(refused.value)
    # A warn shape is not fatal: the clause does not refuse and returns the row.
    warned = clean_graph.query.uql(
        f'MATCH (:{label}) |> VALIDATE SHAPE "integrity:required_property:owner" '
        "KEEP VIOLATING FAIL ON FATAL |> LIMIT 100000"
    )
    assert {row["id"] for row in warned["rows"]} == {missing}
    assert warned["rows"][0]["provenance"]["shape.blocks"] == "false"
