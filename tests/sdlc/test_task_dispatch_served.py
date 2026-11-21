"""Run-context assembly for a `TaskPlan` question (MEDULLA-SDLC-R015.2).

L3: a `TaskPlan` question that carries `task_context_row` and `task_context_repo`
gets its run context assembled from the graph behind the ordinary read ACL; the
digest of the context rides in the answer's premises as `task_context.digest`.
"""

from __future__ import annotations

import sys
import uuid
from pathlib import Path
from types import SimpleNamespace

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "decide"))

from _decision_resident_fixture import publish, schema_body  # noqa: E402
from conftest import (  # noqa: E402
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    TEST_TENANT,
    bootstrap_context,
    request_context,
    strict_server_env,
)
from test_task_plan_served import (  # noqa: E402,F401
    TEST_AUTH_FIXTURE_VALUE,
    _proposal,
    authenticated_plan_client,
)

from medulla.client import EngineResponseError, SyncMedullaClient  # noqa: E402
from medulla.decision_client import param, task_plan_request  # noqa: E402

pytestmark = pytest.mark.no_engine  # The test owns its authenticated engine.

GRAPH = "__commons__"
READER = "service:worker-2"


@pytest.fixture
def plan_writer_and_reader(tmp_path):
    """One authenticated engine, a system writer and a registered non-System reader."""
    from test_native_no_auth_served import server

    secret = TEST_AUTH_FIXTURE_VALUE
    env = strict_server_env(
        str(tmp_path / "security"),
        auth_secret=secret,
        persist_dir=str(tmp_path / "data"),
    )
    env["MEDULLA_ENCRYPTION_KEY"] = "test-medulla-encryption-key"
    with server(tmp_path, mode="authenticated", extra_env=env) as address:
        tcp = f"{address[0]}:{address[1]}"

        def connect(agent_id: str | None = None, context: dict | None = None):
            return SyncMedullaClient.connect(
                tcp_addr=tcp,
                auth_secret=secret,
                graph_name=GRAPH,
                verified_context=context or request_context(agent_id=agent_id),
            )

        bootstrap = connect(context=bootstrap_context())
        try:
            bootstrap.consensus.bootstrap_system_identity(
                agent_id=TEST_AGENT_ID,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )
        finally:
            bootstrap.close()
        writer = connect(TEST_AGENT_ID)
        writer.rbac.add_role("commons-access")
        writer.rbac.add_grant("commons-access", {"Graph": GRAPH}, "Read")
        writer.rbac.add_grant("commons-access", {"Graph": GRAPH}, "Write")
        writer.consensus.register_identity(
            READER,
            "Agent",
            ["team:test"],
            ["commons-access"],
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
        reader = connect(READER)
        try:
            yield SimpleNamespace(writer=writer, reader=reader)
        finally:
            reader.close()
            writer.close()


def _node(client, node_id: str, properties: dict) -> None:
    client._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
        },
        GRAPH,
    )


def _edge(client, source: str, target: str, relationship: str) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": source,
            "target_id": target,
            "properties_msgpack": msgpack.packb(
                {"relationship": relationship}, use_bin_type=True
            ),
        },
        GRAPH,
    )


def _decide(client, pin, repo: str, row: str, graph: str | None = GRAPH) -> dict:
    asked = task_plan_request(TEST_TENANT, "context-" + uuid.uuid4().hex[:8], _proposal(), pin)
    asked["params"] = [
        *asked["params"],
        param("task_context_row", "text", row),
        param("task_context_repo", "text", repo),
        *([param("graph", "text", graph)] if graph is not None else []),
    ]
    return client._send("Decide", {"request": asked})["records"][0]


def _digest(record: dict) -> str:
    facts = [
        premise["fact"]
        for premise in record["premises"]
        if premise["fact"].startswith("task_context.digest=")
    ]
    assert len(facts) == 1
    return facts[0].split("=", 1)[1]


@pytest.mark.spec("MEDULLA-SDLC-R015.2")
def test_context_holds_row_plan_contracts_and_neighbourhood(
    authenticated_plan_client,
) -> None:
    client = authenticated_plan_client
    try:
        suffix = uuid.uuid4().hex[:8]
        repo, row = f"repo-{suffix}", "DEMO-R001"
        requirement = f"requirement:{repo}:{row}"
        plan = f"plansection:{repo}:specs/a/plan.md#L3"
        contract = f"specsection:{repo}:specs/a/contracts/seam.md#L1"
        symbol = f"symbol:serve_{suffix}"
        _node(client, requirement, {"type": "Requirement", "text": "Serve the row."})
        _node(
            client,
            plan,
            {"type": "PlanSection", "title": "Plan", "start_line": "3", "text": "Plan the row."},
        )
        _node(
            client,
            contract,
            {
                "type": "SpecSection",
                "repo": repo,
                "file": "specs/a/contracts/seam.md",
                "title": "Seam",
                "start_line": "1",
                "text": f"Provider: demo {row}",
            },
        )
        _node(client, symbol, {"type": "CodeSymbolRef", "name": f"serve_{suffix}"})
        _edge(client, plan, requirement, "covers")
        _edge(client, requirement, symbol, "implementedBy")

        pin = publish(client, "task-context-schema-" + suffix, "feature_schema", schema_body())
        first = _digest(_decide(client, pin, repo, row))
        # spec: MEDULLA-SDLC-R015.2
        assert first == _digest(_decide(client, pin, repo, row))

        # A row with no covering plan section refuses.
        _node(client, f"requirement:{repo}:DEMO-R002", {"type": "Requirement", "text": "x"})
        with pytest.raises(EngineResponseError, match="TASK_CONTEXT_PLAN_MISSING"):
            _decide(client, pin, repo, "DEMO-R002")
        with pytest.raises(EngineResponseError, match="TASK_CONTEXT_ROW_UNKNOWN"):
            _decide(client, pin, repo, "DEMO-R404")
        # The graph is required: a question without it refuses and assumes no graph.
        with pytest.raises(EngineResponseError, match="PARAMETER_INVALID"):
            _decide(client, pin, repo, row, graph=None)
    finally:
        client.close()


@pytest.mark.spec("MEDULLA-SDLC-R015.2")
def test_hidden_section_changes_neither_digest_nor_bound(plan_writer_and_reader) -> None:
    writer, reader = plan_writer_and_reader.writer, plan_writer_and_reader.reader
    suffix = uuid.uuid4().hex[:8]
    repo, row = f"repo-{suffix}", "DEMO-R001"
    requirement = f"requirement:{repo}:{row}"
    shared = {"_owner": TEST_AGENT_ID, "_visibility": "public"}
    _node(writer, requirement, {"type": "Requirement", "text": "Serve the row.", **shared})
    plan = f"plansection:{repo}:specs/a/plan.md#L3"
    _node(
        writer,
        plan,
        {"type": "PlanSection", "title": "Plan", "start_line": "3", "text": "Plan the row.", **shared},
    )
    _edge(writer, plan, requirement, "covers")
    pin = publish(writer, "task-context-hidden-" + suffix, "feature_schema", schema_body())
    before = _digest(_decide(reader, pin, repo, row))

    # A contract section the reader may not see: private, owned by another principal.
    hidden = f"specsection:{repo}:specs/a/contracts/secret.md#L1"
    _node(
        writer,
        hidden,
        {
            "type": "SpecSection",
            "repo": repo,
            "file": "specs/a/contracts/secret.md",
            "title": "Secret",
            "start_line": "1",
            "text": f"Provider: hidden {row} " + "x" * 4096,
            "_owner": "service:worker-1",
            "_visibility": "private",
        },
    )
    assert _digest(_decide(reader, pin, repo, row)) == before
    # The system identity sees the section, so its digest differs.
    assert _digest(_decide(writer, pin, repo, row)) != before
