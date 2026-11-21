"""MEDULLA-ORCH-VOCAB-R010.100 served evolution metrics: ``evolution_trace_metrics`` joins an
``EvolutionProgress`` node to its ``RunTrace`` source nodes and the ``ProgramRevision`` of its
prompt, through ``Method::Uql``. A source run id with no trace makes the answer partial; it is
never counted as zero."""

from __future__ import annotations

import hashlib
import json
import struct
import uuid

import pytest

DOMAIN = b"eg/evolution-metric-window/v1"


def _framed(fields: list[bytes]) -> str:
    material = b"eg/framed-sha256/v1\0"
    material += struct.pack(">I", len(DOMAIN)) + DOMAIN
    material += struct.pack(">I", len(fields))
    for field in fields:
        material += struct.pack(">Q", len(field)) + field
    return hashlib.sha256(material).hexdigest()


def _window_digest(
    proposal: tuple[str, int],
    prompt: tuple[str, int],
    prompt_digest: str,
    window: tuple[str, int],
    run_ids: list[str],
) -> str:
    fields = [
        proposal[0].encode(),
        struct.pack(">Q", proposal[1]),
        prompt[0].encode(),
        struct.pack(">Q", prompt[1]),
        bytes.fromhex(prompt_digest),
        window[0].encode(),
        struct.pack(">Q", window[1]),
        *[run_id.encode() for run_id in run_ids],
    ]
    return _framed(fields)


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.100")
def test_missing_trace_is_partial_not_zero(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    proposal = (f"proposal-{suffix}", 2)
    prompt = (f"prompt-{suffix}", 3)
    window = (f"window-{suffix}", 4)
    run_ids = [f"run-a-{suffix}", f"run-b-{suffix}"]
    prompt_digest = hashlib.sha256(suffix.encode()).hexdigest()

    clean_graph.nodes.add(
        f"revision-{suffix}",
        {
            "type": "ProgramRevision",
            "program_ref": prompt[0],
            "revision": prompt[1],
            "content_digest": prompt_digest,
        },
    )
    clean_graph.nodes.add(
        f"progress-{suffix}",
        {
            "node_type": "EvolutionProgress",
            "proposalId": proposal[0],
            "proposalRevision": proposal[1],
            "promptVersionId": prompt[0],
            "promptRevision": prompt[1],
            "metricWindowId": window[0],
            "metricWindowRevision": window[1],
            "sourceRunIds": json.dumps(run_ids),
            "observedAtMs": 1_000,
        },
    )
    # Only the first of the two source traces exists.
    clean_graph.nodes.add(
        run_ids[0], {"node_type": "RunTrace", "status": "completed"}
    )

    params = {
        "proposal_id": proposal[0],
        "proposal_revision": proposal[1],
        "prompt_version_id": prompt[0],
        "prompt_revision": prompt[1],
        "prompt_digest": prompt_digest,
        "metric_window_id": window[0],
        "metric_window_revision": window[1],
        "metric_window_digest": _window_digest(
            proposal, prompt, prompt_digest, window, run_ids
        ),
    }
    result = clean_graph.query.uql("evolution_trace_metrics", params)
    assert result["kind"] == "rows"
    rows = {row["id"]: row["channels"] for row in result["rows"]}
    answer = rows["answer"]
    # One of two source traces was found: half coverage, and the count says so.
    assert answer["value"] == pytest.approx(0.5)
    assert answer["numerator"] == 1
    assert answer["denominator"] == 2
    assert f"trace:{run_ids[1]}" in rows
    assert any(run_ids[1] in warning for warning in result["warnings"])
    # The metrics divide by the found traces only; the missing one counts as nothing.
    assert rows["metric:success_rate"]["denominator"] == 1
    assert rows["metric:success_rate"]["value"] == pytest.approx(1.0)

    # A window revision older than the stored one is stale, not silently served.
    stale = {**params, "metric_window_revision": window[1] - 1}
    with pytest.raises(Exception, match="EVOLUTION_WINDOW_STALE"):
        clean_graph.query.uql("evolution_trace_metrics", stale)
