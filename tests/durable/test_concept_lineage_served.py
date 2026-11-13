"""Served proof for MEDULLA-DURABLE-KERNEL-R094.7 and R094.7.1.

Concept ids and their `superseded_by` edges are served by `ConceptLineage`
over the real native engine: claimed ids and edges survive an engine restart,
a path claimed by another repository is refused, and an edge that is a self
edge, names an unknown id or closes a cycle is refused by its registered code.
"""

import uuid

import msgpack
import pytest

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its real isolated process.
]

TENANT = "tenant-shared"
PRINCIPAL = "principal:sha256:" + "a" * 64
PLACEHOLDER = "bound-by-engine"


def _raw(value):
    """A raw method result travels as the MessagePack of its value."""
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _context(key):
    return {
        "request_id": 0,
        "principal": PRINCIPAL,
        "caller_principal": PRINCIPAL,
        "attempt_nonce": uuid.uuid4().hex * 2,
        "tenant_id": TENANT,
        "actor_scope": PLACEHOLDER,
        "purpose_id": PLACEHOLDER,
        "policy_revision": PLACEHOLDER,
        "policy_digest": "sha256:" + "0" * 64,
        "policy_decision_id": PLACEHOLDER,
        "idempotency_key": key,
        "expected_revision": None,
        "trace_id": None,
        "created_at_ms": 0,
    }


def _claim(rpc, sock, key, concept_id, repository):
    return rpc(
        sock,
        "ConceptLineage",
        {
            "op": {
                "op": "claim",
                "context": _context(key),
                "concept_id": concept_id,
                "repository": repository,
            }
        },
        key=key,
    )


def _edge(rpc, sock, key, superseded, successor):
    return rpc(
        sock,
        "ConceptLineage",
        {
            "op": {
                "op": "add_edge",
                "context": _context(key),
                "superseded": superseded,
                "successor": successor,
                "repository": "medulla",
            }
        },
        key=key,
    )


def _read(rpc, ok, sock, concept_id):
    return _raw(
        ok(
            rpc(
                sock,
                "ConceptLineage",
                {
                    "op": {
                        "op": "read",
                        "tenant_id": TENANT,
                        "concept_id": concept_id,
                    }
                },
            )
        )
    )


def _code(response):
    error = response.get("error")
    assert error, f"expected a refusal, got {response}"
    return str(error).split(":", 1)[0].strip()


def test_known_ids_and_edges_survive_restart(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R094.7.1.

    Claims two ids, adds an edge, restarts the server and reads both back.
    """
    from test_native_no_auth_served import ok, rpc, server

    older, newer = "MEDULLA-KG.memory", "MEDULLA-KG.recall"
    with server(tmp_path) as sock:
        ok(_claim(rpc, sock, "claim-older", older, "medulla"))
        ok(_claim(rpc, sock, "claim-newer", newer, "medulla"))
        ok(_edge(rpc, sock, "edge-1", older, newer))
        collision = _claim(rpc, sock, "claim-collision", older, "meristem")
        assert _code(collision) == "CONCEPT_ID_PATH_COLLISION"

    with server(tmp_path) as sock:
        seen = _read(rpc, ok, sock, older)
        assert seen["concept"]["concept_id"] == older
        assert seen["concept"]["repository"] == "medulla"
        assert [edge["successor"] for edge in seen["successors"]] == [newer]
        assert _read(rpc, ok, sock, newer)["concept"]["concept_id"] == newer
        collision = _claim(rpc, sock, "claim-collision-2", older, "meristem")
        assert _code(collision) == "CONCEPT_ID_PATH_COLLISION"


def test_lineage_cycle_is_refused(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R094.7.

    Adds edges through the served method and asserts the three codes.
    """
    from test_native_no_auth_served import ok, rpc, server

    ids = ["MEDULLA-KG.one", "MEDULLA-KG.two", "MEDULLA-KG.three"]
    with server(tmp_path) as sock:
        for number, concept_id in enumerate(ids):
            ok(_claim(rpc, sock, f"claim-{number}", concept_id, "medulla"))
        ok(_edge(rpc, sock, "edge-12", ids[0], ids[1]))
        ok(_edge(rpc, sock, "edge-23", ids[1], ids[2]))

        cycle = _edge(rpc, sock, "edge-31", ids[2], ids[0])
        assert _code(cycle) == "CONCEPT_LINEAGE_CYCLE"
        selfie = _edge(rpc, sock, "edge-11", ids[0], ids[0])
        assert _code(selfie) == "CONCEPT_LINEAGE_SELF_EDGE"
        unknown = _edge(rpc, sock, "edge-1x", ids[0], "MEDULLA-KG.ghost")
        assert _code(unknown) == "CONCEPT_LINEAGE_UNKNOWN_ID"
        assert _read(rpc, ok, sock, ids[2])["successors"] == []
