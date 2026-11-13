"""Runtime quota and run trace over the authenticated native wire (R133.6)."""

import time
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_AGENT_ID, TEST_AUDIENCE, TEST_POLICY_VERSION, TEST_TENANT

from decide.test_run_work_item_served import authenticated_plan_session
from medulla import decision_client as dc
from medulla.client import WorkItemClient

pytestmark = [
    pytest.mark.no_engine,
    pytest.mark.spec("MEDULLA-DECISION-ENGINE-R133.6"),
]

DELEGATOR = TEST_AGENT_ID
SELECTED = "agent:selected-b"
DIGESTS = {
    name: char * 64
    for name, char in (
        ("capability", "b"),
        ("catalog", "c"),
        ("policy", "d"),
        ("model", "e"),
    )
}


@pytest.fixture
def authenticated_plan_client(tmp_path):
    yield from authenticated_plan_session(tmp_path)


def _quota_options(client: Any, params: list[dict[str, Any]]) -> list[str]:
    schema = publish(
        client, "quota-schema-" + uuid.uuid4().hex, "feature_schema", schema_body()
    )
    request = dc.decide_request(
        TEST_TENANT,
        ("quota.test", "runtime_quota", "ordinary"),
        dc.declared([dc.DeclaredOption("quota")]),
        schema,
        params=params,
    )
    request["belief_as_of"] = []
    record = client._send("Decide", {"request": request})["records"][0]
    assert record["outcome"]["outcome"] == "advisory", record["outcome"]
    return [score["option_id"] for score in record["outcome"]["scores"]]


def _submit(client: Any, work_item_id: str, run_id: str, now_ms: int) -> dict[str, Any]:
    return client.work_items.submit(
        {
            "schema_version": "1",
            "context": {
                "schema_version": "2",
                "request_id": uuid.uuid4().hex,
                "subject_id": TEST_AGENT_ID,
                "tenant_id": TEST_TENANT,
                "agent_id": DELEGATOR,
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
            "kind": "delegated_run",
            "priority": 0,
            "depends_on": [],
            "input_ref": "input:quota-trace",
            "policy_digest": DIGESTS["policy"],
            "catalog_digest": DIGESTS["catalog"],
            "model_digest": DIGESTS["model"],
            "max_attempts": 1,
            "deadline_unix": None,
            "metadata": {
                "delegation_id": "delegation:" + run_id,
                "run_id": run_id,
                "agent_id": SELECTED,
                "capability_digest": DIGESTS["capability"],
            },
            "provenance_refs": [],
            "max_tenant_in_flight": 8,
        }
    )


def _claim(client: Any, work_item_id: str, now_ms: int) -> dict[str, Any]:
    claim = client.work_items.claim(
        {
            "schema_version": "1",
            "tenant_ref": TEST_TENANT,
            "work_item_id": work_item_id,
            "queue_ref": None,
            "resource_class": None,
            "fairness_group": None,
            "worker_ref": TEST_AGENT_ID,
            "now_ms": now_ms,
            "lease_ms": 60_000,
            "max_tenant_in_flight": 8,
        },
        idempotency_key=f"claim-{work_item_id}",
    )
    assert claim["claimed"] is True, claim
    return claim


def _extension(
    client: Any, work_item_id: str, run_id: str, trace_id: str, fence: int
) -> dict[str, Any]:
    identity = {
        "delegation_id": "delegation:" + run_id,
        "delegator_id": DELEGATOR,
        "selected_agent_id": SELECTED,
        "executor_lease_actor": TEST_AGENT_ID,
        "outcome": "succeeded",
        "work_item_id": work_item_id,
        "run_id": run_id,
        "fence_token": fence,
        "result_ref": "cas:result:" + run_id,
        "event_sequence": 1,
        "completeness": "complete",
        "missing_refs": [],
        "capability_digest": DIGESTS["capability"],
        "catalog_digest": DIGESTS["catalog"],
        "policy_digest": DIGESTS["policy"],
        "model_digest": DIGESTS["model"],
        "outbox_id": "",
    }
    payload_ref = "cas:receipt:" + trace_id
    properties = {
        **identity,
        "node_id": trace_id,
        "kind": "run_trace",
        "result_digest": "a" * 64,
        "payload_ref": payload_ref,
        "payload": {"fixture": True},
        "run_trace": {
            "schema_version": 1,
            "run_id": run_id,
            "task_signature": "sig:quota",
            "model_id": "model:quota",
            "steps": [
                {
                    "index": 0,
                    "kind": "tool_call",
                    "action_ref": "tool:fixture",
                    "outcome": "ok",
                }
            ],
            "outcome": "succeeded",
            "started_at_ms": 1,
        },
    }
    receipt = {
        "node_id": trace_id,
        "kind": "run_trace",
        "delegation_id": identity["delegation_id"],
        "work_item_id": work_item_id,
        "run_id": run_id,
        "fence_token": fence,
        "result_ref": identity["result_ref"],
        "outbox_id": "",
        "payload_ref": payload_ref,
        "payload_digest": "",
        "properties_msgpack": WorkItemClient.receipt_properties(properties),
    }
    return {
        "outcome_bundle": {
            "schema_version": 1,
            **identity,
            "result_digest": "a" * 64,
            "artifacts": [],
            "trace_ref": trace_id,
            "tool_call_refs": [],
            "outcome_ref": "outcome:" + run_id,
            "langfuse_observation_refs": [],
        },
        "receipt_nodes": [receipt],
        "run_event": {
            "schema_version": 1,
            **{k: v for k, v in identity.items() if k != "missing_refs"},
            "missing_refs": [],
            "kind": "outcome",
            "tool_call_ref": None,
            "outcome_ref": "outcome:" + run_id,
            "payload_digest": "f" * 64,
            "timestamp_ms": 10,
            "cursor_token": "cursor:" + run_id,
            "carrier_digest": "0" * 64,
        },
    }


def _commit(
    client: Any, work_item_id: str, claim: dict[str, Any], run_id, trace_id, fence
):
    return client.work_items.commit_result(
        tenant=TEST_TENANT,
        work_item_id=work_item_id,
        worker_id=TEST_AGENT_ID,
        lease_epoch=claim["lease_epoch"],
        fencing_token=fence,
        idempotency_key=f"commit-{work_item_id}",
        outcome="succeeded",
        result_ref="cas:result:" + run_id,
        outcome_extension=_extension(client, work_item_id, run_id, trace_id, fence),
        now_ms=int(time.time() * 1000),
    )


def _traces(client: Any) -> dict[str, Any]:
    nodes = client._send(
        "GetNodesByLabel", {"label": "RunTrace", "after": None, "limit": 100}
    )
    return {node_id: props for node_id, props in nodes}


def test_quota_and_trace_end_to_end(authenticated_plan_client):
    client = authenticated_plan_client
    try:
        # Step 1: Decide tightens a requested bound, keeps the other defaults and
        # writes no trace.
        assert _quota_options(client, []) == [
            "wall_time_ms:600000",
            "memory_bytes:2147483648",
            "process_count:32",
            "grace_ms:10000",
        ]
        assert _quota_options(
            client, [dc.param("requested_wall_time_ms", "int", 5000)]
        ) == [
            "wall_time_ms:5000",
            "memory_bytes:2147483648",
            "process_count:32",
            "grace_ms:10000",
        ]
        assert _traces(client) == {}

        # Step 2: the terminal commit stores the run trace and links it from the unit.
        now_ms = int(time.time() * 1000)
        run_id = "run-" + uuid.uuid4().hex
        item = "workitem:quota:" + uuid.uuid4().hex
        trace_id = "trace:" + run_id
        _submit(client, item, run_id, now_ms)
        claim = _claim(client, item, now_ms + 1)
        committed = _commit(
            client, item, claim, run_id, trace_id, claim["fencing_token"]
        )
        assert committed["status"] == "succeeded", committed
        traces = _traces(client)
        assert list(traces) == [trace_id]
        assert traces[trace_id]["node_type"] == "RunTrace"
        assert traces[trace_id]["run_trace"]["run_id"] == run_id
        assert trace_id in client._send("GetNeighbors", {"node_id": item})

        # Step 3: a commit under a fence the lease does not hold stores neither
        # the node nor the link.
        stale_run = "run-" + uuid.uuid4().hex
        stale_item = "workitem:quota:" + uuid.uuid4().hex
        stale_trace = "trace:" + stale_run
        _submit(client, stale_item, stale_run, now_ms)
        stale_claim = _claim(client, stale_item, now_ms + 2)
        stale = _commit(
            client,
            stale_item,
            stale_claim,
            stale_run,
            stale_trace,
            stale_claim["fencing_token"] + 1,
        )
        assert stale["status"] == "fenced", stale
        assert list(_traces(client)) == [trace_id]
        assert stale_trace not in client._send("GetNeighbors", {"node_id": stale_item})
    finally:
        client.close()
