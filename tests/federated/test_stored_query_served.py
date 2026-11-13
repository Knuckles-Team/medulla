"""Stored, versioned queries over the served wire (MEDULLA-FEDERATED-QUERY-R708).

The session fixture in conftest starts and authenticates the native server. The tests
drive the generated ``StoredQueryPut`` / ``StoredQueryCall`` requests and the UQL form
``CALL STORED name(args)``, as the principals the share scope distinguishes.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

TEXT = "MATCH (:StoredIncident) WHERE status = $status"


async def _connect(principal: str | None = None) -> MedullaClient:
    # The agent is the principal, so the verified caller is exactly the name given.
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
    name = f"open-{uuid.uuid4().hex}"
    status = f"open-{uuid.uuid4().hex}"
    try:
        stored = await _put(client, _stored(name))
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
        assert put["author"] == "usr:stored-author"
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
