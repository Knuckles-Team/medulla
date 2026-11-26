"""Stored, versioned queries over the served wire (MEDULLA-FEDERATED-QUERY-R708).

The session fixture in conftest starts and authenticates the native server. The tests
drive the generated ``StoredQueryPut`` / ``StoredQueryCall`` requests and the UQL form
``CALL STORED name(args)``, as the principals the share scope distinguishes.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import register_commons_agents, request_context

from medulla.client import MedullaClient
from medulla.generated import ingestion, query

TEXT = "MATCH (:StoredIncident) WHERE status = $status"


async def _connect(principal: str | None = None) -> MedullaClient:
    # The agent is the principal, so the verified caller is exactly the name given.
    if principal:
        # A principal other than the suite's System identity is registered first.
        register_commons_agents(principal)
    context = request_context(agent_id=principal) if principal else request_context()
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )


def _stored(name: str, **overrides: object) -> dict:
    record: dict = {
        "name": name,
        "version": 1,
        "language": "uql",
        "text": TEXT,
        "params": {"status": "string"},
        "materialize": False,
        "refresh": {"kind": "on_change"},
    }
    record.update(overrides)
    return record


async def _put(client: MedullaClient, record: dict) -> dict:
    return (await query.send_stored_query_put(client, {"query": record})).payload


async def _call(
    client: MedullaClient, name: str, args: dict, version: int | None = None
) -> dict:
    request = {"name": name, "version": version, "args": args}
    return (await query.send_stored_query_call(client, request)).payload


async def _incident(client: MedullaClient, tag: str, status: str) -> None:
    await client.nodes.add(
        f"stored-incident-{tag}-{uuid.uuid4().hex}",
        {"type": "StoredIncident", "status": status, "tag": tag},
    )


# spec: MEDULLA-FEDERATED-QUERY-R708.3
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.2")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.3")
@pytest.mark.asyncio
async def test_put_then_call() -> None:
    client = await _connect()
    # The UQL form names the query as an identifier, so the name has no hyphen.
    name = f"open_{uuid.uuid4().hex}"
    status = f"open-{uuid.uuid4().hex}"
    try:
        stored = (await _put(client, _stored(name)))["query"]
        assert stored["version"] == 1
        await _put(client, _stored(name, version=2, materialize=True))

        await _incident(client, name, status)
        by_method = await _call(client, name, {"status": status}, version=1)
        assert by_method["row_count"] == 1
        assert by_method["materialized"] is False

        # A call without a version runs the highest published one; the UQL form reaches
        # the same handler and reads the same rows.
        latest = await _call(client, name, {"status": status})
        assert latest["version"] == 2 and latest["materialized"] is True
        by_uql = await client.query.uql(f"CALL STORED {name}(status = '{status}')")
        assert len(by_uql["rows"]) == latest["row_count"] == 1

        # An input change is visible on the next call.
        await _incident(client, name, status)
        changed = await _call(client, name, {"status": status})
        assert changed["row_count"] == 2
        assert changed["input_generation"] >= latest["input_generation"]
    finally:
        await client.close()


# spec: MEDULLA-FEDERATED-QUERY-R708.4
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.4")
@pytest.mark.asyncio
async def test_stored_query_refusals() -> None:
    client = await _connect()
    name = f"refuse-{uuid.uuid4().hex}"
    try:
        await _put(client, _stored(name))
        with pytest.raises(RuntimeError, match="STORED_QUERY_IMMUTABLE"):
            await _put(client, _stored(name, text=TEXT + " |> LIMIT 1"))
        with pytest.raises(RuntimeError, match="STORED_QUERY_PARAM_MISSING"):
            await _call(client, name, {})
        with pytest.raises(RuntimeError, match="STORED_QUERY_INVALID"):
            await _call(client, name, {"status": 3})
        with pytest.raises(RuntimeError, match="STORED_QUERY_INVALID"):
            await _put(
                client,
                _stored(f"{name}-fast", refresh={"kind": "period", "seconds": 59}),
            )
        with pytest.raises(RuntimeError, match="STORED_QUERY_NOT_FOUND"):
            await _call(client, f"{name}-absent", {})
    finally:
        await client.close()


# spec: MEDULLA-FEDERATED-QUERY-R708.5
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.5")
@pytest.mark.asyncio
async def test_staleness_reported() -> None:
    client = await _connect()
    name = f"stale-{uuid.uuid4().hex}"
    status = f"open-{uuid.uuid4().hex}"
    try:
        await _incident(client, name, status)
        await _put(
            client,
            _stored(
                name, materialize=True, refresh={"kind": "period", "seconds": 3600}
            ),
        )
        first = await _call(client, name, {"status": status})
        second = await _call(client, name, {"status": status})
        # Inside the period the stored result is read back as it was refreshed.
        assert second["refreshed_at_ms"] == first["refreshed_at_ms"]
        assert second["input_generation"] == first["input_generation"]
        assert isinstance(second["staleness_s"], int) and second["staleness_s"] >= 0
        assert second["staleness_s"] < 3600
    finally:
        await client.close()


# spec: MEDULLA-FEDERATED-QUERY-R708.8
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.8")
@pytest.mark.asyncio
async def test_share_scope_limits_calls() -> None:
    author = await _connect("usr:stored-author")
    sharee = await _connect("usr:stored-sharee")
    suffix = uuid.uuid4().hex
    private, shared = f"private-{suffix}", f"shared-{suffix}"
    try:
        # The author comes from the verified principal, whatever the request says.
        put = await _put(author, _stored(private, author="usr:someone-else"))
        assert put["query"]["author"] == "usr:stored-author"
        await _put(author, _stored(shared, share="tenant"))

        assert (await _call(author, private, {"status": "x"}))["version"] == 1
        with pytest.raises(RuntimeError, match="ACCESS_DENIED"):
            await _call(sharee, private, {"status": "x"})
        assert (await _call(sharee, shared, {"status": "x"}))["version"] == 1
        # Only the author publishes further versions of a name.
        with pytest.raises(RuntimeError, match="ACCESS_DENIED"):
            await _put(sharee, _stored(shared, version=2, share="tenant"))
    finally:
        await author.close()
        await sharee.close()


# spec: MEDULLA-FEDERATED-QUERY-R708.10
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.10")
@pytest.mark.asyncio
async def test_sql_and_sparql_stored_queries_call() -> None:
    client = await _connect()
    suffix = uuid.uuid4().hex
    status = f"o'pen-{suffix}"
    other = f"closed-{suffix}"
    subject = f"http://example.org/stored/{suffix}"
    predicate = "http://example.org/stored/status"
    try:
        await _incident(client, suffix, status)
        await _incident(client, suffix, other)
        await client.rdf.add_triples(
            ntriples=(
                f'<{subject}-a> <{predicate}> "{status}" .\n'
                f'<{subject}-b> <{predicate}> "{other}" .\n'
            )
        )

        sql = f"sql-{suffix}"
        await _put(
            client,
            _stored(
                sql,
                language="sql",
                text="SELECT id, status FROM nodes WHERE status = $status",
            ),
        )
        called = await _call(client, sql, {"status": status})
        assert called["row_count"] == 1
        assert called["columns"] == ["id", "status"]
        assert called["rows"][0]["status"] == status
        # The value is one literal: a quote and a SQL fragment in it match nothing.
        injected = await _call(client, sql, {"status": f"{status}' OR '1'='1"})
        assert injected["row_count"] == 0

        sparql = f"sparql-{suffix}"
        await _put(
            client,
            _stored(
                sparql,
                language="sparql",
                text=f"SELECT ?s WHERE {{ ?s <{predicate}> $status }}",
            ),
        )
        called = await _call(client, sparql, {"status": status})
        assert called["row_count"] == 1
        assert called["columns"] == ["s"]
        assert called["rows"][0]["s"].strip("<>") == f"{subject}-a"
        injected = await _call(client, sparql, {"status": 'x" . ?s ?p ?o . "'})
        assert injected["row_count"] == 0

        # Text that does not parse, an undeclared placeholder and a SQL write are
        # refused at put, and the call refusals are the shared ones.
        with pytest.raises(RuntimeError, match="STORED_QUERY_INVALID"):
            await _put(
                client,
                _stored(f"{sql}-bad", language="sql", text="SELEC id FROM nodes"),
            )
        with pytest.raises(RuntimeError, match="STORED_QUERY_INVALID"):
            await _put(
                client,
                _stored(
                    f"{sql}-undeclared",
                    language="sql",
                    text="SELECT id FROM nodes WHERE status = $nope",
                ),
            )
        with pytest.raises(RuntimeError, match="STORED_QUERY_INVALID"):
            await _put(
                client,
                _stored(
                    f"{sql}-write",
                    language="sql",
                    params={},
                    text="DELETE FROM nodes WHERE id = 'x'",
                ),
            )
        with pytest.raises(RuntimeError, match="STORED_QUERY_PARAM_MISSING"):
            await _call(client, sparql, {})
    finally:
        await client.close()


# spec: MEDULLA-FEDERATED-QUERY-R708.7
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.7")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.9")
@pytest.mark.asyncio
async def test_workspace_share_schedule_and_lint() -> None:
    """The shared query workspace: a query is shared by scope and refreshes no faster
    than the floor. It warns on its put receipt and on each call when it reads a
    deprecated asset, and the save and the run still proceed."""
    client = await _connect()
    sharee = await _connect(f"workspace-sharee-{uuid.uuid4().hex[:8]}")
    suffix = uuid.uuid4().hex
    label = f"StoredLint{suffix}"
    name = f"lint_{suffix}"
    text = f"MATCH (:{label}) WHERE status = $status"
    try:
        # The share scope: a tenant-shared query is callable by another principal.
        clean = await _put(client, _stored(name, text=text, share="tenant"))
        assert clean["query"]["version"] == 1
        assert clean["warnings"] == []
        assert (await _call(sharee, name, {"status": "open"}))["version"] == 1
        # The schedule: a period under the floor knob is refused.
        with pytest.raises(RuntimeError, match="STORED_QUERY_INVALID"):
            await _put(
                client,
                _stored(
                    f"{name}_fast",
                    refresh={"kind": "period", "seconds": 1},
                    materialize=True,
                ),
            )
        assert (await _call(client, name, {"status": "open"}))["warnings"] == []

        # A steward deprecates the asset the query reads.
        await ingestion.send_stewardship(
            client,
            {
                "op": {
                    "op": "set_trust_flag",
                    "asset": label,
                    "flag": "deprecation",
                    "reason": "replaced by a newer table",
                }
            },
        )
        expected = [{"code": "STORED_QUERY_DEPRECATED_ASSET", "asset": label}]

        # The save proceeds and carries the typed warning on its receipt.
        second = await _put(client, _stored(name, text=text, version=2, share="tenant"))
        assert second["query"]["version"] == 2
        assert second["warnings"] == expected

        # The run proceeds too; the lint is re-read before it, so version 1, saved
        # clean, now warns.
        called = await _call(client, name, {"status": "open"}, version=1)
        assert called["warnings"] == expected
        assert called["row_count"] == 0
    finally:
        await sharee.close()
        await client.close()


async def _get(client: MedullaClient, name: str, version: int | None = None) -> dict:
    request = {"name": name, "version": version}
    return (await query.send_stored_query_get(client, request)).payload


async def _list(client: MedullaClient, **params: object) -> dict:
    return (await query.send_stored_query_list(client, params)).payload


async def _retire(client: MedullaClient, name: str, reason: str) -> dict:
    request = {"name": name, "reason": reason}
    return (await query.send_stored_query_retire(client, request)).payload


# spec: MEDULLA-FEDERATED-QUERY-R708.12
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R708.12")
@pytest.mark.asyncio
async def test_get_list_and_retire_a_stored_query() -> None:
    author = await _connect("usr:stored-workspace-author")
    sharee = await _connect("usr:stored-workspace-sharee")
    suffix = uuid.uuid4().hex
    private, shared = f"wsprivate-{suffix}", f"wsshared-{suffix}"
    try:
        await _put(author, _stored(private))
        await _put(author, _stored(shared, share="tenant"))
        await _put(author, _stored(shared, version=2, share="tenant"))

        # Get returns the version as written, the highest one when none is named.
        latest = await _get(author, shared)
        assert latest["version"] == 2 and latest["text"] == TEXT
        assert latest["author"] == "usr:stored-workspace-author"
        assert latest["created_at_ms"] > 0
        assert (await _get(author, shared, version=1))["version"] == 1

        # A private name and a missing name refuse alike for another principal.
        with pytest.raises(RuntimeError, match="STORED_QUERY_NOT_FOUND"):
            await _get(sharee, private)
        with pytest.raises(RuntimeError, match="STORED_QUERY_NOT_FOUND"):
            await _get(sharee, f"absent-{suffix}")
        assert (await _get(sharee, shared))["version"] == 2

        # List pages by cursor and shows each reader only what it may call.
        page = await _list(sharee, limit=1, author="usr:stored-workspace-author")
        listed = [entry["name"] for entry in page["queries"]]
        cursor = page["next_cursor"]
        while cursor:
            page = await _list(
                sharee, limit=1, cursor=cursor, author="usr:stored-workspace-author"
            )
            listed += [entry["name"] for entry in page["queries"]]
            cursor = page["next_cursor"]
        assert shared in listed and private not in listed
        mine = {
            entry["name"]
            for entry in (await _list(author, author="usr:stored-workspace-author"))[
                "queries"
            ]
        }
        assert {private, shared} <= mine

        # Retire is a tombstone: the name is no longer served or published again.
        with pytest.raises(RuntimeError, match="STORED_QUERY_NOT_FOUND"):
            await _retire(sharee, shared, "superseded")
        retired = await _retire(author, shared, "superseded")
        assert retired["retired"] is True
        assert retired["tombstone"]["reason"] == "superseded"
        with pytest.raises(RuntimeError, match="STORED_QUERY_NOT_FOUND"):
            await _get(author, shared)
        with pytest.raises(RuntimeError, match="STORED_QUERY_NOT_FOUND"):
            await _call(author, shared, {"status": "x"})
        with pytest.raises(RuntimeError, match="STORED_QUERY_IMMUTABLE"):
            await _put(author, _stored(shared, version=3, share="tenant"))
        with_retired = await _list(
            author, include_retired=True, author="usr:stored-workspace-author"
        )
        tombstones = {
            entry["name"]: entry["retired"] for entry in with_retired["queries"]
        }
        assert tombstones[shared]["reason"] == "superseded"
        assert tombstones[private] is None
    finally:
        await author.close()
        await sharee.close()
