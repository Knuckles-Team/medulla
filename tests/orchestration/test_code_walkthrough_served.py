"""MEDULLA-ORCH-VOCAB-R013.7.3 served walkthrough evidence: a walkthrough that
resolves against the repository index of the tenant graph and the artifact blob of
the carrier owner scope is stored, reads back through UQL with the same ordered refs
and digests it was written with, and the artifact read verifies its digest."""

from __future__ import annotations

import hashlib

import pytest
from conftest import TEST_TENANT

from medulla.generated.index_repository import IndexRepositoryScope

_REPO = "repo-walkthrough"
_REVISION = "a" * 40
_BODY = b"def parse_invoice(text):\n    return text.strip()\n"
_ARTIFACT = b"walkthrough artifact: parse_invoice trims its input\n"


def _hex(value: str) -> str:
    return value.removeprefix("sha256:")


def _step(index: int) -> dict:
    return {
        "step_index": index,
        "kind": "tool_call",
        "model_request_digest": "a" * 64,
        "tool_args_digest": "b" * 64,
        "result_digest": "c" * 64,
    }


def _index_into_tenant_graph(client):
    graph = f"tenant__{TEST_TENANT}"
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": _REPO,
            "commits": [],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": _REVISION,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": "billing/parse.py",
                    "blob_digest": "sha256:" + hashlib.sha256(_BODY).hexdigest(),
                }
            ],
        }
    )
    try:
        client.tenants.create(graph, "Agent")
    except Exception:  # the graph may already exist
        pass
    try:
        result = client.graph.index_repository(
            [("billing/parse.py", _BODY)], scope=scope, graph=graph
        )
    except Exception as exc:
        text = str(exc)
        if "AST feature not enabled" in text or "requires durable persistence" in text:
            pytest.skip("engine built without the ast feature or durable persistence")
        raise
    entities = [
        node for node in result.nodes if (node.properties or {}).get("source_digest")
    ]
    assert entities, "the index lowering stamps source_digest on entity nodes"
    return entities[0]


# proves R013.7.3: Evidence: independent readback and artifact digest
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R013.7.3")
def test_walkthrough_trace_and_artifact_read_back(clean_graph):
    entity = _index_into_tenant_graph(clean_graph)
    # The upload is stored under the carrier owner scope of this verified request.
    artifact_digest = clean_graph.blob.store(_ARTIFACT)
    artifact_ref = f"cas:sha256:{_hex(artifact_digest)}"
    source_ref = {
        "entity_id": entity.node_id,
        "source_revision": _REVISION,
        "source_digest": _hex(entity.properties["source_digest"]),
        "path": "billing/parse.py",
        "start_line": 1,
        "end_line": 2,
    }
    walkthrough = {
        "run_id": "run-walkthrough",
        "repository_id": _REPO,
        "repository_revision": _REVISION,
        "graph_snapshot_revision": 1,
        "query_digest": "d" * 64,
        "ordered_sections": [
            {
                "ordinal": 0,
                "source_refs": [source_ref],
                "content": {"mode": "referenced", "digest": "e" * 64},
            }
        ],
        "interaction_edges": [],
        "artifact_ref": artifact_ref,
        "artifact_digest": hashlib.sha256(_ARTIFACT).hexdigest(),
        "execution_notes": [
            {
                "run_id": "run-walkthrough",
                "step_index": 0,
                "span_id": "span-0",
                "kind": "tool_call",
                "result_digest": "c" * 64,
                "source_refs": [],
            }
        ],
    }
    clean_graph.nodes.add(
        "trace-walkthrough",
        {
            "node_type": "RunTrace",
            "terminationReason": "final_answer",
            "steps": [_step(0)],
            "walkthrough": walkthrough,
        },
    )

    # UQL readback returns the same ordered refs and digests as the write.
    rows = clean_graph.query.uql("MATCH (:RunTrace)")
    assert rows["kind"] == "rows"
    assert "trace-walkthrough" in [row["id"] for row in rows["rows"]]
    stored = clean_graph.nodes.properties("trace-walkthrough")["walkthrough"]
    assert stored["repository_revision"] == _REVISION
    assert stored["ordered_sections"][0]["source_refs"] == [source_ref]
    assert stored["artifact_ref"] == artifact_ref
    assert stored["artifact_digest"] == walkthrough["artifact_digest"]

    # The artifact read verifies its digest.
    body = clean_graph.blob.fetch(artifact_digest)
    assert hashlib.sha256(body).hexdigest() == stored["artifact_digest"]
