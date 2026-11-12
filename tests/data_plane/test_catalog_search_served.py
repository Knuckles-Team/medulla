"""Served asset usage (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1).

The server records which datasets a served query referenced, per verified
principal, in the `asset_usage.reads` and `asset_usage.queries` series. The count
comes only from the server's own dispatch: a usage claim sent by the client is
not a field of any method and changes nothing.
"""

from __future__ import annotations

import os
import socket
import subprocess
import time
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_AGENT_ID, request_context


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


# The served engine's outbound allow-list is read from the process environment at start,
# so the loopback source named here is allowed before the session engine starts.
_PG_PORT = _free_port()
os.environ["MEDULLA_FEDERATION_ALLOW"] = ",".join(
    part
    for part in (
        os.environ.get("MEDULLA_FEDERATION_ALLOW", ""),
        f"127.0.0.1:{_PG_PORT}",
    )
    if part
)


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _sql(client: Any, graph: str, query: str) -> Any:
    return _decode(client._send("Sql", {"query": query, "params_msgpack": b""}, graph))


def _usage_points(client: Any, graph: str, base: str, asset: str) -> int:
    return _series_total(
        client, graph, f"{base}|principal={TEST_AGENT_ID}|asset={asset}"
    )


def _series_total(client: Any, graph: str, series: str) -> int:
    rows = _decode(
        client._send("TsRange", {"series_id": series, "from": 0, "to": 2**62}, graph)
    )
    return sum(int(sum(values)) for _ts, values in rows)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.3.1")
def test_usage_counts_come_from_served_queries(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.3.1: two served reads count two."""
    graph = f"usage-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") == 0
        _sql(client, graph, "SELECT count(*) AS n FROM nodes")
        _sql(client, graph, "SELECT count(*) AS n FROM nodes")
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") == 2
        assert _usage_points(client, graph, "asset_usage.queries", "nodes") == 2

        # A failed statement records nothing.
        with pytest.raises(Exception, match="."):
            _sql(client, graph, "SELECT nope FROM nodes WHERE")
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") == 2

        # A client-supplied usage claim is not a field of any method. Whether the
        # server drops the extra field or refuses the request, it counts at most the
        # one read it served itself, never the claimed 1000.
        try:
            client._send(
                "Sql",
                {
                    "query": "SELECT count(*) AS n FROM nodes",
                    "params_msgpack": b"",
                    "asset_usage": {"asset": "nodes", "reads": 1000},
                },
                graph,
            )
        except Exception:
            pass
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") in (2, 3)
    finally:
        client.tenants.delete(graph)
        client.close()


_ENV_DSN = "MEDULLA_TEST_PG_DSN"


@pytest.fixture(scope="module")
def pg_log_source():
    """A PostgreSQL server with `pg_stat_statements` and a way to run SQL as a role.

    The nightly lane names one in MEDULLA_TEST_PG_DSN; otherwise a disposable container
    preloads the extension. Skips when neither exists.
    """
    psycopg = pytest.importorskip("psycopg")
    container = None
    dsn = os.environ.get(_ENV_DSN)
    if not dsn:
        if subprocess.run(["docker", "version"], capture_output=True).returncode != 0:
            pytest.skip(f"{_ENV_DSN} is not set and docker is unavailable")
        container = f"medulla-qlog-pg-{uuid.uuid4().hex[:8]}"
        subprocess.run(
            [
                "docker",
                "run",
                "-d",
                "--rm",
                "--name",
                container,
                "-e",
                "POSTGRES_PASSWORD=qlog-test",
                "-p",
                f"127.0.0.1:{_PG_PORT}:5432",
                "postgres:16-alpine",
                "-c",
                "shared_preload_libraries=pg_stat_statements",
            ],
            check=True,
            capture_output=True,
        )
        dsn = f"postgres://postgres:qlog-test@127.0.0.1:{_PG_PORT}/postgres"
    try:
        deadline = time.monotonic() + 60
        while True:
            try:
                with psycopg.connect(dsn, autocommit=True, connect_timeout=2) as conn:
                    conn.execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")
                    conn.execute("SELECT 1 FROM pg_stat_statements LIMIT 1")
                break
            except Exception:
                if time.monotonic() > deadline:
                    pytest.skip("the PostgreSQL server did not become ready")
                time.sleep(0.5)

        def run_as(role: str | None, statements: list[str]) -> None:
            with psycopg.connect(dsn, autocommit=True) as conn:
                if role:
                    conn.execute(f'SET SESSION AUTHORIZATION "{role}"')
                for statement in statements:
                    conn.execute(statement)

        yield dsn, run_as
    finally:
        if container:
            subprocess.run(["docker", "rm", "-f", container], capture_output=True)


def _wait_job(client: Any, job_id: str) -> dict:
    status: dict = {}
    for _ in range(300):
        status = client.jobs.status(job_id)
        state = status["state"]
        name = state if isinstance(state, str) else next(iter(state))
        if name in ("Succeeded", "Failed", "Cancelled"):
            assert name == "Succeeded", f"query log job ended {state!r}"
            return status
        time.sleep(0.1)
    raise AssertionError(f"query log job did not finish: {status!r}")


def _ingest(client: Any, graph: str, source: str) -> dict:
    job = client.jobs.submit(graph, {"SourceQueryLogIngest": {"source": source}})
    return _wait_job(client, job["job_id"])


class _Log:
    """One attached Postgres source whose log holds statements of two roles."""

    def __init__(self, client: Any, graph: str, dsn: str, run_as: Any) -> None:
        tag = uuid.uuid4().hex[:8]
        self.client, self.graph, self.run_as = client, graph, run_as
        self.source = f"qlog-{tag}"
        self.orders, self.customers = f"orders_{tag}", f"customers_{tag}"
        self.alice, self.bob = f"qa_{tag}", f"qb_{tag}"
        run_as(
            None,
            [
                f"CREATE TABLE {self.orders} "
                "(id int, customer_id int, status text, total int)",
                f"CREATE TABLE {self.customers} (id int, region text)",
                f'CREATE ROLE "{self.alice}" LOGIN SUPERUSER',
                f'CREATE ROLE "{self.bob}" LOGIN SUPERUSER',
            ],
        )
        client.query.register_foreign_source(
            self.source,
            {"Sql": {"dsn": dsn, "query": "SELECT 1 AS id", "id_field": "id"}},
        )

    def alice_joins(self, times: int) -> None:
        sql = (
            f"SELECT o.id FROM {self.orders} o JOIN {self.customers} c "
            f"ON c.id = o.customer_id WHERE o.status = 'open'"
        )
        for _ in range(times):
            self.run_as(self.alice, [sql])

    def bob_reads(self, times: int) -> None:
        for _ in range(times):
            self.run_as(self.bob, [f"SELECT id FROM {self.orders} WHERE total > 10"])

    def series(self, base: str, user: str, asset: str, tail: str = "") -> int:
        name = f"{base}|principal=source:{self.source}:{user}|asset={asset}{tail}"
        return _series_total(self.client, self.graph, name)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.7")
def test_source_query_log_feeds_usage(start_medulla_server, pg_log_source) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.7: reads, joins, watermark, replay."""
    dsn, run_as = pg_log_source
    graph = f"qlog-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        log = _Log(client, graph, dsn, run_as)
        log.alice_joins(3)
        log.bob_reads(2)
        first = _ingest(client, graph, log.source)
        row = first["output"]["rows"][0]
        assert row["watermark_ms"] > 0 and row["statements"] >= 5

        reads = "asset_usage.reads"
        assert log.series(reads, log.alice, log.orders) == 3
        assert log.series(reads, log.alice, log.customers) == 3
        assert log.series(reads, log.bob, log.orders) == 2
        join = f"|with={log.customers}"
        assert log.series("asset_usage.join", log.alice, log.orders, join) == 3
        assert log.series("asset_usage.join", log.bob, log.orders, join) == 0
        flt = "asset_usage.filter"
        assert log.series(flt, log.alice, log.orders, "|column=status") == 3
        assert log.series(flt, log.bob, log.orders, "|column=total") == 2

        # A replay of the same log state adds nothing.
        _ingest(client, graph, log.source)
        assert log.series(reads, log.alice, log.orders) == 3
        assert log.series(reads, log.bob, log.orders) == 2

        # Only the growth since the watermark is counted.
        log.alice_joins(1)
        _ingest(client, graph, log.source)
        assert log.series(reads, log.alice, log.orders) == 4
        assert log.series("asset_usage.join", log.alice, log.orders, join) == 4

        # A source kind with no log refuses with its code.
        client.query.register_foreign_source(
            "no-log",
            {
                "HttpJson": {
                    "url": "https://x.example.test/r",
                    "json_path": "",
                    "field_map": {"id": "id"},
                }
            },
        )
        job = client.jobs.submit(graph, {"SourceQueryLogIngest": {"source": "no-log"}})
        for _ in range(300):
            state = client.jobs.status(job["job_id"])["state"]
            if isinstance(state, dict) and "Failed" in state:
                assert "QUERY_LOG_UNSUPPORTED" in str(state)
                break
            time.sleep(0.1)
        else:
            raise AssertionError("an unsupported source did not fail its job")
    finally:
        client.tenants.delete(graph)
        client.close()


def _usage(client: Any, graph: str, asset: str, top_n: int | None = None) -> dict:
    return _decode(
        client._send("CatalogUsage", {"asset": asset, "top_n": top_n}, graph)
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.8")
def test_frequent_users_and_join_suggestions(
    start_medulla_server, pg_log_source
) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.8: users, suggestions, scope."""
    from medulla.client import SyncMedullaClient

    dsn, run_as = pg_log_source
    graph = f"usage-top-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        log = _Log(client, graph, dsn, run_as)
        log.alice_joins(4)
        log.bob_reads(2)
        _ingest(client, graph, log.source)

        answer = _usage(client, graph, log.orders)
        by_principal = {u["principal"]: u["reads"] for u in answer["frequent_users"]}
        assert by_principal[f"source:{log.source}:{log.alice}"] == 4
        assert by_principal[f"source:{log.source}:{log.bob}"] == 2
        assert answer["frequent_users"][0]["reads"] == 4
        kinds = {(s["kind"], s["target"]): s["count"] for s in answer["suggestions"]}
        assert kinds[("join", log.customers)] == 4
        assert kinds[("filter", "status")] == 4
        assert ("filter", "total") not in kinds  # 2 uses is under the minimum of 3

        assert len(_usage(client, graph, log.orders, top_n=1)["frequent_users"]) == 1
        with pytest.raises(Exception, match="CATALOG_LIMIT_INVALID"):
            _usage(client, graph, log.orders, top_n=0)
        with pytest.raises(Exception, match="CATALOG_ASSET_NOT_FOUND"):
            _usage(client, graph, "no_such_table")

        # Without the scope the counts stay and the identities go.
        plain = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(scopes=["kg:read", "query:unified"]),
        )
        hidden = _usage(plain, graph, log.orders)
        assert [u["principal"] for u in hidden["frequent_users"]] == [None, None]
        assert [u["reads"] for u in hidden["frequent_users"]] == [4, 2]
        plain.close()

        # Another tenant sees no trace of the asset.
        other = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context={**request_context(), "tenant": "tenant:other"},
        )
        with pytest.raises(Exception, match="."):
            _usage(other, graph, log.orders)
        other.close()
    finally:
        client.tenants.delete(graph)
        client.close()
