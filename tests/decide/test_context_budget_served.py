"""MEDULLA-DECISION-ENGINE-R137.6 served over-window plan: a model profile is
published over the wire, then `Decide` plans how a prompt of five windows is
split, reading the window from the profile and sealing the limits as premises.

The served engine admits only its deployment tenant, so the other-tenant read of
a profile is proven by the Rust L3 test
`over_window_plan_tests.rs::cross_tenant_profile_is_not_read`; here the second
case asks for a profile the caller's tenant does not hold and asserts the engine
does not use it."""

from __future__ import annotations

from typing import Any

import pytest
from _decision_resident_fixture import publication, publish
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds

WINDOW = 1_000


def _publish_model_profile(client: Any, component_id: str, window: int) -> None:
    """The served-test kit fixture: one tenant-owned model profile."""
    request = publication(component_id, "model_profile", {})
    request["op"]["request"]["component"]["facts"] = {
        "facts": "model_profile",
        "provider": "provider",
        "model_identity": f"model:{component_id}",
        "context_window_tokens": window,
        "max_output_tokens": 1_024,
        "supports_tools": False,
        "supports_structured_output": False,
        "supports_vision": False,
        "modalities": {"input": [], "output": []},
    }
    client._send("AgentComponent", request)


def _decide(client: Any, schema: dict[str, Any], params: list[dict[str, Any]]):
    request = dc.decide_request(
        TEST_TENANT,
        ("window.plan", "over_window_plan", "ordinary"),
        dc.declared([dc.DeclaredOption("plan", numbers={"signal": 0})]),
        schema,
        params=params,
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


def _plan(record: dict[str, Any]) -> dict[str, str]:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    facts = (score["option_id"].split(":", 1) for score in outcome["scores"])
    return {key: value for key, value in facts}


def _schema(client: Any) -> dict[str, Any]:
    body = ds.feature_schema_body([ds.feature("signal", ds.number("signal"))])
    return publish(client, "window-schema", "feature_schema", body)


# spec: MEDULLA-DECISION-ENGINE-R137.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R137.6")
def test_over_window_plan_end_to_end(clean_graph):
    schema = _schema(clean_graph)
    _publish_model_profile(clean_graph, "profile-small", WINDOW)
    prompt = dc.param("prompt_tokens", "int", 5 * WINDOW)

    # The window comes from the published profile: five windows halve three
    # times into eight chunks, and 2 * 8 - 1 model calls run.
    record = _decide(
        clean_graph,
        schema,
        [prompt, dc.param("model_profile_ref", "iri", "profile-small")],
    )
    plan = _plan(record)
    assert plan["exceeds"] == "true"
    assert plan["chunks"] == "8"
    assert plan["depth"] == "3"
    assert plan["chunk_tokens"] == "625"
    assert int(plan["model_calls"]) == 2 * int(plan["chunks"]) - 1
    facts = {(p["fact"], p["class"]) for p in record["premises"]}
    assert (f"context_budget.window_tokens={WINDOW}", "observation") in facts
    assert any(fact == "context_budget.limit:max_depth=3" for fact, _ in facts)
    assert any(fact.startswith("knob:context.") for fact, _ in facts)

    # A profile the caller's tenant does not hold is never read: the plan is
    # made from the request's own window, sealed as a claim, not an observation.
    record = _decide(
        clean_graph,
        schema,
        [
            prompt,
            dc.param("model_profile_ref", "iri", "profile-of-another-tenant"),
            dc.param("window_tokens", "int", 10 * WINDOW),
        ],
    )
    plan = _plan(record)
    assert plan["exceeds"] == "false"
    assert plan["chunks"] == "1"
    facts = {(p["fact"], p["class"]) for p in record["premises"]}
    assert (f"context_budget.window_tokens={10 * WINDOW}", "claim") in facts
    assert (f"context_budget.window_tokens={WINDOW}", "observation") not in facts
