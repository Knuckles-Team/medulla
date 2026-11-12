"""MEDULLA-ORCH-VOCAB-R013.3 / R013.4 served step records: a valid ``RunTrace``
stores its steps in index order, and a malformed digest or a skipped/repeated
index refuses with its own code and stores nothing."""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError


def _step(index: int, digest: str = "b" * 64) -> dict:
    return {
        "step_index": index,
        "kind": "tool_call",
        "model_request_digest": "a" * 64,
        "tool_args_digest": digest,
        "result_digest": "c" * 64,
    }


def _trace(steps: list[dict]) -> dict:
    return {
        "node_type": "RunTrace",
        "terminationReason": "final_answer",
        "steps": steps,
    }


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R013.3")
def test_valid_trace_stores_steps_in_order(clean_graph):
    steps = [_step(0), _step(1, "d" * 64), _step(2, "e" * 64)]
    clean_graph.nodes.add("trace-steps-valid", _trace(steps))
    stored = clean_graph.nodes.properties("trace-steps-valid")
    assert [step["step_index"] for step in stored["steps"]] == [0, 1, 2]
    assert stored["steps"] == steps


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R013.4")
@pytest.mark.parametrize(
    ("node_id", "steps", "code"),
    [
        ("trace-bad-digest", [_step(0, "z" * 64)], "STEP_DIGEST_INVALID"),
        ("trace-short-digest", [_step(0, "b" * 63)], "STEP_DIGEST_INVALID"),
        ("trace-gap", [_step(0), _step(2)], "STEP_ORDER_INVALID"),
        ("trace-repeat", [_step(0), _step(0)], "STEP_ORDER_INVALID"),
    ],
)
def test_each_fault_returns_its_code_and_stores_nothing(
    clean_graph, node_id, steps, code
):
    with pytest.raises(EngineResponseError, match=code):
        clean_graph.nodes.add(node_id, _trace(steps))
    assert clean_graph.nodes.has(node_id) is False


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R013.5")
def test_stored_steps_read_in_order_with_actor(clean_graph):
    steps = [_step(0), _step(1, "d" * 64), _step(2, "e" * 64)]
    # A client-supplied actor is never trusted: the engine stamps the verified
    # principal of the request on every step.
    steps[1]["recordedBy"] = "mallory"
    clean_graph.nodes.add("trace-steps-actor", _trace(steps))
    stored = clean_graph.nodes.properties("trace-steps-actor")["steps"]
    assert [step["step_index"] for step in stored] == [0, 1, 2]
    actors = {step["recordedBy"] for step in stored}
    assert len(actors) == 1
    (actor,) = actors
    assert actor and actor != "mallory"
