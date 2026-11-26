"""A run records the workflow its work item row names (MEDULLA-TYPED-PACKS-R108.6).

L3 over the authenticated native wire. A work item submitted for a registered
workflow records that identity on its row. The terminal commit stamps `workflow_id`,
`workflow_version` and `workflow_content_digest` on the run trace it writes, from the
row and never from the trace the caller sent, and the catalog run asset links to the
definition. An unregistered identity refuses at submit and records no row.
"""

from __future__ import annotations

import time
import uuid
from typing import Any

import msgpack
import pytest
from _sparql_solutions import solution_values
from conftest import TEST_AGENT_ID, TEST_AUDIENCE, TEST_POLICY_VERSION, TEST_TENANT
from decide.test_run_work_item_served import authenticated_plan_session
from decide.test_runtime_quota_served import _claim, _extension, _traces

pytestmark = [pytest.mark.no_engine]

CATALOG = "https://knuckles-team.github.io/ontology/catalog#"
PROV_USED = "http://www.w3.org/ns/prov#used"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
IDENTITY_KEYS = ("workflow_id", "workflow_version", "workflow_content_digest")


@pytest.fixture
def authenticated_plan_client(tmp_path):
    yield from authenticated_plan_session(tmp_path)


def _identity() -> dict[str, str]:
    return {
        "workflow_id": f"wf-run-{uuid.uuid4().hex[:8]}",
        "version": "1.0.0",
        "content_digest": "sha256:" + "b" * 64,
    }


def _register(client: Any, identity: dict[str, str]) -> None:
    client._send(
        "WorkflowRegistry",
        {
            "op": {
                "op": "register",
                "registration": {
                    "identity": identity,
                    "owner_scope": "",
                    "lifecycle": "active",
                    "steps": [],
                },
            }
        },
    )


def _submit(
    client: Any,
    work_item_id: str,
    run_id: str,
    now_ms: int,
    workflow: dict[str, str] | None,
) -> dict[str, Any]:
    digests = {name: char * 64 for name, char in (("p", "d"), ("c", "c"), ("m", "e"))}
    request: dict[str, Any] = {
        "schema_version": "1",
        "context": {
            "schema_version": "2",
            "request_id": uuid.uuid4().hex,
            "subject_id": TEST_AGENT_ID,
            "tenant_id": TEST_TENANT,
            "agent_id": TEST_AGENT_ID,
            "scopes": [],
            "audience": TEST_AUDIENCE,
            "authentication_method": "local_process",
            "policy_version": TEST_POLICY_VERSION,
            "graph": "__commons__",
            "placement_epoch": None,
            "trace_id": uuid.uuid4().hex,
            "issued_at_ms": now_ms,
            "expires_at_ms": now_ms + 60_000,
        },
        "work_item_id": work_item_id,
        "idempotency_key": f"submit-{work_item_id}",
        "command_digest": uuid.uuid4().hex * 2,
        "kind": "agent.execute",
        "priority": 0,
        "depends_on": [],
        "input_ref": "input:workflow-run",
        "policy_digest": digests["p"],
        "catalog_digest": digests["c"],
        "model_digest": digests["m"],
        "max_attempts": 1,
        "deadline_unix": None,
        "metadata": {
            "delegation_id": "delegation:" + run_id,
            "run_id": run_id,
            "agent_id": "agent:selected-b",
            "capability_digest": "b" * 64,
        },
        "provenance_refs": [],
        "max_tenant_in_flight": 8,
    }
    if workflow is not None:
        request["workflow"] = workflow
    return client.work_items.submit(request)


def _commit_run(
    client: Any, item: str, run_id: str, now_ms: int, forged: bool
) -> dict[str, Any]:
    """Claim and commit one item; the sent trace may claim an identity of its own."""
    claim = _claim(client, item, now_ms + 1)
    fence = claim["fencing_token"]
    extension = _extension(client, item, run_id, "trace:" + run_id, fence)
    if forged:
        node = extension["receipt_nodes"][0]
        properties = msgpack.unpackb(node["properties_msgpack"], raw=False)
        properties.update(
            workflow_id="forged",
            workflow_version="9",
            workflow_content_digest="sha256:" + "f" * 64,
        )
        node["properties_msgpack"] = msgpack.packb(properties, use_bin_type=True)
    committed = client.work_items.commit_result(
        tenant=TEST_TENANT,
        work_item_id=item,
        worker_id=TEST_AGENT_ID,
        lease_epoch=claim["lease_epoch"],
        fencing_token=fence,
        idempotency_key=f"commit-{item}",
        outcome="succeeded",
        result_ref="cas:result:" + run_id,
        outcome_extension=extension,
        now_ms=int(time.time() * 1000),
    )
    assert committed["outcome"] == "succeeded", committed
    return committed


# spec: MEDULLA-TYPED-PACKS-R108.6
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.6")
def test_commit_stamps_the_row_workflow_and_links_the_run_asset(
    authenticated_plan_client,
):
    client = authenticated_plan_client
    try:
        identity = _identity()
        now_ms = int(time.time() * 1000)

        # An identity the registry does not hold refuses at submit and records no row.
        refused_item = "workitem:wf:" + uuid.uuid4().hex
        with pytest.raises(Exception, match="WORKFLOW_IDENTITY_UNKNOWN"):
            _submit(client, refused_item, "run-" + uuid.uuid4().hex, now_ms, identity)
        assert (
            client.work_items.get(tenant=TEST_TENANT, work_item_id=refused_item) is None
        )

        # A registered identity is recorded on the row at submit.
        _register(client, identity)
        run_id = "run-" + uuid.uuid4().hex
        item = "workitem:wf:" + uuid.uuid4().hex
        _submit(client, item, run_id, now_ms, identity)
        row = client.work_items.get(tenant=TEST_TENANT, work_item_id=item)
        assert row is not None and row["workflow"] == identity

        # The terminal commit stamps the three properties on the run trace from the row,
        # even though the trace the caller sent claimed another identity.
        _commit_run(client, item, run_id, now_ms, forged=True)
        trace = _traces(client)["trace:" + run_id]
        assert trace["node_type"] == "RunTrace"
        assert (
            trace["workflow_id"],
            trace["workflow_version"],
            trace["workflow_content_digest"],
        ) == (identity["workflow_id"], identity["version"], identity["content_digest"])

        # The run asset links to the registered definition.
        workflow = "workflow:{}:{}:{}".format(
            identity["workflow_id"], identity["version"], identity["content_digest"]
        )
        definition = "urn:medulla:asset:workflow:" + workflow.replace(":", "%3A")
        rows = solution_values(
            client.rdf.sparql(
                f"SELECT ?run ?def WHERE {{ ?run <{RDF_TYPE}> <{CATALOG}WorkflowRun> . "
                f"?run <{PROV_USED}> ?def }}"
            )
        )
        linked = {row["def"].strip("<>") for row in rows}
        assert definition in linked

        # An item of no workflow keeps none of the identity its trace claimed.
        plain_run = "run-" + uuid.uuid4().hex
        plain = "workitem:wf:" + uuid.uuid4().hex
        _submit(client, plain, plain_run, now_ms, None)
        _commit_run(client, plain, plain_run, now_ms, forged=True)
        stamped = _traces(client)["trace:" + plain_run]
        assert not any(key in stamped for key in IDENTITY_KEYS)
    finally:
        client.close()
