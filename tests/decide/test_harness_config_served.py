"""MEDULLA-DECISION-ENGINE-R128.15 served proof of the ``harness_config`` journey.

The entry point is the served ``Decide`` method with the ``harness_config``
question kind: the caller names a model profile, a task class, the usable
context budget, the priority, the capabilities the tool subset must cover and
the graph that holds the task-class facts, and declares its tool candidates.
The engine reads the model profile's measured facts from the Agent Library and
the task-class facts from the graph, decides the three parts (context policy
with its thresholds, plan scaffold, action space with its tool subset) and
seals them in one record whose evidence names every premise and the exact
tool-subset result. ``Decide`` is evaluate-only: nothing is written.
"""

from __future__ import annotations

import os
from typing import Any

import msgpack
import pytest
from _decision_resident_fixture import DIGEST, publication, publish, schema_body
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import SyncMedullaClient

SWARM = "https://knuckles-team.github.io/ontology/swarm#"
PROFILE = "model-profile-r128"
TASK = "urn:task:coding"
TOOLS = [
    ("tool-a", ["eg:cap/a"], 100),
    ("tool-b", ["eg:cap/b"], 50),
    ("tool-c", ["eg:cap/a", "eg:cap/c"], 400),
]


def _measured(value: str, run: str) -> dict[str, Any]:
    return {
        "value": value,
        "evaluation_run_ref": {"evaluation_id": run, "digest": f"sha256:{run}"},
    }


def _publish_model_profile(
    client: Any, component_id: str, shell: str | None, stopping: str | None
) -> None:
    """One ``model_profile`` component carrying measured facts."""
    request = publication(component_id, "model_profile", schema_body())
    component = request["op"]["request"]["component"]
    component["content_digest"] = DIGEST
    component["attributes"] = {}
    component["facts"] = {
        "facts": "model_profile",
        "provider": "fixture",
        "model_identity": "fixture/model",
        "context_window_tokens": 8192,
        "max_output_tokens": 1024,
        "supports_tools": True,
        "supports_structured_output": True,
        "supports_vision": False,
        "shell_proficiency": None if shell is None else _measured(shell, "eval:shell"),
        "stopping_behaviour": None
        if stopping is None
        else _measured(stopping, "eval:stopping"),
    }
    client._send("AgentComponent", request)


def _seed_graph(client: Any, key: str, nodes: list[tuple[str, dict[str, Any]]]) -> None:
    """Write the task-class and profile-node facts through the generated
    client write, the same entry point as the contrast served tests."""
    envelope = _envelope(
        _graph_name(client),
        key,
        key,
        1,
        [
            op(
                ordinal,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
            for ordinal, (node_id, props) in enumerate(nodes)
        ],
    )
    results = client.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results


def _request(schema: dict[str, Any], profile: str, graph: str | None) -> dict[str, Any]:
    options = [
        dc.DeclaredOption(
            name,
            numbers={"schema_tokens": tokens},
            classification=tuple(capabilities),
        )
        for name, capabilities, tokens in TOOLS
    ]
    params = [
        dc.param("model_profile_ref", "iri", profile),
        dc.param("task_class", "iri", TASK),
        dc.param("usable_context_budget_tokens", "int", 1000),
        dc.param("priority", "text", "interactive"),
        dc.param("expected_trajectory_tokens", "int", 4096),
        dc.param("required_capabilities", "iri_list", ["eg:cap/a", "eg:cap/b"]),
    ]
    if graph is not None:
        params.append(dc.param("graph", "text", graph))
    return dc.decide_request(
        TEST_TENANT,
        ("q:r128:config", "harness_config", "ordinary"),
        dc.declared(options),
        schema,
        params=params,
    )


def _evidence(record: dict[str, Any]) -> dict[str, Any]:
    return record["inputs"]["harness_evaluation"]


def _premise_facts(record: dict[str, Any]) -> list[str]:
    return [premise["fact"] for premise in record["premises"]]


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R128.15")
def test_harness_config_end_to_end(clean_graph):
    client = clean_graph
    graph = _graph_name(client)
    _seed_graph(
        client,
        "r128-15:facts",
        [
            (
                TASK,
                {f"{SWARM}shellCentric": False, f"{SWARM}taskTokenUpperBound": 10000},
            ),
            (
                PROFILE,
                {
                    f"{SWARM}planScaffoldNoGainAblation": True,
                    f"{SWARM}planScaffoldNoGainAblationRun": "eval:ablation",
                    f"{SWARM}planScaffoldNoGainAblationRunDigest": (
                        "sha256:eval:ablation"
                    ),
                },
            ),
        ],
    )
    _publish_model_profile(client, PROFILE, "high", "balanced")
    schema = publish(client, "r128-15-schema", "feature_schema", schema_body())

    # Case 1: a profile with measured facts decides the three parts; the sealed
    # record carries the premises and the exact tool-subset result.
    record = client._send("Decide", {"request": _request(schema, PROFILE, graph)})[
        "records"
    ][0]
    assert record["question"]["kind"] == "harness_config"
    answer = _evidence(record)["answer"]
    assert answer["action_space"] == {"decision": "decided", "value": "tool_set"}
    assert answer["tool_subset"]["decision"] == "decided"
    assert sorted(answer["tool_subset"]["value"]) == ["tool-a", "tool-b"]
    assert _evidence(record)["tool_subset_certificate"]["model_digest"]
    facts = _premise_facts(record)
    for name in (
        "harness_config.context_window_tokens=",
        "harness_config.shell_proficiency=",
        "harness_config.shell_centric=",
        "harness_config.stopping_behaviour=",
        "harness_config.plan_scaffold_ablation=",
    ):
        assert any(fact.startswith(name) for fact in facts), (name, facts)
    assert any(
        fact.startswith("harness_config.why_not:tool_subset:tool-c") for fact in facts
    ), facts

    # Case 2: a profile without ``shellProficiency`` abstains the action space
    # with the conservative default ``tool_set``.
    unmeasured = "model-profile-r128-unmeasured"
    _publish_model_profile(client, unmeasured, None, "balanced")
    record = client._send("Decide", {"request": _request(schema, unmeasured, graph)})[
        "records"
    ][0]
    action_space = _evidence(record)["answer"]["action_space"]
    assert action_space["decision"] == "abstained"
    assert "shellProficiency" in action_space["missing_fact"]
    assert action_space["conservative_default"] == "tool_set"

    # Case 3: a second tenant cannot see the first tenant's profile.
    other = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context={**request_context(), "tenant": "tenant:other"},
    )
    try:
        foreign = _request(schema, PROFILE, None)
        foreign["tenant_id"] = "tenant:other"
        with pytest.raises(Exception, match="HARNESS_PROFILE_UNKNOWN"):
            other._send("Decide", {"request": foreign})
    finally:
        other.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R656.4")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R656.6")
def test_profile_constrains_a_run(clean_graph):
    """A saved assistant profile constrains a served ``Decide`` run: the
    allowlist removes tools and the profile enters the sealed record as a
    ``profile:<id>@<revision>`` premise. A profile the caller cannot read (here
    another tenant's view of it) is refused with ``PROFILE_NOT_VISIBLE``."""
    client = clean_graph
    graph = _graph_name(client)
    _seed_graph(
        client,
        "r656-4:facts",
        [
            (
                TASK,
                {f"{SWARM}shellCentric": False, f"{SWARM}taskTokenUpperBound": 10000},
            ),
            (PROFILE, {}),
        ],
    )
    _publish_model_profile(client, PROFILE, "high", "balanced")
    schema = publish(client, "r656-4-schema", "feature_schema", schema_body())
    saved = client._send(
        "AssistantProfile",
        {
            "op": dc.put_profile_op(
                dc.assistant_profile_wire(
                    profile_id="profile-r656-4",
                    name="Narrow",
                    owner_principal=TEST_AGENT_ID,
                    tool_allowlist=["tool-b", "tool-c"],
                )
            )
        },
    )
    revision = saved["revision"]

    request = _request(schema, PROFILE, graph)
    request["params"] = sorted(
        [
            *request["params"],
            dc.param("assistant_profile_ref", "iri", "profile-r656-4"),
        ],
        key=lambda p: str(p["name"]),
    )
    record = client._send("Decide", {"request": request})["records"][0]
    answer = _evidence(record)["answer"]
    assert answer["tool_subset"]["decision"] == "decided"
    assert sorted(answer["tool_subset"]["value"]) == ["tool-b", "tool-c"]
    facts = _premise_facts(record)
    assert f"profile:profile-r656-4@{revision}" in facts, facts

    # Another tenant cannot read the profile: one non-disclosing refusal.
    other = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context={**request_context(), "tenant": "tenant:other"},
    )
    try:
        foreign = dict(request)
        foreign["tenant_id"] = "tenant:other"
        with pytest.raises(Exception, match="PROFILE_NOT_VISIBLE"):
            other._send("Decide", {"request": foreign})
    finally:
        other.close()
