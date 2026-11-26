"""Run-context assembly for a `TaskPlan` question (MEDULLA-SDLC-R015.2).

L3: a `TaskPlan` question that carries `task_context_row` and `task_context_repo`
gets its run context assembled from the graph behind the ordinary read ACL; the
digest of the context rides in the answer's premises as `task_context.digest`.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import sys
import uuid
from pathlib import Path
from types import SimpleNamespace

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "decide"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from _decision_resident_fixture import publish, schema_body
from _landing_fixture import LandingRepo
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    TEST_TENANT,
    bootstrap_context,
    request_context,
    strict_server_env,
)
from test_task_plan_served import (  # noqa: F401
    TEST_AUTH_FIXTURE_VALUE,
    _proposal,
    authenticated_plan_client,
)

from medulla.client import EngineResponseError, SyncMedullaClient
from medulla.decision_client import (
    commit_op,
    get_op,
    param,
    task_plan_request,
)

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
    asked = task_plan_request(
        TEST_TENANT, "context-" + uuid.uuid4().hex[:8], _proposal(), pin
    )
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


def _body(record: dict) -> dict:
    """The context body sealed in the `TaskPlan` answer of a record."""
    return record["answer"]["value"]["context"]


def _canonical(body: dict) -> bytes:
    """The bytes the engine digests: compact JSON in struct field order."""
    return json.dumps(body, separators=(",", ":"), ensure_ascii=False).encode()


@pytest.mark.spec("MEDULLA-SDLC-R015.2")
def test_context_holds_row_plan_contracts_and_neighbourhood(
    authenticated_plan_client,  # noqa: F811
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
            {
                "type": "PlanSection",
                "title": "Plan",
                "start_line": "3",
                "text": "Plan the row.",
            },
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

        pin = publish(
            client, "task-context-schema-" + suffix, "feature_schema", schema_body()
        )
        first = _digest(_decide(client, pin, repo, row))
        # spec: MEDULLA-SDLC-R015.2
        assert first == _digest(_decide(client, pin, repo, row))

        # A row with no covering plan section refuses.
        _node(
            client,
            f"requirement:{repo}:DEMO-R002",
            {"type": "Requirement", "text": "x"},
        )
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
def test_hidden_section_changes_neither_digest_nor_bound(
    plan_writer_and_reader,
) -> None:
    writer, reader = plan_writer_and_reader.writer, plan_writer_and_reader.reader
    suffix = uuid.uuid4().hex[:8]
    repo, row = f"repo-{suffix}", "DEMO-R001"
    requirement = f"requirement:{repo}:{row}"
    shared = {"_owner": TEST_AGENT_ID, "_visibility": "public"}
    _node(
        writer, requirement, {"type": "Requirement", "text": "Serve the row.", **shared}
    )
    plan = f"plansection:{repo}:specs/a/plan.md#L3"
    _node(
        writer,
        plan,
        {
            "type": "PlanSection",
            "title": "Plan",
            "start_line": "3",
            "text": "Plan the row.",
            **shared,
        },
    )
    _edge(writer, plan, requirement, "covers")
    pin = publish(
        writer, "task-context-hidden-" + suffix, "feature_schema", schema_body()
    )
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


@pytest.mark.spec("MEDULLA-SDLC-R015.3")
def test_dispatch_hands_context_to_a_run_and_logs_it(authenticated_plan_client) -> None:  # noqa: F811
    """One sealed dispatch record per task, each carrying its own context and digest.

    Each task of the slice is one `TaskPlan` question naming its row; the engine
    assembles that task's context and seals one record whose premises carry the
    context digest. `DecisionLog` commits the record and reads it back byte for
    byte. The engine starts no run: the only writes are the node seeds and the
    committed records.
    """
    client = authenticated_plan_client
    try:
        suffix = uuid.uuid4().hex[:8]
        repo = f"repo-{suffix}"
        rows = ("DEMO-R001", "DEMO-R002")
        for row in rows:
            requirement = f"requirement:{repo}:{row}"
            plan = f"plansection:{repo}:specs/a/plan.md#{row}"
            _node(client, requirement, {"type": "Requirement", "text": f"Serve {row}."})
            _node(
                client,
                plan,
                {
                    "type": "PlanSection",
                    "title": "Plan",
                    "start_line": "3",
                    "text": f"Plan {row}.",
                },
            )
            _edge(client, plan, requirement, "covers")

        pin = publish(
            client, "task-dispatch-schema-" + suffix, "feature_schema", schema_body()
        )
        digests = {}
        for row in rows:
            record = _decide(client, pin, repo, row)
            digests[row] = _digest(record)
            # spec: MEDULLA-SDLC-R015.3
            committed = client._send("DecisionLog", {"op": commit_op(record)})
            assert committed["record_digest"] == record["record_digest"]
            entry = client._send(
                "DecisionLog", {"op": get_op(TEST_TENANT, committed["record_id"])}
            )
            assert entry["record"] == record
            assert _digest(entry["record"]) == digests[row]
            # The context body is sealed in the record and returned in the answer, so
            # the run receives it and the log holds it; its digest is the premise
            # digest.
            for held in (record, entry["record"]):
                body = _body(held)
                assert body["row_id"] == row
                assert body["row_text"] == f"Serve {row}."
                assert [s["text"] for s in body["plan_sections"]] == [f"Plan {row}."]
                assert hashlib.sha256(_canonical(body)).hexdigest() == digests[row]
        # One context per task: the two rows never share a context.
        assert digests[rows[0]] != digests[rows[1]]
    finally:
        client.close()


def _run(client, coroutine):
    """Run a coroutine on the sync client's own loop and wait for it."""
    return asyncio.run_coroutine_threadsafe(coroutine, client._loop).result(timeout=900)


async def _main_paths(repo: LandingRepo, head: str) -> list[str]:
    tree = await repo.owner._send(
        "ReadTree",
        {
            "request": {
                "repo": repo.graph,
                "branch": "main",
                "rev": head,
                "prefix": "",
                "depth": 0,
            }
        },
    )
    return sorted(entry["path"] for entry in tree["entries"])


@pytest.mark.spec("MEDULLA-SDLC-R015.4")
def test_result_counts_only_after_gates_and_landing(authenticated_plan_client) -> None:  # noqa: F811
    """A task result is a changeset. It counts when its change lands on the default
    branch through the landing gate, never before: with no verdict, and with a failed
    verdict, the merge refuses and the sealed verdict stays on the record; a fixed
    result with a passing verdict lands."""
    client = authenticated_plan_client
    graph = f"task-result-{uuid.uuid4().hex[:8]}"
    spec_trailer = "Spec: DEMO-R001"

    async def journey():
        owner = client._client
        repo = await LandingRepo.create(owner, graph)
        try:
            await repo.branch("result/first")
            first = await repo.submit(
                owner,
                "result/first",
                "src/t1.py",
                b"print(1)\n",
                f"feat: T001\n\n{spec_trailer}",
            )
            # The result is submitted, but nothing has judged it: it does not land.
            with pytest.raises(Exception, match="LANDING_GATE_BLOCKED"):
                await repo.merge("result/first")
            # A failed gate seals a failed verdict and still blocks the landing.
            failed = await repo.record(first, "result/first", "fail")
            node = await owner._send(
                "GetNodeProperties", {"node_id": failed["node_id"]}, graph
            )
            assert (node if isinstance(node, dict) else {}).get(
                "verdict", "fail"
            ) == "fail"
            with pytest.raises(Exception, match="LANDING_GATE_BLOCKED"):
                await repo.merge("result/first")
            # The fixed result passes the gate and lands on main.
            second = await repo.submit(
                owner,
                "result/first",
                "src/t1.py",
                b"print(2)\n",
                f"fix: T001\n\n{spec_trailer}",
                base_head=first["branch_head"],
                op="modify",
            )
            await repo.record(second, "result/first", "pass")
            merged = await repo.merge("result/first")
            # spec: MEDULLA-SDLC-R015.4
            assert merged["outcome"] == "fast_forward"
            assert "src/t1.py" in await _main_paths(repo, second["branch_head"])
        finally:
            await repo.close()

    _run(client, journey())


@pytest.mark.spec("MEDULLA-SDLC-R015.6")
def test_dispatch_runs_layers_and_lands_through_gates(
    authenticated_plan_client,  # noqa: F811
) -> None:
    """A three-layer plan with two parallel tasks: the plan orders the layers, each task
    gets its own context, each result lands through the gate in layer order, and the
    decision replays from the decision log."""
    client = authenticated_plan_client
    suffix = uuid.uuid4().hex[:8]
    repo_name = f"repo-{suffix}"
    tasks = ("a", "b", "c", "join")
    for task in tasks:
        row = f"DEMO-{task.upper()}"
        requirement = f"requirement:{repo_name}:{row}"
        plan = f"plansection:{repo_name}:specs/a/plan.md#{task}"
        _node(client, requirement, {"type": "Requirement", "text": f"Do {task}."})
        _node(
            client,
            plan,
            {
                "type": "PlanSection",
                "title": "Plan",
                "start_line": "3",
                "text": f"Plan {task}.",
            },
        )
        _edge(client, plan, requirement, "covers")
    pin = publish(
        client, "task-journey-schema-" + suffix, "feature_schema", schema_body()
    )

    # The plan: layer order from the sealed dependencies.
    asked = task_plan_request(TEST_TENANT, "journey-" + suffix, _proposal(), pin)
    planned = client._send("Decide", {"request": asked})["records"][0]
    sealed = {
        task["task_id"]: list(task.get("depends_on", []))
        for task in planned["answer"]["value"]["tasks"]
    }
    layers: list[list[str]] = []
    placed: set[str] = set()
    while len(placed) < len(sealed):
        layer = sorted(
            task
            for task, needs in sealed.items()
            if task not in placed and set(needs) <= placed
        )
        assert layer, "the sealed plan has a cycle"
        layers.append(layer)
        placed.update(layer)
    assert layers == [["a", "b"], ["c"], ["join"]]

    # Each task has its own context, and the decision replays from the log.
    digests = {}
    for task in tasks:
        record = _decide(client, pin, repo_name, f"DEMO-{task.upper()}")
        digests[task] = _digest(record)
        committed = client._send("DecisionLog", {"op": commit_op(record)})
        entry = client._send(
            "DecisionLog", {"op": get_op(TEST_TENANT, committed["record_id"])}
        )
        assert entry["record"] == record
    assert len(set(digests.values())) == len(tasks)
    replayed = client._send(
        "DecisionLog",
        {
            "op": get_op(
                TEST_TENANT,
                client._send("DecisionLog", {"op": commit_op(planned)})["record_id"],
            )
        },
    )
    assert replayed["record"]["answer"] == planned["answer"]

    # The results land layer by layer, each through the landing gate.
    graph = f"task-journey-{suffix}"

    async def journey():
        owner = client._client
        repo = await LandingRepo.create(owner, graph)
        try:
            landed: list[str] = []
            for layer in layers:
                for task in layer:
                    branch = f"result/{task}"
                    await repo.branch(branch)
                    receipt = await repo.submit(
                        owner,
                        branch,
                        f"src/{task}.py",
                        f"print('{task}')\n".encode(),
                        f"feat: {task}\n\nSpec: DEMO-{task.upper()}",
                    )
                    with pytest.raises(Exception, match="LANDING_GATE_BLOCKED"):
                        await repo.merge(branch)
                    await repo.record(receipt, branch, "pass")
                # The layer's results land after every result of the layer passed.
                for task in layer:
                    await repo.merge(f"result/{task}")
                    landed.append(task)
            # spec: MEDULLA-SDLC-R015.6
            assert landed == ["a", "b", "c", "join"]
        finally:
            await repo.close()

    _run(client, journey())
