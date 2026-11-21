"""Served `DataPrepDeclare` and `DataPrepDeclarationRead`.

MEDULLA-INGEST-WORKER-R001.1007 (replace), R001.1008 (wiring through the Python
client) and R001.1009 (refusals) over the session engine started by the
`start_medulla_server` fixture of `tests/conftest.py`.
"""

from __future__ import annotations

import hashlib

import pytest
from conftest import TEST_TENANT

from medulla.client import EngineResponseError, SyncMedullaClient
from medulla.generated import models

CAPABILITY = "text.normalize-v1"
POLICY = hashlib.sha256(b"policy").hexdigest()


def _payload(fields: list[str], capability: str = CAPABILITY, **extra) -> dict:
    return {
        "capability": capability,
        "field_refs": fields,
        "policy_digest": POLICY,
        **extra,
    }


def _declare(client: SyncMedullaClient, payload: dict, tenant: str = TEST_TENANT):
    params = models.MethodDataPrepDeclareParams(
        request=models.DataPrepDeclareRequest(tenant=tenant, payload=payload)
    )
    result = client._send("DataPrepDeclare", params.model_dump(mode="json"))
    return models.DataPrepDeclareReceipt.model_validate(result)


def _read(client: SyncMedullaClient, capability: str = CAPABILITY):
    params = models.MethodDataPrepDeclarationReadParams(
        request=models.DataPrepDeclarationReadRequest(
            tenant=TEST_TENANT, capability=capability
        )
    )
    result = client._send("DataPrepDeclarationRead", params.model_dump(mode="json"))
    return models.DataPrepDeclarationView.model_validate(result)


def _counts(client: SyncMedullaClient) -> tuple[int, int]:
    return client.nodes.count(), client._send("EdgeCount")


def _fields(client: SyncMedullaClient, capability: str = CAPABILITY) -> set[str]:
    found = _read(client, capability).declaration
    return set(found.field_refs) if found else set()


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.1007")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.1008")
def test_declare_replace_and_refuse(clean_graph):
    first = _declare(clean_graph, _payload(["a.field", "b.field"]))
    assert (first.written, first.unchanged) == (1, 0)
    assert _fields(clean_graph) == {"a.field", "b.field"}
    after_first = _counts(clean_graph)

    # An equal declaration, in another field order, writes nothing.
    again = _declare(clean_graph, _payload(["b.field", "a.field"]))
    assert (again.written, again.unchanged) == (0, 1)
    assert again.content_digest == first.content_digest
    assert _counts(clean_graph) == after_first

    # A changed declaration replaces the edge set in one commit.
    changed = _declare(clean_graph, _payload(["b.field", "c.field"]))
    assert (changed.written, changed.unchanged) == (1, 0)
    assert changed.content_digest != first.content_digest
    assert _fields(clean_graph) == {"b.field", "c.field"}

    refused = _declare(clean_graph, _payload(["usr/bin/sh"]))
    assert (refused.written, refused.unchanged, refused.refused) == (0, 0, 1)
    assert refused.refusal_code == "DATA_PREP_DECLARATION_INVALID"
    assert _fields(clean_graph) == {"b.field", "c.field"}


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.1008")
def test_declare_then_read_through_the_python_client(clean_graph):
    assert _read(clean_graph).declaration is None
    receipt = _declare(clean_graph, _payload(["a.field", "b.field"]))
    view = _read(clean_graph)
    assert view.content_digest == receipt.content_digest
    declaration = view.declaration
    assert declaration is not None
    assert declaration.capability == CAPABILITY
    assert sorted(declaration.field_refs) == ["a.field", "b.field"]
    assert declaration.policy_digest == POLICY
    # A declaration of another tenant is refused, never read.
    with pytest.raises(EngineResponseError, match="INGEST_TENANT_MISMATCH"):
        _declare(clean_graph, _payload(["a.field"]), tenant="tenant:other")
    with pytest.raises(EngineResponseError, match="DATA_PREP_DECLARATION_INVALID"):
        _read(clean_graph, capability="Bad:Capability")
    assert _read(clean_graph, capability="other.capability").declaration is None


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.1009")
def test_declare_refusals_change_nothing(clean_graph):
    _declare(clean_graph, _payload(["a.field"]))
    before = _counts(clean_graph)
    digest = _read(clean_graph).content_digest

    invalid = [
        _payload(["a:b"]),
        _payload(["a/b"]),
        _payload(["has space"]),
        _payload([""]),
        _payload(["a.field"], capability="mod:fn"),
        _payload(["a.field"], capability="Upper"),
        _payload(["a.field"], command="/bin/sh"),
        _payload(["a.field"], **{"import": "os"}),
    ]
    for payload in invalid:
        refused = _declare(clean_graph, payload)
        assert refused.refused == 1
        assert refused.refusal_code == "DATA_PREP_DECLARATION_INVALID"
        assert refused.content_digest is None
    too_many = [f"f{i}" for i in range(257)]
    refused = _declare(clean_graph, _payload(too_many))
    assert refused.refused == 1
    assert refused.refusal_code == "DATA_PREP_DECLARATION_TOO_LARGE"

    assert _counts(clean_graph) == before
    assert _read(clean_graph).content_digest == digest
    assert _fields(clean_graph) == {"a.field"}


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.1010")
def test_report_counts_written_unchanged_refused(clean_graph):
    written = _declare(clean_graph, _payload(["a.field"]))
    assert (written.written, written.unchanged, written.refused) == (1, 0, 0)
    unchanged = _declare(clean_graph, _payload(["a.field"]))
    assert (unchanged.written, unchanged.unchanged, unchanged.refused) == (0, 1, 0)
    refused = _declare(clean_graph, _payload(["a:b"]))
    assert (refused.written, refused.unchanged, refused.refused) == (0, 0, 1)
    assert refused.refusal_code == "DATA_PREP_DECLARATION_INVALID"
    # A refusal is a report, not a call error; a foreign tenant is still an error.
    with pytest.raises(EngineResponseError, match="INGEST_TENANT_MISMATCH"):
        _declare(clean_graph, _payload(["a.field"]), tenant="tenant:other")
    assert _fields(clean_graph) == {"a.field"}
